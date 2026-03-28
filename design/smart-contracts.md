# Smart Contracts (WASM Runtime)

## Purpose

Executes WebAssembly smart contracts deterministically within the block execution pipeline. Provides a sandboxed, gas-metered environment with a host API for storage access, crypto primitives, and inter-contract calls.

## Scope

**In scope:**
- WASM module validation before deployment (no floats, no SIMD, no threads, no bulk memory, `rustbft`-only imports)
- Module compilation and caching via wasmtime `Engine`
- Gas metering via wasmtime fuel consumption
- Host API: storage read/write/delete/has, caller/origin/self/block context, sha256, ed25519 verify, event emission, nested contract call
- Snapshot isolation: contracts run inside an `AppState` snapshot, reverted on failure

**Out of scope:**
- Contract upgrades / proxy patterns
- ABI encoding/decoding (raw bytes only)
- Off-chain event indexing (events emitted but not persisted)
- Code storage in `AppState` (handled by `StateExecutor::exec_deploy` via `AppState::set_code`)

## Primary User Flow

**Deploy:**
1. `ContractDeployTx` arrives in `StateExecutor::exec_deploy`.
2. `ContractRuntime::deploy(wasm)` validates module and compiles it.
3. `code_hash = sha256(wasm)` stored as key in `runtime.modules` BTreeMap.
4. Contract address derived as `sha256(sender||nonce)[..20]`.
5. If `init_input` non-empty, constructor called via `ContractRuntime::call`.

**Call:**
1. `ContractCallTx` arrives in `StateExecutor::exec_call`.
2. `ContractRuntime::call(app, code_hash, input, ctx)` looks up compiled `Module`.
3. `AppState` moved into wasmtime `Store` via `HostState`; fuel set to `gas_limit`.
4. Input written to WASM memory at offset `0x1000`.
5. `call(input_ptr, input_len) -> ret_ptr` exported function invoked.
6. Return format at `ret_ptr`: `[status(1B)][len(4B BE)][data(len B)]`.
7. `AppState` moved back out; commit or revert snapshot.

## System Flow

```
StateExecutor::exec_deploy / exec_call
    │
    ├── validate_wasm_module (src/contracts/validation.rs)
    │       wasmparser scan: reject floats/SIMD/threads/bulk-memory/start-fn
    │       check max_code_bytes (512KB), max_memory_pages (256 = 16MB)
    │       check all imports from module "rustbft" only
    │
    ├── ContractRuntime::deploy (src/contracts/runtime.rs)
    │       Module::new(&engine, wasm) → compile via Cranelift
    │       modules.insert(sha256(wasm), module)
    │
    └── ContractRuntime::call (src/contracts/runtime.rs)
            app.snapshot()
            Store::new(engine, HostState { app, env, costs, events, hook })
            store.set_fuel(gas_limit)   [1 fuel = 1 gas]
            Linker::new + add_host_functions → linker.instantiate
            memory.write(0x1000, input)
            call_fn.call(input_ptr, input_len) → ret_ptr
            read [status(1B)][len(4B)][output(len B)] at ret_ptr
            gas_used = gas_limit - store.get_fuel()
            success → app.commit_snapshot / failure → app.revert_to
```

## Data Model

`ContractRuntime` (`src/contracts/runtime.rs`):
- `engine: Engine` — shared wasmtime compilation engine; NaN canonicalization on, SIMD off, threads off, fuel on
- `modules: BTreeMap<Hash, Module>` — compiled modules keyed by `sha256(wasm)`; in-memory only (not persisted across restarts)
- `limits: ValidationLimits { max_code_bytes: 512*1024, max_memory_pages: 256 }`
- `costs: HostCosts` — gas costs per host function (see below)

`HostCosts` (`src/contracts/host_api.rs`):
- `storage_read: 500`, `storage_write: 5000`, `storage_delete: 2500`, `storage_has: 200`
- `ctx_get: 100`, `call_base: 10000`, `emit_base: 1000`, `emit_per_32b: 100`
- `sha256_base: 100`, `sha256_per_32b: 10`, `sig_verify: 3000`

`HostState` (`src/contracts/host_api.rs`):
- Owned `app: AppState` — moved into Store for execution, moved back out after
- `env: CallEnv { self_addr, caller, origin, block_height, block_timestamp_ms, value, call_depth }`
- `events: Vec<EventLog { topic: Vec<u8>, data: Vec<u8> }>` — accumulated during execution

## Interfaces and Contracts

