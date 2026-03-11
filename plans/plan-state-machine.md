I# Feature: State Machine & Block Execution

## 1. Purpose

The State Machine is the deterministic execution engine that takes a committed block from consensus and produces a new world state. It runs the `BeginBlock → DeliverTx(×N) → EndBlock → Commit` pipeline, updates account balances and nonces, computes a Merkle state root, and collects validator set updates for the next height.

Determinism is the single most important property of this module. Every honest node that executes the same block sequence must arrive at exactly the same state root. Any divergence is a critical bug that halts the node.

## 2. Responsibilities

- Execute the `BeginBlock → DeliverTx → EndBlock → Commit` pipeline for each committed block
- Maintain account state: `address → Account{balance, nonce, code_hash, storage_root}`
- Validate transactions statefully during `DeliverTx`: nonce ordering, balance sufficiency, gas limit
- Apply state changes on success; revert per-transaction changes on failure (snapshot/revert pattern)
- Still charge gas on failed transactions and include failed txs in the block
- Execute `Transfer`, `ContractDeploy`, `ContractCall` transactions
- Collect `ValidatorUpdate` transactions during `DeliverTx` and apply them in `EndBlock`
- Compute the new state root by updating the sparse Merkle trie and hashing the root
- Load genesis state on first startup: insert initial accounts and validators
- Return `BlockExecutionResult` to the consensus core via channel

## 3. Non-Responsibilities

- Does not initiate consensus decisions
- Does not access the network
- Does not use wall-clock time, randomness, or any non-deterministic source
- Does not perform async operations — fully synchronous
- Does not persist state itself — hands changesets to the storage layer
- Does not execute WASM contract logic directly — delegates to the Contracts module

## 4. Architecture Design

```
Consensus Core Thread
        |
   ExecuteBlock(block)
        |
+-------v-----------------------------------------+
|   State Machine Thread (sync)                    |
|                                                  |
|  BeginBlock(header)                              |
|     → process evidence                           |
|     → set block context (height, timestamp)      |
|                                                  |
|  DeliverTx(tx) × N                               |
|     → stateful validate (nonce, balance, gas)    |
|     → snapshot state                             |
|     → execute tx (Transfer / Deploy / Call)      |
|     → on success: apply changes                  |
|     → on failure: revert to snapshot             |
|     → charge gas always                          |
|     → emit Receipt                               |
|                                                  |
|  EndBlock(header)                                |
|     → apply collected ValidatorUpdates          |
|     → validate safety (quorum preserved)         |
|                                                  |
|  Commit                                          |
|     → compute state_root = merkle_root(AppState) |
|     → return BlockExecutionResult               |
+-------+-----------------------------------------+
        |
   BlockExecutionResult{state_root, receipts, validator_updates}
        |
  Consensus Core (sends PersistBlock, advances height)
```

## 5. Core Data Structures (Rust)

```rust
// src/state/mod.rs

pub struct Account {
    pub address: Address,
    pub balance: u128,      // no floats; integer token units
    pub nonce: u64,         // monotonically increasing; prevents replay
    pub code_hash: Option<Hash>,       // None for EOA
    pub storage_root: Hash,            // empty hash for EOA
}

pub struct AppState {
    // All maps use BTreeMap for deterministic iteration — never HashMap
    pub accounts: BTreeMap<Address, Account>,
    pub validator_set: ValidatorSet,
    pub chain_params: ChainParams,
}

pub struct BlockContext {
    pub height: u64,
    pub timestamp: u64,
    pub proposer: ValidatorId,
}

// src/state/executor.rs

pub struct StateExecutor {
    state: AppState,
    contract_engine: Box<dyn ContractEngine>,
    pending_validator_updates: Vec<ValidatorUpdate>,
    block_ctx: Option<BlockContext>,
}

pub struct BlockExecutionResult {
    pub height: u64,
    pub state_root: Hash,
    pub tx_receipts: Vec<Receipt>,
    pub validator_updates: Vec<ValidatorUpdate>,  // to apply at height+1
    pub gas_used: u64,
}

// src/state/merkle.rs — sparse Merkle trie for state root

pub struct MerkleTree {
    // key: sha256(address) or sha256(address || storage_key) → 32 bytes
    // Stored as BTreeMap in memory; flushed to storage on Commit
    nodes: BTreeMap<Hash, MerkleNode>,
    root: Hash,
}

// src/state/accounts.rs — snapshot/revert

pub struct StateSnapshot {
    pub depth: u32,
    pub changes: Vec<StateChange>,
}

pub struct StateChange {
    pub key: Bytes,
    pub old_value: Option<Bytes>,
    pub new_value: Option<Bytes>,
}
```

## 6. Public Interfaces

```rust
// src/state/mod.rs

pub trait ContractEngine: Send {
    fn deploy(
        &mut self,
        deployer: Address,
        code: &[u8],
        init_args: &[u8],
        gas_limit: u64,
        state: &mut AppState,
    ) -> Result<(Address, u64), ContractError>;  // (contract_address, gas_used)

    fn call(
        &mut self,
        caller: Address,
        contract: Address,
        input: &[u8],
        value: u128,
        gas_limit: u64,
        state: &mut AppState,
    ) -> Result<(Vec<u8>, u64, Vec<Event>), ContractError>;  // (output, gas_used, events)
}

// Main execution pipeline
impl StateExecutor {
    pub fn new(state: AppState, contract_engine: Box<dyn ContractEngine>) -> Self;

    pub fn begin_block(&mut self, header: &BlockHeader);
    pub fn deliver_tx(&mut self, tx: &SignedTransaction) -> Receipt;
    pub fn end_block(&mut self, header: &BlockHeader) -> Result<Vec<ValidatorUpdate>, StateError>;
    pub fn commit(&mut self) -> BlockExecutionResult;

    pub fn load_genesis(cfg: &GenesisConfig) -> Result<AppState, StateError>;
    pub fn state_root(&self) -> Hash;
    pub fn get_account(&self, address: &Address) -> Option<&Account>;
}
```

## 7. Internal Algorithms

### Gas Cost Table
```
Base transaction cost:         21_000 gas
Transfer (additional):              0 gas
Contract deploy (per byte):       200 gas
Contract call (base):          10_000 gas
State read (per key):             500 gas
State write (per key):          5_000 gas
State delete (per key):        -2_500 gas (refund)
SHA-256 per 32 bytes:             100 gas
Signature verify:               3_000 gas
```

### DeliverTx (Transfer)
```
fn deliver_transfer(tx, state):
    snapshot = state.snapshot()
    // Stateful validation
    from_account = state.accounts.get(tx.from)?       else revert(snapshot, "no account")
    if from_account.nonce != tx.nonce:                   revert(snapshot, "bad nonce")
    gas_cost = 21_000
    total_cost = tx.amount + gas_cost  // in token units; gas_price=1 for MVP
    if from_account.balance < total_cost:                revert(snapshot, "insufficient balance")
    // Apply
    state.accounts[tx.from].balance -= total_cost
    state.accounts[tx.to].balance   += tx.amount
    state.accounts[tx.from].nonce   += 1
    Ok(Receipt { success: true, gas_used: gas_cost, logs: [] })
```

### Failure Semantics
```
fn execute_tx(tx, state):
    snapshot = state.snapshot()
    gas_charge = min(gas_limit, base_cost)
    result = try_execute(tx, state)
    match result:
        Ok(receipt):
            // State changes already applied
            return receipt
        Err(e):
            state.revert_to(snapshot)
            // Still charge gas: deduct gas_charge from sender
            // This must succeed (validated above that balance ≥ base_cost)
            state.accounts[tx.from].balance -= gas_charge
            state.accounts[tx.from].nonce   += 1
            return Receipt { success: false, gas_used: gas_charge, error: e }
```

