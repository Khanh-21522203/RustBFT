# State Machine

## Purpose

Executes committed blocks deterministically against the account-based world state, computing a new state root after each block. Handles four transaction types and validator set updates via EndBlock.

## Scope

**In scope:**
- `AppState` — in-memory world state with snapshot/revert for atomic tx isolation
- `StateExecutor::execute_block` — BeginBlock / DeliverTx loop / EndBlock / state root computation
- Gas metering: intrinsic costs + VM gas for contract transactions
- State root computation via flat SHA256 hash over sorted accounts, code, and storage

**Out of scope:**
- Block validation at consensus layer (done in `ConsensusCore::validate_block` callback)
- Persistent storage (done by `StateStore` via `CommandRouter`)
- Mempool ordering (not implemented — blocks always arrive pre-ordered)
- Merkle proofs (state root is a flat hash, not a tree)

## Primary User Flow

1. `CommandRouter` receives `ConsensusCommand::ExecuteBlock { block }`.
2. Calls `StateExecutor::execute_block(&mut app_state, &mut contracts, &block)`.
3. For each tx in `block.txs`: decode, validate nonce/balance, execute, charge gas.
4. EndBlock: process any pending `ValidatorUpdateTx` entries.
5. Compute `state_root = compute_state_root(&app_state)`.
6. Return `(state_root, validator_updates)` → sent back as `ConsensusEvent::BlockExecuted`.

## System Flow

```
CommandRouter::ExecuteBlock
    │
    ▼
StateExecutor::execute_block (src/state/executor.rs)
    │
    ├── begin_block (no-op in MVP)
    │
    ├── [for each tx in block.txs]
    │       decode_tx (src/state/tx.rs) → DecodedTx enum
    │       ├── Transfer   → exec_transfer
    │       │       snapshot → check nonce + balance → transfer → commit/revert
    │       │       charge intrinsic gas (base_tx + sig_verify = 24,000)
    │       ├── ContractDeploy → exec_deploy
    │       │       snapshot → validate_wasm → ContractRuntime::deploy
    │       │       → address_from_sender_nonce → set_account(code_hash)
    │       │       → ContractRuntime::call (constructor if init_input non-empty)
    │       │       → commit/revert → charge gas
    │       ├── ContractCall → exec_call
    │       │       snapshot → transfer value → ContractRuntime::call
    │       │       → commit/revert → charge gas
    │       └── ValidatorUpdate → collect in pending_val_updates, charge intrinsic
    │
    ├── end_block → convert pending_val_updates to Vec<ValidatorUpdate>
    │
    └── compute_state_root (src/state/merkle.rs)
            sha256(sorted accounts || sorted code || sorted storage)
```

## Data Model

`Account` (`src/state/accounts.rs`):
- `address: Address ([u8; 20])`, `balance: u128`, `nonce: u64`
- `code_hash: Option<Hash>` — set if account has deployed contract code
- `storage_root: Hash` — placeholder, not derived from contract storage in MVP

`AppState` (`src/state/accounts.rs`):
- `accounts: BTreeMap<Address, Account>` — sorted for determinism
- `contract_code: BTreeMap<Hash, Vec<u8>>` — code hash → wasm bytes
- `contract_storage: BTreeMap<(Address, Vec<u8>), Vec<u8>>` — per-contract key/value
- `params: ChainParams { max_block_gas: 10M, max_tx_bytes: 64KB, max_block_bytes: 1MB }`
- `stack: Vec<Vec<StateChange>>` — snapshot/revert journal (not serialized)

`AppState` snapshot/revert (`src/state/accounts.rs`):
- `snapshot()` → pushes new `Vec<StateChange>` frame, returns `SnapshotId(frame_index)`
- `revert_to(id)` → pops frames and applies reverse changes
- `commit_snapshot(id)` → merges top frame into parent

`GasSchedule` (`src/state/executor.rs`):
- `base_tx: 21_000`, `sig_verify: 3_000`, `deploy_per_byte: 200`, `call_base: 10_000`
- Intrinsic for Transfer = 24,000; Deploy = 24,000 + wasm_bytes × 200; Call = 34,000