**Host API** (WASM import module `"rustbft"`):
- `host_storage_read(key_ptr, key_len, out_ptr) -> (exists: i32, out_len: i32)` — reads keyed by `(self_addr, key)`
- `host_storage_write(key_ptr, key_len, val_ptr, val_len)` — writes `contract_storage[(self_addr, key)]`
- `host_storage_delete(key_ptr, key_len)` — deletes `contract_storage[(self_addr, key)]`
- `host_storage_has(key_ptr, key_len) -> i32` — 1 if exists, 0 if not
- `host_get_caller(out_ptr)`, `host_get_origin(out_ptr)`, `host_get_self_address(out_ptr)` — write 20-byte address
- `host_get_block_height() -> u64`, `host_get_block_timestamp() -> u64`
- `host_get_value() -> u64` — value truncated to u64 (MVP limitation)
- `host_get_gas_remaining() -> u64`
- `host_sha256(data_ptr, data_len, out_ptr)` — writes 32-byte hash
- `host_verify_ed25519(pk_ptr, sig_ptr, msg_ptr, msg_len) -> i32` — 1=valid, 0=invalid
- `host_emit_event(topic_ptr, topic_len, data_ptr, data_len)` — appends to `events`
- `host_call_contract(addr_ptr, addr_len, input_ptr, input_len, gas_limit, out_ptr) -> i32` — 1=success
- `host_abort(ptr, len)` — panics with "abort"

**Contract export required:** `call(input_ptr: u32, input_len: u32) -> ret_ptr: u32`

**Memory:** Contract must export `"memory"`; input written at offset `0x1000`.

**`ContractRuntime::deploy(wasm) -> Result<DeployResult, ContractError>`**:
- `DeployResult { code_hash: Hash }`
- Errors: `InvalidModule(String)` if validation or compilation fails

**`ContractRuntime::call(app, code_hash, input, ctx) -> Result<CallResult, ContractError>`**:
- `CallResult { success: bool, output: Vec<u8>, gas_used: u64, events: Vec<(Vec<u8>, Vec<u8>)> }`
- Errors: `ModuleNotFound`, `OutOfGas`, `Trap(String)`, `MissingCallExport`

## Dependencies

**Internal modules:**
- `src/state/accounts.rs` — `AppState` moved into `HostState` for execution
- `src/crypto/hash.rs` — `sha256` for `code_hash` and `host_sha256`

**External crates:**
- `wasmtime v17` — Cranelift JIT compiler, fuel-based gas metering, linker
- `wasmparser v0.120` — static validation of WASM bytecode
- `ed25519-dalek` — `VerifyingKey::verify_strict` in `host_verify_ed25519`

## Failure Modes and Edge Cases

- **Module not in cache after restart:** `modules` BTreeMap is not persisted; re-deployment required after node restart. A `ContractRuntime::call` for a deployed contract whose module was lost returns `ContractError::ModuleNotFound`.
- **Call depth > 64:** `host_call_contract` returns 0 (failure) immediately without executing.
- **Out of gas:** wasmtime traps with "fuel" message → `map_trap` returns `ContractError::OutOfGas` → `StateExecutor` reverts snapshot and charges full `gas_limit`.
- **WASM trap:** Returns `ContractError::Trap(msg)` → revert, charge intrinsic.
- **input > memory size:** `memory.write` fails with "oob" → `ContractError::Trap`.
- **`host_get_value` truncation:** `value: u128` is cast to `u64` — values > `u64::MAX` are silently truncated.
- **Float in WASM:** Rejected at validation by `validate_wasm_module` → `ValidationError::FloatNotAllowed`.
- **Nested call state corruption:** `call_contract_hook` uses a raw pointer (`*mut ContractRuntime`) unsafely — re-entrant calls into the same `ContractRuntime` are possible and unsafe.

## Observability and Debugging

- No structured logs in contract execution path.
- `CallResult.gas_used` available after each call but not logged.
- Debugging starting point: `ContractRuntime::call` at `src/contracts/runtime.rs:call` — add logging before/after `call_fn.call`.

## Risks and Notes

- `call_contract_hook` uses `unsafe { &mut *rt_ptr }` — undefined behavior if the runtime is moved or dropped during a nested call.
- Compiled `modules` are lost on restart; all contracts must be redeployed or the runtime needs persistent module caching.
- `value` truncated from `u128` to `u64` in `host_get_value` — token transfers via nested calls will silently lose precision for large amounts.
- No re-entrancy guard beyond `call_depth <= 64` — contracts can call back into themselves.
- Events are collected in `HostState.events` but not persisted anywhere after `execute_block` returns.

Changes:

