# Feature: Smart Contract Execution (WASM)

## 1. Purpose

The Smart Contracts module provides a deterministic, gas-metered, sandboxed execution environment for user-deployed WebAssembly contracts. It implements the `ContractEngine` trait expected by the state machine, using `wasmtime` as the WASM runtime with Cranelift-based AOT compilation.

Determinism is the paramount constraint. A contract that produces different results on different nodes breaks consensus. Every non-deterministic capability (randomness, clocks, network, filesystem) is prohibited at the host API level. Floating-point WASM instructions are rejected at deploy time.

## 2. Responsibilities

- Validate WASM modules at deploy time: reject floats, SIMD, threads, disallowed imports, oversized code
- AOT-compile validated WASM modules to native code at deploy time; cache compiled modules by `code_hash`
- Execute contract constructor (`init` function) during `ContractDeploy`
- Instantiate and call compiled modules during `ContractCall`
- Provide a sandboxed host API: state read/write, caller context, event emission, SHA-256, Ed25519 verify, cross-contract calls
- Enforce gas metering via wasmtime's fuel mechanism (1 fuel = 1 gas)
- Enforce bounded linear memory (max 16 MB per instance)
- Enforce bounded call stack depth (max 64 nested cross-contract calls)
- Snapshot/revert contract storage on call failure
- Support read-only query execution (no state mutation, no gas charge for RPC queries)

## 3. Non-Responsibilities

- Does not allocate contract addresses — the state machine does that
- Does not manage account balances — the state machine does that
- Does not persist compiled modules — the storage layer caches them
- Does not support async WASM, threads, or SIMD
- Does not provide EVM compatibility

## 4. Architecture Design

```
State Machine Thread
        |
  ContractEngine::deploy() / call()
        |
+-------v-----------------------------------------+
|   WasmContractEngine (src/contracts/runtime.rs)  |
|                                                  |
|  Module Cache: code_hash → CompiledModule         |
|                                                  |
|  deploy():                                       |
|    validate_module(bytecode)                     |
|    compile: wasmtime::Module::new(bytecode)      |
|    execute init(args) with fuel                  |
|    cache compiled module                         |
|                                                  |
|  call():                                         |
|    load module from cache                        |
|    instantiate with host API linker              |
|    set fuel = gas_limit                          |
|    call entry point                              |
|    collect state changes, events                 |
|    if error: revert state changes                |
+-------+-----------------------------------------+
        |
  Host API callbacks (src/contracts/host_api.rs)
  - host_storage_read/write/delete
  - host_get_caller / host_get_block_height / ...
  - host_call_contract (nested, with snapshot)
  - host_emit_event / host_sha256 / host_verify_ed25519
```

## 5. Core Data Structures (Rust)

```rust
// src/contracts/runtime.rs

pub struct WasmContractEngine {
    engine: wasmtime::Engine,
    // Compiled module cache: code_hash → wasmtime::Module
    module_cache: BTreeMap<Hash, wasmtime::Module>,
    config: ContractConfig,
}

pub struct ContractConfig {
    pub max_code_size_bytes: usize,    // default: 512 KB
    pub max_memory_pages: u32,         // default: 256 (16 MB)
    pub max_call_depth: usize,         // default: 64
    pub max_wasm_stack_size: usize,    // default: 1 MB
}

// src/contracts/host_api.rs

/// All state accessible during contract execution.
/// Passed as mutable reference into wasmtime host functions.
pub struct HostContext<'a> {
    pub state: &'a mut AppState,
    pub caller: Address,
    pub origin: Address,
    pub self_address: Address,
    pub block_ctx: BlockContext,
    pub value: u128,
    pub gas_remaining: u64,
    pub depth: usize,
    pub events: Vec<Event>,
    pub snapshot_stack: Vec<StateSnapshot>,
}

// src/contracts/validation.rs

pub struct ValidationError {
    pub kind: ValidationErrorKind,
    pub message: String,
}

pub enum ValidationErrorKind {
    InvalidMagic,
    FloatInstruction,
    SimdInstruction,
    UnknownImport { module: String, name: String },
    MissingExport { name: String },
    CodeTooLarge { size: usize, max: usize },
    MemoryTooLarge { pages: u32, max: u32 },
    HasStartFunction,
}
```