### EndBlock: Validator Updates
```
fn end_block(updates, state):
    if updates is empty: return Ok([])
    new_set = apply_updates(state.validator_set, updates)
    // Safety checks
    assert!(new_set.total_power > 0)
    power_change = sum(|old_power - new_power| for each changed validator)
    assert!(power_change <= state.validator_set.total_power / 3)
    Ok(new_set changes)  // applied to state at height+1
```

### Commit: State Root Computation
```
fn commit(state):
    // Update Merkle trie with all modified accounts
    for (addr, account) in state.dirty_accounts:
        key   = sha256(addr)
        value = account.canonical_encode()
        merkle_tree.update(key, value)
    for (addr, key, val) in state.dirty_contract_storage:
        mkey  = sha256(addr || key)
        merkle_tree.update(mkey, val)
    state_root = merkle_tree.root()
    return state_root
```

### Determinism Invariants
| Risk | Mitigation |
|------|------------|
| Map iteration | `BTreeMap` everywhere; no `HashMap` |
| Floating-point | Prohibited; all arithmetic is `u64`/`u128` with checked ops |
| Overflow | All arithmetic uses `checked_add` / `checked_sub`; overflow is a deterministic error |
| Timestamp | Block timestamp is advisory; not used in state transition logic |
| Randomness | Prohibited; no `rand` crate in this module |
| Serialization | Canonical binary encoding; same bytes on every platform |

## 8. Persistence Model

The state machine does not write to disk itself. After `Commit`, it returns the `BlockExecutionResult` (including the changeset) to the node binary, which routes it to the storage thread via a `PersistBlock` command. The state machine keeps the latest in-memory `AppState` for the next block execution.

## 9. Concurrency Model

The state machine runs on a single dedicated OS thread. It receives `ExecuteBlock` commands via a `crossbeam::channel::Receiver` and sends `BlockExecutionResult` back via a `oneshot::Sender` included in the command. No async code, no `Mutex`, no `Arc`.

The `ContractEngine` is owned exclusively by the state machine thread — it is `Box<dyn ContractEngine + Send>` but only ever accessed from one thread.

## 10. Configuration

No separate configuration section. Uses `chain_params` from genesis:
```toml
[chain_params]  # in genesis.json
max_block_gas = 10000000
max_block_bytes = 1048576
max_tx_bytes = 65536
```

## 11. Observability

- `rustbft_state_block_execution_duration_seconds` (Histogram)
- `rustbft_state_tx_execution_duration_seconds{tx_type}` (Histogram)
- `rustbft_state_tx_success_total` (Counter)
- `rustbft_state_tx_failure_total{error_type}` (Counter)
- `rustbft_state_gas_used_total` (Counter)
- `rustbft_state_state_root_computation_seconds` (Histogram)
- Log INFO for each committed block (height, state_root, tx_count, gas_used); ERROR for state root mismatch → HALT node

## 12. Testing Strategy

- **`test_genesis_load`**: load genesis config, verify initial accounts have correct balances, validator set matches
- **`test_transfer_success`**: deliver a valid transfer, assert sender balance decreases, recipient increases, nonce increments
- **`test_transfer_insufficient_balance`**: balance < amount + gas → receipt `success=false`, state reverted, gas charged
- **`test_transfer_bad_nonce`**: wrong nonce → receipt `success=false`, state reverted
- **`test_failed_tx_still_in_block`**: failed tx included in block; gas deducted from sender
- **`test_gas_exhaustion`**: tx with gas_limit < base_cost → fails, gas charged
- **`test_state_root_deterministic`**: same block executed twice produces same state root
- **`test_state_root_changes_on_different_tx`**: block A vs block B → different state roots
- **`test_snapshot_revert`**: take snapshot, modify state, revert → state identical to snapshot
- **`test_validator_update_applied`**: add validator in EndBlock → validator in set at next height
- **`test_validator_update_quorum_violated`**: update that reduces quorum → rejected as batch
- **`test_no_floats_in_gas_math`**: property test: gas computed with only integer arithmetic
- **`test_btreemap_iteration_order`**: same account set → same iteration order → same state root

## 13. Open Questions

None.
