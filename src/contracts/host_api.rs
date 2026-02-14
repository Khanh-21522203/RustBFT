use crate::crypto::hash::sha256;
use crate::state::AppState;
use crate::types::Address;
use ed25519_dalek::{Signature, VerifyingKey};
use wasmtime::Caller;

#[derive(Clone, Debug)]
pub struct HostCosts {
    pub storage_read: u64,   // 500
    pub storage_write: u64,  // 5000
    pub storage_delete: u64, // 2500
    pub storage_has: u64,    // 200
    pub ctx_get: u64,        // 100
    pub call_base: u64,      // 10000
    pub emit_base: u64,      // 1000
    pub emit_per_32b: u64,   // 100
    pub sha256_base: u64,    // 100
    pub sha256_per_32b: u64, // 10
    pub sig_verify: u64,     // 3000
}

impl Default for HostCosts {
    fn default() -> Self {
        Self {
            storage_read: 500,
            storage_write: 5000,
            storage_delete: 2500,
            storage_has: 200,
            ctx_get: 100,
            call_base: 10000,
            emit_base: 1000,
            emit_per_32b: 100,
            sha256_base: 100,
            sha256_per_32b: 10,
            sig_verify: 3000,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CallEnv {
    pub self_addr: Address,
    pub caller: Address,
    pub origin: Address,
    pub block_height: u64,
    pub block_timestamp_ms: u64,
    pub value: u128,
    pub call_depth: u32,
}

#[derive(Clone, Debug)]
pub struct EventLog {
    pub topic: Vec<u8>,
    pub data: Vec<u8>,
}

pub struct HostState {
    pub app: AppState,
    pub env: CallEnv,
    pub costs: HostCosts,
    pub events: Vec<EventLog>,
    pub call_contract_hook: Box<dyn FnMut(&mut AppState, &CallEnv, Address, Vec<u8>, u128, u64) -> anyhow::Result<(bool, Vec<u8>)>>,
}

fn charge(caller: &mut Caller<'_, HostState>, gas: u64) -> anyhow::Result<()> {
    let fuel = caller.get_fuel().map_err(|_| anyhow::anyhow!("fuel unavailable"))?;
    if fuel < gas {
        anyhow::bail!("out of gas");
    }
    caller.set_fuel(fuel - gas).map_err(|_| anyhow::anyhow!("out of gas"))?;
    Ok(())
}

fn mem_read(caller: &mut Caller<'_, HostState>, ptr: u32, len: u32) -> anyhow::Result<Vec<u8>> {
    let mem = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| anyhow::anyhow!("missing memory export"))?;

    let mut buf = vec![0u8; len as usize];
    mem.read(caller, ptr as usize, &mut buf)
        .map_err(|_| anyhow::anyhow!("memory read oob"))?;
    Ok(buf)
}

fn mem_write(caller: &mut Caller<'_, HostState>, ptr: u32, data: &[u8]) -> anyhow::Result<()> {
    let mem = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| anyhow::anyhow!("missing memory export"))?;

    mem.write(caller, ptr as usize, data)
        .map_err(|_| anyhow::anyhow!("memory write oob"))?;
    Ok(())
}

pub fn add_host_functions(linker: &mut wasmtime::Linker<HostState>) -> anyhow::Result<()> {
    // storage_read(key_ptr, key_len, out_ptr) -> (exists: i32, out_len: i32)
    linker.func_wrap(
        "rustbft",
        "host_storage_read",
        |mut caller: Caller<'_, HostState>, key_ptr: u32, key_len: u32, out_ptr: u32| -> anyhow::Result<(i32, i32)> {
            let cost = caller.data().costs.storage_read;
            charge(&mut caller, cost)?;

            let key = mem_read(&mut caller, key_ptr, key_len)?;
            let addr = caller.data().env.self_addr;

            let val = caller.data().app.contract_storage.get(&(addr, key.clone())).cloned();
            if let Some(v) = val {
                let len = v.len() as i32;
                mem_write(&mut caller, out_ptr, &v)?;
                Ok((1, len))
            } else {
                Ok((0, 0))
            }
        },
    )?;