## 6. Public Interfaces

```rust
// src/contracts/mod.rs

impl WasmContractEngine {
    pub fn new(config: ContractConfig) -> Self;
}

impl ContractEngine for WasmContractEngine {
    fn deploy(
        &mut self,
        deployer: Address,
        code: &[u8],
        init_args: &[u8],
        gas_limit: u64,
        state: &mut AppState,
    ) -> Result<(Address, u64), ContractError>;

    fn call(
        &mut self,
        caller: Address,
        contract: Address,
        input: &[u8],
        value: u128,
        gas_limit: u64,
        state: &mut AppState,
    ) -> Result<(Vec<u8>, u64, Vec<Event>), ContractError>;
}

// Validation (called before compile at deploy time)
pub fn validate_module(code: &[u8], config: &ContractConfig) -> Result<(), ValidationError>;

// Host API (exposed to WASM as imports in the "rustbft" namespace)
// host_storage_read(key_ptr, key_len) -> (value_ptr, value_len, exists)
// host_storage_write(key_ptr, key_len, val_ptr, val_len)
// host_storage_delete(key_ptr, key_len)
// host_storage_has(key_ptr, key_len) -> bool
// host_get_caller() -> address_ptr
// host_get_origin() -> address_ptr
// host_get_self_address() -> address_ptr
// host_get_block_height() -> u64
// host_get_block_timestamp() -> u64
// host_get_value() -> u128
// host_get_gas_remaining() -> u64
// host_call_contract(addr_ptr, addr_len, input_ptr, input_len, value, gas) -> (success, output_ptr, output_len)
// host_emit_event(topic_ptr, topic_len, data_ptr, data_len)
// host_sha256(data_ptr, data_len) -> hash_ptr
// host_verify_ed25519(pubkey_ptr, sig_ptr, msg_ptr, msg_len) -> bool
// host_abort(msg_ptr, msg_len)
```

## 7. Internal Algorithms

### Deploy-Time Module Validation
```
fn validate_module(code):
    // 1. Parse WASM binary (check magic bytes + version)
    module = wasmparser::parse(code)?

    // 2. Walk all instructions; reject float and SIMD
    for instruction in module.all_instructions():
        if instruction.is_float():  return Err(FloatInstruction)
        if instruction.is_simd():   return Err(SimdInstruction)

    // 3. Validate imports: all must be from "rustbft" namespace
    for import in module.imports():
        if import.module != "rustbft": return Err(UnknownImport)
        if import.name not in HOST_API_NAMES: return Err(UnknownImport)

    // 4. Check exports: must have "init" (for deploy) or "call" (for calls)
    if module.exports().find("init").is_none() AND module.exports().find("call").is_none():
        return Err(MissingExport)

    // 5. Size limits
    if code.len() > config.max_code_size_bytes: return Err(CodeTooLarge)

    // 6. Memory declaration
    for mem in module.memories():
        if mem.maximum.unwrap_or(u32::MAX) > config.max_memory_pages: return Err(MemoryTooLarge)

    // 7. No start function
    if module.start().is_some(): return Err(HasStartFunction)

    Ok(())
```

### Gas / Fuel Mapping
```
wasmtime Engine config:
    consume_fuel = true

Before call:
    store.add_fuel(gas_limit)

After call:
    fuel_remaining = store.fuel_consumed()  // actually, store.get_fuel()
    gas_used = gas_limit - fuel_remaining

Out-of-fuel trap:
    wasmtime returns Trap::OutOfFuel
    → revert tx state
    → gas_used = gas_limit (fully charged)
```

### Cross-Contract Call (Nested)
```
fn host_call_contract(ctx, address, input, value, gas_limit):
    if ctx.depth >= MAX_CALL_DEPTH: return error
    snapshot = ctx.state.snapshot()
    ctx.depth += 1
    nested_result = engine.call(ctx.self_address, address, input, value, gas_limit, ctx.state)
    ctx.depth -= 1
    match nested_result:
        Ok((output, gas_used, events)):
            ctx.gas_remaining -= gas_used
            ctx.events.extend(events)
            return (true, output)
        Err(e):
            ctx.state.revert_to(snapshot)  // callee state reverted; caller continues
            ctx.gas_remaining -= gas_used_before_failure
            return (false, error_bytes)
```

