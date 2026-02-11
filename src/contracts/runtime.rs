use crate::contracts::host_api::{add_host_functions, CallEnv, HostCosts, HostState};
use crate::contracts::validation::{validate_wasm_module, ValidationLimits};
use crate::crypto::hash::sha256;
use crate::state::AppState;
use crate::types::{Address, Hash};
use std::collections::BTreeMap;
use thiserror::Error;
use wasmtime::{Config, Engine, Linker, Module, Store};

#[derive(Error, Debug)]
pub enum ContractError {
    #[error("invalid module: {0}")]
    InvalidModule(String),
    #[error("module not found")]
    ModuleNotFound,
    #[error("execution trapped: {0}")]
    Trap(String),
    #[error("out of gas")]
    OutOfGas,
    #[error("missing export: call")]
    MissingCallExport,
}

#[derive(Clone, Debug)]
pub struct CallContext {
    pub self_addr: Address,
    pub caller: Address,
    pub origin: Address,
    pub block_height: u64,
    pub block_timestamp_ms: u64,
    pub value: u128,
    pub gas_limit: u64,
    pub call_depth: u32,
}

#[derive(Clone, Debug)]
pub struct CallResult {
    pub success: bool,
    pub output: Vec<u8>,
    pub gas_used: u64,
    pub events: Vec<(Vec<u8>, Vec<u8>)>,
}

#[derive(Clone, Debug)]
pub struct DeployResult {
    pub code_hash: Hash,
}

pub struct ContractRuntime {
    engine: Engine,
    modules: BTreeMap<Hash, Module>,
    limits: ValidationLimits,
    costs: HostCosts,
}

impl ContractRuntime {
    pub fn new() -> anyhow::Result<Self> {
        let mut cfg = Config::new();

        // Determinism / sandbox knobs
        cfg.wasm_simd(false);
        cfg.wasm_threads(false);
        cfg.consume_fuel(true);
        cfg.cranelift_nan_canonicalization(true);

        // Note: max memory is enforced at validation + module limits by wasmtime.
        let engine = Engine::new(&cfg)?;

        Ok(Self {
            engine,
            modules: BTreeMap::new(),
            limits: ValidationLimits::default(),
            costs: HostCosts::default(),
        })
    }

    pub fn deploy(&mut self, wasm: &[u8]) -> Result<DeployResult, ContractError> {
        validate_wasm_module(wasm, &self.limits).map_err(|e| ContractError::InvalidModule(e.to_string()))?;

        let code_hash = sha256(wasm);
        let module = Module::new(&self.engine, wasm).map_err(|e| ContractError::InvalidModule(e.to_string()))?;

        self.modules.insert(code_hash, module);

        Ok(DeployResult { code_hash })
    }

    pub fn call(
        &mut self,
        app: &mut AppState,
        code_hash: Hash,
        input: &[u8],
        ctx: &CallContext,
    ) -> Result<CallResult, ContractError> {
        let module = self.modules.get(&code_hash).ok_or(ContractError::ModuleNotFound)?.clone();

        // Nested snapshot for isolation. If contract fails, revert its own changes.
        let snap = app.snapshot();

        let env = CallEnv {
            self_addr: ctx.self_addr,
            caller: ctx.caller,
            origin: ctx.origin,
            block_height: ctx.block_height,
            block_timestamp_ms: ctx.block_timestamp_ms,
            value: ctx.value,
            call_depth: ctx.call_depth,
        };

        // Move AppState into the store (HostState owns it now).
        // We'll take it back out after execution.
        let app_taken = std::mem::replace(app, AppState::new(Default::default()));

        // Create store with fuel = gas_limit (1 fuel = 1 gas)
        let mut store = Store::new(
            &self.engine,
            HostState {
                app: app_taken,
                env: env.clone(),
                costs: self.costs.clone(),
                events: Vec::new(),
                call_contract_hook: Box::new(|_app, _env, _addr, _input, _value, _gas| Ok((false, vec![]))),
            },
        );
        store.set_fuel(ctx.gas_limit).map_err(|_| ContractError::InvalidModule("fuel not supported".into()))?;

        let mut linker = Linker::new(&self.engine);
        add_host_functions(&mut linker).map_err(|e| ContractError::InvalidModule(e.to_string()))?;

        // Configure nested calls
        {
            let rt_ptr: *mut ContractRuntime = self;
            store.data_mut().call_contract_hook = Box::new(move |app2, env2, callee, input2, value2, gas2| {
                // This closure is sync and deterministic.
                unsafe {
                    let rt = &mut *rt_ptr;
                    // Find callee code_hash via its account
                    let acc = app2.get_account(callee);
                    let code_hash = acc.code_hash.ok_or_else(|| anyhow::anyhow!("callee has no code"))?;

                    let child_ctx = CallContext {
                        self_addr: callee,
                        caller: env2.self_addr,
                        origin: env2.origin,
                        block_height: env2.block_height,
                        block_timestamp_ms: env2.block_timestamp_ms,
                        value: value2,
                        gas_limit: gas2,
                        call_depth: env2.call_depth + 1,
                    };

                    let res = rt.call(app2, code_hash, &input2, &child_ctx);
                    match res {
                        Ok(r) => Ok((r.success, r.output)),
                        Err(_) => Ok((false, vec![])),
                    }
                }
            });
        }

        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| ContractError::InvalidModule(e.to_string()))?;

        // Write input to memory at offset 0x1000 (simple MVP convention)
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| ContractError::InvalidModule("missing memory export".into()))?;
        let input_ptr: u32 = 0x1000;
        memory.write(&mut store, input_ptr as usize, input).map_err(|_| ContractError::Trap("memory write oob".into()))?;

        let call_fn = instance
            .get_typed_func::<(u32, u32), u32>(&mut store, "call")
            .map_err(|_| ContractError::MissingCallExport)?;

        let call_result = call_fn.call(&mut store, (input_ptr, input.len() as u32));

        let remaining = store.get_fuel().unwrap_or(0);
        let gas_used = ctx.gas_limit.saturating_sub(remaining);

        // Collect events before taking app back
        let events: Vec<(Vec<u8>, Vec<u8>)> = store
            .data()
            .events
            .iter()
            .map(|ev| (ev.topic.clone(), ev.data.clone()))
            .collect();

        // Determine success and read output
        let (success, out) = match call_result {
            Ok(ret_ptr) => {
                // Read return format: [status(1)][len(4)][data(len)]
                let mut header = [0u8; 5];
                memory.read(&mut store, ret_ptr as usize, &mut header)
                    .map_err(|_| ContractError::Trap("return read oob".into()))?;
                let status = header[0];
                let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;

                let mut data = vec![0u8; len];
                memory.read(&mut store, (ret_ptr + 5) as usize, &mut data)
                    .map_err(|_| ContractError::Trap("return read oob".into()))?;

                (status == 0, data)
            }
            Err(e) => {
                // Take app back before returning error for OOG
                *app = store.into_data().app;
                app.revert_to(snap);

                return Err(map_trap(e));
            }
        };

        // Take AppState back from the store
        *app = store.into_data().app;

        if success {
            app.commit_snapshot(snap);
        } else {
            app.revert_to(snap);
        }

        Ok(CallResult {
            success,
            output: out,
            gas_used,
            events,
        })
    }
}

fn map_trap(e: anyhow::Error) -> ContractError {
    let s = e.to_string();
    if s.contains("out of gas") || s.contains("all fuel consumed") || s.contains("fuel") {
        ContractError::OutOfGas
    } else {
        ContractError::Trap(s)
    }
}