Transaction wire format (`src/state/tx.rs`): big-endian binary, leading type byte:
- `0x01` Transfer: `from(20) + to(20) + amount(16) + nonce(8) + gas_limit(8) + sig(64)`
- `0x02` ContractDeploy: `from(20) + nonce(8) + gas_limit(8) + wasm(u32-len-prefix) + init_input(u32-len-prefix) + sig(64)`
- `0x03` ContractCall: `from(20) + to(20) + nonce(8) + gas_limit(8) + value(16) + input(u32-len-prefix) + sig(64)`
- `0x04` ValidatorUpdate: `from(20) + action(1) + validator_id(32) + new_power(8) + nonce(8) + gas_limit(8) + sig(64)`

`compute_state_root` (`src/state/merkle.rs`):
- Iterates sorted `accounts`, `contract_code`, `contract_storage` in BTreeMap order
- Concatenates: `"acct"||addr||balance||nonce`, `"code"||hash||len||bytes`, `"stor"||addr||key_len||key||val_len||val`
- Returns `sha256(all_bytes)` — flat hash, not a Merkle tree

## Interfaces and Contracts

`StateExecutor::execute_block(state, contracts, block) -> Result<(Hash, Vec<ValidatorUpdate>), anyhow::Error>`:
- Returns `Err` if block exceeds `max_block_bytes` or `max_block_gas`
- Individual tx failures are non-fatal: failed tx charges gas and moves on
- Always deterministic: same block + same state → same result

`AppState::get_account(addr) -> Account`:
- Returns `Account::empty(addr)` (zero balance/nonce, no code) if address not found

Contract address derivation:
- `sha256(sender_bytes || nonce_be_bytes)` first 20 bytes → deterministic, no CREATE2

## Dependencies

**Internal modules:**
- `src/contracts/runtime.rs` — `ContractRuntime::deploy` / `call` for WASM execution
- `src/state/merkle.rs` — `compute_state_root`
- `src/state/tx.rs` — `decode_tx`
- `src/crypto/hash.rs` — `sha256` for address derivation and state root

**External crates:**
- `anyhow` — error propagation in `execute_block`

## Failure Modes and Edge Cases

- **Bad nonce:** `ExecError::BadNonce` — reverts snapshot, charges intrinsic gas, tx counted as failed (gas consumed, nonce NOT incremented).
- **Insufficient balance:** `ExecError::InsufficientBalance` — reverts snapshot, charges intrinsic gas.
- **WASM validation failure on deploy:** `ExecError::Contract` — reverts, charges full `gas_limit`.
- **Contract out of gas:** `ContractError::OutOfGas` — reverts, charges full `gas_limit`.
- **Contract trap:** `ContractError::Trap` — reverts, charges intrinsic gas.
- **tx decode error:** `TxDecodeError` — entire tx skipped (no gas charged); no receipt produced.
- **Block gas limit exceeded:** Returns `Err` from `execute_block` — entire block execution fails; CommandRouter logs error but consensus does not retry or halt.
- **Oversized tx:** Skipped silently (not decoded) if `tx.len() > max_tx_bytes`.
- **`storage_root` field in Account:** Never actually computed from contract storage — always `Hash([0; 32])` placeholder.

## Observability and Debugging

- `Metrics.state_block_execution_duration` histogram updated by `CommandRouter` per block.
- `Metrics.state_tx_success/failure/gas_used` — registered but not updated in MVP.
- Block execution errors logged via `tracing::error!` in `CommandRouter`.
- Debugging starting point: `StateExecutor::execute_block` at `src/state/executor.rs:execute_block`.

## Risks and Notes

- Transaction signatures are never verified in `StateExecutor` — anyone can forge a tx from any sender.
- `storage_root` in `Account` is always zero — Merkle proofs of contract storage are not possible.
- `compute_state_root` is O(N) over all state entries per block — no incremental Merkle tree.
- Validator update txs are not signature-verified or access-controlled — any address can submit them.
- `decode_account` / `encode_account` are internal binary formats not tied to the serde JSON used in `StateStore` — a round-trip through `StateStore` goes through JSON which is separate from the in-memory binary codec.

Changes:

