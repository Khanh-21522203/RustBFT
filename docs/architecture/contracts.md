# RustBFT — Smart Contract Execution

**Purpose:** Define the deterministic smart contract execution engine: runtime, host API, gas metering, sandboxing, and deployment lifecycle.
**Audience:** Engineers implementing the contract execution layer.

---

## 1. Design Principles

1. **Deterministic** — identical inputs produce identical outputs on every node, every time.
2. **Synchronous** — no async/await, no callbacks, no futures.
3. **Sandboxed** — contracts cannot access the host OS, network, filesystem, or clock.
4. **Gas-metered** — every operation costs gas; execution halts when gas is exhausted.
5. **Isolated** — a failing contract reverts its own state without corrupting global state.

---

## 2. Runtime Choice: WebAssembly (WASM)

### Why WASM

| Property | Benefit for RustBFT |
|----------|---------------------|
| Deterministic execution | No undefined behavior, no platform-dependent results |
| Language-agnostic | Contracts can be written in Rust, C, AssemblyScript, etc. |
| Sandboxed by design | No host access unless explicitly imported |
| Metered execution | Fuel/gas injection at the WASM level |
| Mature tooling | wasmtime, wasmer provide production-grade runtimes |

### Runtime: wasmtime

- **wasmtime** is chosen for its focus on correctness, security, and Cranelift-based compilation.
- WASM modules are **AOT-compiled** at deploy time and cached.
- Execution uses wasmtime's **fuel metering** (mapped to RustBFT gas).

### WASM Constraints

| Constraint | Enforcement |
|------------|-------------|
| No floating-point | WASM modules MUST NOT use `f32` or `f64` instructions. Validated at deploy time. |
| No SIMD | SIMD proposal disabled in wasmtime config. |
| No threads | WASM threads proposal disabled. |
| No external imports beyond host API | Module validation rejects unknown imports. |
| Bounded memory | Linear memory capped at 16 MB (configurable). |
| Bounded execution | Gas/fuel limit enforced. |
| Bounded stack | Call stack depth limited to 1024 frames. |

---

## 3. Contract Lifecycle

```
┌──────────────────────────────────────────────────────────┐
│                  CONTRACT LIFECYCLE                       │
│                                                           │
│  1. DEPLOY                                                │
│     Client submits ContractDeploy tx                      │
│     │                                                     │
│     ▼                                                     │
│     Validate WASM module:                                 │
│       - Check magic bytes                                 │
│       - Reject float instructions                         │
│       - Reject disallowed imports                         │
│       - Verify size < max_code_size                       │
│     │                                                     │
│     ▼                                                     │
│     AOT compile WASM → native code                        │
│     Store: code_hash → compiled module                    │
│     │                                                     │
│     ▼                                                     │
│     Execute constructor (init function)                   │
│       - With gas metering                                 │
│       - Constructor may set initial storage               │
│     │                                                     │
│     ▼                                                     │
│     Create contract account:                              │
│       address = hash(deployer, nonce)                     │
│       code_hash = hash(bytecode)                          │
│                                                           │
│  2. CALL                                                  │
│     Client submits ContractCall tx                        │
│     │                                                     │
│     ▼                                                     │
│     Load compiled module by code_hash                     │
│     Instantiate WASM instance with host API               │
│     Set gas/fuel limit                                    │
│     │                                                     │
│     ▼                                                     │
│     Execute entry point function                          │
│       - Contract reads/writes state via host API          │
│       - Gas deducted per operation                        │
│     │                                                     │
│     ▼                                                     │
│     On success: commit state changes, emit events         │
│     On failure: revert state changes, charge gas          │
│                                                           │
│  3. QUERY (read-only, no state mutation)                  │
│     RPC calls contract view function                      │
│     No gas charged (or configurable query gas limit)      │
│     No state changes persisted                            │
│                                                           │
└──────────────────────────────────────────────────────────┘
```

---

## 4. Host API

The host API is the **only** interface between a contract and the outside world. It is a set of functions imported by the WASM module.

### 4.1 State Access

```
host_storage_read(key_ptr, key_len) -> (value_ptr, value_len, exists)
    Gas: 500 per call
    Read a value from this contract's storage.

host_storage_write(key_ptr, key_len, value_ptr, value_len)
    Gas: 5000 per call
    Write a value to this contract's storage.

host_storage_delete(key_ptr, key_len)
    Gas: 2500 per call (with refund)
    Delete a key from this contract's storage.

host_storage_has(key_ptr, key_len) -> bool
    Gas: 200 per call
    Check if a key exists.
```

### 4.2 Context

```
host_get_caller() -> address_ptr
    Gas: 100
    Returns the address of the immediate caller (EOA or contract).

host_get_origin() -> address_ptr
    Gas: 100
    Returns the address of the original transaction sender.

host_get_self_address() -> address_ptr
    Gas: 100
    Returns this contract's address.

host_get_block_height() -> u64
    Gas: 100
    Returns the current block height.

host_get_block_timestamp() -> u64
    Gas: 100
    Returns the block timestamp (advisory, proposer-set).

host_get_value() -> u128
    Gas: 100
    Returns the native token value sent with this call.

host_get_gas_remaining() -> u64
    Gas: 0 (free)
    Returns remaining gas.
```

### 4.3 Cross-Contract Calls

```
host_call_contract(
    address_ptr, address_len,
    input_ptr, input_len,
    value: u128,
    gas_limit: u64
) -> (success: bool, output_ptr, output_len)
    Gas: 10000 base + forwarded gas
    Call another contract synchronously.
    Creates a nested snapshot.
    If callee fails, callee state is reverted but caller continues.
```

### 4.4 Events

```
host_emit_event(
    topic_ptr, topic_len,
    data_ptr, data_len
)
    Gas: 1000 + 100 per 32 bytes of data
    Emit a structured event (included in tx receipt).
```

### 4.5 Cryptographic Utilities

```
host_sha256(data_ptr, data_len) -> hash_ptr
    Gas: 100 + 10 per 32 bytes
    Compute SHA-256 hash.

host_verify_ed25519(
    pubkey_ptr, sig_ptr, msg_ptr, msg_len
) -> bool
    Gas: 3000
    Verify an Ed25519 signature.
```

### 4.6 Abort

```
host_abort(msg_ptr, msg_len)
    Gas: 0
    Abort execution with an error message.
    Reverts all state changes for this call frame.
```

### Explicitly NOT Provided

| Capability | Reason |
|------------|--------|
| `host_random()` | Non-deterministic |
| `host_time()` (wall clock) | Non-deterministic |
| `host_http_request()` | Non-deterministic, side effects |
| `host_file_read/write()` | Non-deterministic, side effects |
| `host_sleep()` | Non-deterministic |
| `host_spawn_thread()` | Non-deterministic |

---

## 5. Gas Metering

### 5.1 Fuel Mapping

wasmtime's fuel system maps to RustBFT gas:

```
1 WASM fuel unit = 1 RustBFT gas unit

Fuel is injected at compile time into the WASM module.
Every basic block of WASM instructions consumes fuel proportional
to the number of instructions.
```

### 5.2 Gas Accounting

```
Transaction gas flow:

    gas_limit (from tx)
        │
        ├── base_cost (21,000)
        │
        ├── input_data_cost (per byte)
        │
        ├── execution_cost (WASM fuel + host API calls)
        │
        └── remaining → refunded to sender

    gas_used = gas_limit - remaining
    gas_fee = gas_used * gas_price (MVP: gas_price = 1)
```

### 5.3 Out-of-Gas Behavior

```
When fuel reaches 0:
    1. wasmtime traps with OutOfFuel error
    2. Host catches the trap
    3. All state changes for this tx are reverted
    4. Gas is fully charged (gas_used = gas_limit)
    5. Receipt records: success=false, error="out of gas"
```

---

## 6. Memory Model

### 6.1 WASM Linear Memory

- Each contract instance gets its own linear memory.
- Initial size: 1 page (64 KB).
- Maximum size: 256 pages (16 MB).
- Memory is zeroed on each invocation (no persistent memory between calls).

### 6.2 Data Passing

Data is passed between host and contract via the WASM linear memory:

```
Host → Contract:
    1. Host writes data to a region in WASM memory.
    2. Host passes (ptr, len) to the contract function.

Contract → Host:
    1. Contract writes data to its own memory.
    2. Contract passes (ptr, len) to the host function.
    3. Host reads from WASM memory.
```

### 6.3 Allocator

Contracts must include their own allocator (e.g., `wee_alloc` for Rust contracts). The host does not manage contract heap memory.

---

## 7. Contract-to-Contract Calls

```
Contract A calls Contract B:

    ┌───────────────────────────────────────────────┐
    │ Contract A execution                          │
    │                                               │
    │   ... some logic ...                          │
    │                                               │
    │   host_call_contract(B, input, value, gas)    │
    │       │                                       │
    │       │  ┌─────────────────────────────────┐  │
    │       └─▶│ Snapshot state                  │  │
    │          │ Instantiate Contract B           │  │
    │          │ Execute B with forwarded gas     │  │
    │          │                                  │  │
    │          │ IF success:                      │  │
    │          │   Commit B's state changes       │  │
    │          │   Return output to A             │  │
    │          │                                  │  │
    │          │ IF failure:                      │  │
    │          │   Revert B's state changes       │  │
    │          │   Return error to A              │  │
    │          │   A continues (can handle error) │  │
    │          └─────────────────────────────────┘  │
    │                                               │
    │   ... A continues with result ...             │
    │                                               │
    └───────────────────────────────────────────────┘
```