### Out-of-Gas Behavior
```
When fuel reaches 0:
    wasmtime traps with OutOfFuel
    host catches the trap
    revert all state changes for this tx
    gas_used = gas_limit  (fully charged)
    return Receipt { success: false, error: "out of gas" }
```

### ABI Encoding
```
Function call input:
    [ selector: 4 bytes ] [ args: variable ]
    selector = sha256(function_signature)[0..4]

Return value:
    [ status: 1 byte ] [ data_len: 4 bytes BE ] [ data: variable ]
    status: 0 = success, 1 = revert, 2 = error
```

## 8. Persistence Model

The `WasmContractEngine` caches compiled `wasmtime::Module` objects in a `BTreeMap<Hash, wasmtime::Module>` in-memory. On restart the cache is cold; modules are re-compiled from bytecode fetched from the state store (storage layer). Re-compilation is deterministic and fast with Cranelift. A persistent module cache (serialized native code) is a future optimization.

Contract storage values (`host_storage_read/write`) are stored in `AppState.contract_storage` as `BTreeMap<(Address, Bytes), Bytes>` in memory and flushed to RocksDB via the storage layer on `Commit`.

## 9. Concurrency Model

`WasmContractEngine` is owned by the state machine thread. Each WASM instantiation creates a fresh `wasmtime::Store` that owns the linear memory and fuel counter. WASM execution is fully synchronous (no threads, no async). Cross-contract calls are stack-based within the same thread.

The `ContractEngine` trait bound is `Send` to allow the state machine to move it across thread boundaries at startup, but it is never accessed from more than one thread simultaneously.

## 10. Configuration

```toml
[contracts]
max_code_size_bytes = 524288     # 512 KB
max_memory_pages = 256           # 16 MB per instance
max_call_depth = 64
max_wasm_stack_size = 1048576    # 1 MB
```

Wasmtime engine is configured with:
- `consume_fuel = true`
- `wasm_simd = false`
- `wasm_threads = false`
- `wasm_bulk_memory = false`
- `cranelift_opt_level = Speed`

## 11. Observability

- `rustbft_contract_executions_total{result}` (Counter) — success / failure / out_of_gas
- `rustbft_contract_gas_used` (Histogram) — gas consumed per call
- `rustbft_contract_deploy_total` (Counter)
- `rustbft_contract_call_depth` (Histogram) — nested call depth distribution
- Log DEBUG for each contract call (address, function selector, gas_used); WARN on deploy validation failure

## 12. Testing Strategy

- **`test_validate_valid_module`**: minimal WASM module with `init` and `call` exports → validation passes
- **`test_validate_rejects_floats`**: module with `f32.add` → `Err(FloatInstruction)`
- **`test_validate_rejects_simd`**: module with `i8x16.add` → `Err(SimdInstruction)`
- **`test_validate_rejects_unknown_import`**: module importing `env.malloc` → `Err(UnknownImport)`
- **`test_validate_rejects_oversized`**: module exceeding `max_code_size_bytes` → `Err(CodeTooLarge)`
- **`test_deploy_and_call_counter_contract`**: deploy counter WASM, call `increment()`, call `get_count()` → assert count=1
- **`test_gas_exhaustion`**: call with gas_limit too low → Out-of-fuel trap, state reverted, full gas charged
- **`test_contract_failure_reverts_state`**: contract aborts mid-execution → prior state changes reverted
- **`test_cross_contract_call`**: contract A calls contract B; B succeeds → both state changes persist
- **`test_cross_contract_call_callee_fails`**: contract A calls B; B aborts → B state reverted; A continues
- **`test_call_depth_limit`**: 65 nested calls → 65th returns error (depth exceeded)
- **`test_host_storage_read_write`**: write key→value, read back → same value
- **`test_host_sha256`**: contract calls `host_sha256("abc")` → matches known SHA-256("abc")
- **`test_deterministic_execution`**: same call twice → same output, same gas_used, same state

## 13. Open Questions

- **Module cache persistence**: Should compiled native code be stored on disk to avoid re-compilation on restart? For MVP, in-memory cache only; re-compilation on cold start is acceptable given small number of contracts.