    linker.func_wrap(
        "rustbft",
        "host_storage_write",
        |mut caller: Caller<'_, HostState>,
         key_ptr: u32,
         key_len: u32,
         val_ptr: u32,
         val_len: u32| -> anyhow::Result<()> {
            let cost = caller.data().costs.storage_write;
            charge(&mut caller, cost)?;

            let key = mem_read(&mut caller, key_ptr, key_len)?;
            let val = mem_read(&mut caller, val_ptr, val_len)?;
            let addr = caller.data().env.self_addr;

            caller.data_mut().app.set_storage(addr, key, val);
            Ok(())
        },
    )?;

    linker.func_wrap(
        "rustbft",
        "host_storage_delete",
        |mut caller: Caller<'_, HostState>, key_ptr: u32, key_len: u32| -> anyhow::Result<()> {
            let cost = caller.data().costs.storage_delete;
            charge(&mut caller, cost)?;

            let key = mem_read(&mut caller, key_ptr, key_len)?;
            let addr = caller.data().env.self_addr;

            caller.data_mut().app.delete_storage(addr, key);
            Ok(())
        },
    )?;

    linker.func_wrap(
        "rustbft",
        "host_storage_has",
        |mut caller: Caller<'_, HostState>, key_ptr: u32, key_len: u32| -> anyhow::Result<i32> {
            let cost = caller.data().costs.storage_has;
            charge(&mut caller, cost)?;

            let key = mem_read(&mut caller, key_ptr, key_len)?;
            let addr = caller.data().env.self_addr;

            Ok(if caller.data().app.contract_storage.contains_key(&(addr, key)) { 1 } else { 0 })
        },
    )?;

    linker.func_wrap("rustbft", "host_get_caller", |mut caller: Caller<'_, HostState>, out_ptr: u32| -> anyhow::Result<()> {
        let cost = caller.data().costs.ctx_get;
        charge(&mut caller, cost)?;
        let addr = caller.data().env.caller.0;
        mem_write(&mut caller, out_ptr, &addr)?;
        Ok(())
    })?;

    linker.func_wrap("rustbft", "host_get_origin", |mut caller: Caller<'_, HostState>, out_ptr: u32| -> anyhow::Result<()> {
        let cost = caller.data().costs.ctx_get;
        charge(&mut caller, cost)?;
        let addr = caller.data().env.origin.0;
        mem_write(&mut caller, out_ptr, &addr)?;
        Ok(())
    })?;

    linker.func_wrap("rustbft", "host_get_self_address", |mut caller: Caller<'_, HostState>, out_ptr: u32| -> anyhow::Result<()> {
        let cost = caller.data().costs.ctx_get;
        charge(&mut caller, cost)?;
        let addr = caller.data().env.self_addr.0;
        mem_write(&mut caller, out_ptr, &addr)?;
        Ok(())
    })?;

    linker.func_wrap("rustbft", "host_get_block_height", |mut caller: Caller<'_, HostState>| -> anyhow::Result<u64> {
        let cost = caller.data().costs.ctx_get;
        charge(&mut caller, cost)?;
        Ok(caller.data().env.block_height)
    })?;

    linker.func_wrap("rustbft", "host_get_block_timestamp", |mut caller: Caller<'_, HostState>| -> anyhow::Result<u64> {
        let cost = caller.data().costs.ctx_get;
        charge(&mut caller, cost)?;
        Ok(caller.data().env.block_timestamp_ms)
    })?;

    linker.func_wrap("rustbft", "host_get_value", |mut caller: Caller<'_, HostState>| -> anyhow::Result<u64> {
        let cost = caller.data().costs.ctx_get;
        charge(&mut caller, cost)?;
        // MVP: value truncated to u64 for WASM ABI compatibility (WASM has no native u128)
        Ok(caller.data().env.value as u64)
    })?;

    linker.func_wrap("rustbft", "host_get_gas_remaining", |caller: Caller<'_, HostState>| -> anyhow::Result<u64> {
        Ok(caller.get_fuel().unwrap_or(0))
    })?;

    // host_sha256(data_ptr, data_len, out_ptr)
    linker.func_wrap(
        "rustbft",
        "host_sha256",
        |mut caller: Caller<'_, HostState>, data_ptr: u32, data_len: u32, out_ptr: u32| -> anyhow::Result<()> {
            let chunks = ((data_len as u64) + 31) / 32;
            let gas = caller.data().costs.sha256_base + caller.data().costs.sha256_per_32b * chunks;
            charge(&mut caller, gas)?;

            let data = mem_read(&mut caller, data_ptr, data_len)?;
            let h = sha256(&data);
            mem_write(&mut caller, out_ptr, &h.0)?;
            Ok(())
        },
    )?;

    // host_verify_ed25519(pubkey_ptr, sig_ptr, msg_ptr, msg_len) -> i32
    linker.func_wrap(
        "rustbft",
        "host_verify_ed25519",
        |mut caller: Caller<'_, HostState>, pk_ptr: u32, sig_ptr: u32, msg_ptr: u32, msg_len: u32| -> anyhow::Result<i32> {
            let cost = caller.data().costs.sig_verify;
            charge(&mut caller, cost)?;

            let pk = mem_read(&mut caller, pk_ptr, 32)?;
            let sig = mem_read(&mut caller, sig_ptr, 64)?;
            let msg = mem_read(&mut caller, msg_ptr, msg_len)?;

            let pk = VerifyingKey::from_bytes(pk.as_slice().try_into().map_err(|_| anyhow::anyhow!("bad pubkey"))?)
                .map_err(|_| anyhow::anyhow!("bad pubkey"))?;
            let sig = Signature::from_bytes(sig.as_slice().try_into().map_err(|_| anyhow::anyhow!("bad sig"))?);

            Ok(if pk.verify_strict(&msg, &sig).is_ok() { 1 } else { 0 })
        },
    )?;

    // host_emit_event(topic_ptr, topic_len, data_ptr, data_len)
    linker.func_wrap(
        "rustbft",
        "host_emit_event",
        |mut caller: Caller<'_, HostState>, topic_ptr: u32, topic_len: u32, data_ptr: u32, data_len: u32| -> anyhow::Result<()> {
            let chunks = ((data_len as u64) + 31) / 32;
            let gas = caller.data().costs.emit_base + caller.data().costs.emit_per_32b * chunks;
            charge(&mut caller, gas)?;

            let topic = mem_read(&mut caller, topic_ptr, topic_len)?;
            let data = mem_read(&mut caller, data_ptr, data_len)?;

            caller.data_mut().events.push(EventLog { topic, data });
            Ok(())
        },
    )?;

    // host_call_contract(addr_ptr, addr_len, input_ptr, input_len, gas_limit, out_ptr) -> success: i32
    // For MVP: addr_len must be 20 bytes. value is taken from env.
    linker.func_wrap(
        "rustbft",
        "host_call_contract",
        |mut caller: Caller<'_, HostState>,
         addr_ptr: u32,
         addr_len: u32,
         input_ptr: u32,
         input_len: u32,
         gas_limit: u64,
         out_ptr: u32| -> anyhow::Result<i32> {
            let cost = caller.data().costs.call_base;
            charge(&mut caller, cost)?;

            if caller.data().env.call_depth >= 64 {
                return Ok(0);
            }

            let addr_bytes = mem_read(&mut caller, addr_ptr, addr_len)?;
            if addr_bytes.len() != 20 {
                anyhow::bail!("bad address length");
            }
            let mut a = [0u8; 20];
            a.copy_from_slice(&addr_bytes);
            let callee = Address(a);

            let input = mem_read(&mut caller, input_ptr, input_len)?;
            let value = caller.data().env.value;
            let env_clone = caller.data().env.clone();
            let mut hook = std::mem::replace(
                &mut caller.data_mut().call_contract_hook,
                Box::new(|_, _, _, _, _, _| Ok((false, vec![]))),
            );
            let result = hook(&mut caller.data_mut().app, &env_clone, callee, input, value, gas_limit);
            caller.data_mut().call_contract_hook = hook;

            let (ok, out) = result?;
            mem_write(&mut caller, out_ptr, &out)?;
            Ok(if ok { 1 } else { 0 })
        },
    )?;

    linker.func_wrap("rustbft", "host_abort", |_caller: Caller<'_, HostState>, _ptr: u32, _len: u32| -> anyhow::Result<()> {
        anyhow::bail!("abort")
    })?;

    Ok(())
}