### Call Depth Limit

- Maximum call depth: 64 nested calls.
- Exceeding the limit causes the current call to fail (revert).
- This prevents stack overflow attacks.

### Reentrancy

- Reentrancy is **allowed** (contract A calls B, B calls A).
- Contracts must protect themselves against reentrancy (e.g., mutex pattern in storage).
- The host does not enforce reentrancy guards — this is the contract developer's responsibility.
- **Rationale:** Prohibiting reentrancy at the host level adds complexity and limits legitimate use cases. The MVP prioritizes simplicity.

---

## 8. Module Validation (Deploy-Time)

Before a WASM module is accepted, it undergoes validation:

```
Validation checklist:
    [1] Valid WASM binary (magic bytes, version)
    [2] No floating-point instructions (f32.*, f64.*)
    [3] No SIMD instructions
    [4] No bulk memory operations (MVP restriction)
    [5] All imports are from the "rustbft" namespace
    [6] All imported functions match the host API signature
    [7] Exports an "init" function (for deploy) or callable functions
    [8] Code size ≤ max_code_size (default: 512 KB)
    [9] Memory declaration ≤ max_memory_pages (default: 256)
    [10] No start function (execution begins only via explicit call)
```

If any check fails, the deploy transaction fails (state reverted, gas charged).

---

## 9. Contract ABI

### 9.1 Function Dispatch

Contracts export a single entry point:

```
export fn call(input_ptr: u32, input_len: u32) -> u32
```

The input is an ABI-encoded function selector + arguments. The contract is responsible for dispatching to the correct internal function.

### 9.2 ABI Encoding

MVP uses a simple ABI:

```
┌──────────────────┬───────────────────────────────┐
│ Selector (4 B)   │ Arguments (variable)          │
└──────────────────┴───────────────────────────────┘

Selector = first 4 bytes of sha256(function_signature)
Arguments = concatenated, length-prefixed encoded values
```

### 9.3 Return Values

```
Contract writes return data to memory and returns a pointer
to a (ptr, len) pair. Host reads the return data.

Return format:
    ┌──────────────┬──────────────┬──────────────┐
    │ status (1 B) │ data_len (4) │ data (var)   │
    └──────────────┴──────────────┴──────────────┘
    status: 0 = success, 1 = revert, 2 = error
```

---

## 10. Determinism Enforcement Summary

```
┌─────────────────────────────────────────────────────────┐
│              DETERMINISM ENFORCEMENT                     │
│                                                          │
│  Compile-time:                                           │
│    ✓ No float instructions in WASM                       │
│    ✓ No SIMD                                             │
│    ✓ No threads                                          │
│    ✓ Only approved host imports                          │
│                                                          │
│  Runtime:                                                │
│    ✓ Fuel metering (deterministic gas)                   │
│    ✓ Bounded memory                                      │
│    ✓ Bounded call depth                                  │
│    ✓ No host access beyond defined API                   │
│    ✓ Snapshot/revert for failure isolation                │
│    ✓ Sequential execution (no parallelism)               │
│                                                          │
│  Host API:                                               │
│    ✓ No randomness                                       │
│    ✓ No wall-clock time                                  │
│    ✓ No network/filesystem                               │
│    ✓ Block timestamp is proposer-set, not clock-derived  │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

---

## 11. Error Taxonomy

| Error | Source | Recovery |
|-------|--------|----------|
| OutOfFuel | WASM execution exceeded gas | Revert tx, charge full gas |
| Trap (unreachable) | WASM hit unreachable instruction | Revert tx, charge gas used |
| HostError | Host API call failed (e.g., invalid key) | Propagated to contract as error return |
| StackOverflow | Call depth exceeded | Revert current call frame |
| MemoryAccessViolation | Out-of-bounds memory access | Revert tx, charge gas used |
| InvalidModule | Deploy-time validation failure | Revert deploy tx |
| ABIDecodeError | Malformed input data | Contract returns error (not host's concern) |

---

## Definition of Done — Smart Contracts

- [x] WASM runtime choice justified (wasmtime)
- [x] Contract lifecycle (deploy, call, query) specified
- [x] Complete host API with gas costs
- [x] Gas metering via fuel mapping
- [x] Memory model and data passing defined
- [x] Contract-to-contract calls with snapshot semantics
- [x] Call depth and reentrancy policy stated
- [x] Module validation checklist
- [x] ABI encoding format defined
- [x] Determinism enforcement summarized
- [x] Error taxonomy with recovery actions
- [x] No Rust source code included
