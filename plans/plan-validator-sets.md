# Feature: Validator Set Management

## 1. Purpose

The Validator Set module manages the lifecycle of validator identities, voting power, and safe transitions between one validator set and the next. It enforces the invariant that all honest nodes agree on the same validator set at every height, and that no update can reduce total voting power below the quorum threshold or change more than one-third of voting power in a single block.

## 2. Responsibilities

- Represent the active validator set as a `BTreeMap<ValidatorId, Validator>` for deterministic iteration
- Compute the `validator_set_hash` included in every `BlockHeader`
- Process `ValidatorUpdate` transactions in `EndBlock`: add, remove, and adjust voting power
- Enforce safety rules: quorum preservation, maximum power change per height (≤ total/3), no duplicate IDs, all voting power > 0
- Gate validator updates to admin-configured addresses only
- Apply the updated set at height H+1 (never mid-height)
- Reset proposer priority state when the validator set changes
- Store historical validator sets keyed by height for light-client and evidence processing
- Reject the update batch as a unit if any safety rule fails; the block itself remains valid

## 3. Non-Responsibilities

- Does not initiate consensus decisions
- Does not participate in block gossip
- Does not implement on-chain governance beyond the MVP admin address model
- Does not manage account balances or staking rewards
- Does not perform block sync or light client verification
- Does not persist data directly — hands changesets to the storage layer

## 4. Architecture Design

```
Block H execution
        |
  DeliverTx (ValidatorUpdate tx)
        |  → collected in pending_updates; not applied yet
        |
  EndBlock(H)
        |
+-------v---------------------------------------------+
|   ValidatorSetManager (src/state/validator_set.rs)   |
|                                                      |
|  apply_updates(current_set, pending_updates):        |
|    1. Authorize each update (admin address check)    |
|    2. Build candidate new_set from current_set       |
|    3. Compute sum_of_power_changes                   |
|    4. Check quorum rules                             |
|    5. On success: return new ValidatorSet(H+1)       |
|    6. On failure: return Err; block is still valid   |
|                                                      |
+-------+---------------------------------------------+
        |
  Commit
        |
  Store ValidatorSet(H+1) → storage
  Reset ProposerState for H+1
```

## 5. Core Data Structures (Rust)

```rust
// src/state/validator_set.rs

pub struct Validator {
    pub id: ValidatorId,
    pub public_key: Ed25519PublicKey,
    pub voting_power: u64,
    pub address: Address,
}

pub struct ValidatorSet {
    // BTreeMap for deterministic iteration — never HashMap
    pub validators: BTreeMap<ValidatorId, Validator>,
    pub total_power: u64,
}

pub enum ValidatorAction {
    Add,
    Remove,
    UpdatePower,
}

pub struct ValidatorUpdate {
    pub action: ValidatorAction,
    pub validator_id: ValidatorId,
    pub public_key: Option<Ed25519PublicKey>,   // required for Add
    pub new_power: Option<u64>,                  // required for Add and UpdatePower
}

pub enum ValidatorSetError {
    Unauthorized { signer: Address },
    DuplicateValidatorId { id: ValidatorId },
    ValidatorNotFound { id: ValidatorId },
    ValidatorAlreadyExists { id: ValidatorId },
    ZeroVotingPower { id: ValidatorId },
    TotalPowerOverflow,
    ZeroTotalPower,
    PowerChangeTooLarge { change: u64, limit: u64 },
    MissingPublicKey,
}
```

## 6. Public Interfaces

```rust
// src/state/validator_set.rs

impl ValidatorSet {
    pub fn new(validators: Vec<Validator>) -> Result<Self, ValidatorSetError>;

    /// SHA-256 of the canonical serialization of (id, voting_power) pairs.
    /// Included in every BlockHeader.
    pub fn hash(&self) -> Hash;

    pub fn get(&self, id: &ValidatorId) -> Option<&Validator>;
    pub fn contains(&self, id: &ValidatorId) -> bool;
    pub fn quorum_threshold(&self) -> u64;  // (total_power * 2 / 3) + 1

    /// Apply a batch of updates. Returns a new ValidatorSet if all safety
    /// rules pass, or Err if the batch is rejected. The old set is unchanged.
    pub fn apply_updates(
        &self,
        updates: &[ValidatorUpdate],
        admin_addresses: &[Address],
        tx_signers: &[Address],
    ) -> Result<ValidatorSet, ValidatorSetError>;
}

// Proposer priority reset when the set changes
pub fn reset_proposer_state(set: &ValidatorSet) -> ProposerState;

// Storage keys
pub const KEY_VALIDATOR_SET_CURRENT: &str = "validator_set/current";
pub const KEY_VALIDATOR_SET_NEXT: &str    = "validator_set/next";
pub fn key_validator_set_at(height: u64) -> String;  // "validator_set/height/{H}"
```

## 7. Internal Algorithms

### ValidatorSetHash
```
fn hash(set: &ValidatorSet) -> Hash:
    // Serialize sorted (id, voting_power) pairs canonically
    let mut buf = Vec::new()
    for (id, v) in set.validators.iter():   // BTreeMap: deterministic order
        buf.extend_from_slice(id.as_bytes())
        buf.extend_from_slice(&v.voting_power.to_be_bytes())
    sha256(&buf)
```

### apply_updates
```
fn apply_updates(current, updates, admin_addrs, tx_signers):
    // 1. Authorization check
    for (update, signer) in updates.iter().zip(tx_signers):
        if !admin_addrs.contains(signer):
            return Err(Unauthorized)

    // 2. Build candidate set (process updates in order; last wins on conflict)
    let mut candidate = current.validators.clone()
    for update in updates:
        match update.action:
            Add:
                if candidate.contains_key(&update.validator_id):
                    return Err(ValidatorAlreadyExists)
                if update.public_key.is_none():
                    return Err(MissingPublicKey)
                if update.new_power.unwrap_or(0) == 0:
                    return Err(ZeroVotingPower)
                candidate.insert(update.validator_id, Validator { ... })
            Remove:
                if !candidate.contains_key(&update.validator_id):
                    return Err(ValidatorNotFound)
                candidate.remove(&update.validator_id)
            UpdatePower:
                if !candidate.contains_key(&update.validator_id):
                    return Err(ValidatorNotFound)
                if update.new_power.unwrap_or(0) == 0:
                    return Err(ZeroVotingPower)
                candidate[update.validator_id].voting_power = update.new_power.unwrap()

    // 3. Compute new total power (checked arithmetic)
    let new_total = candidate.values().map(|v| v.voting_power).checked_sum()?
        .ok_or(TotalPowerOverflow)?
    if new_total == 0: return Err(ZeroTotalPower)

    // 4. Power change limit: |changes| ≤ current.total_power / 3
    let power_change = compute_power_delta(current.validators, candidate)
    let limit = current.total_power / 3
    if power_change > limit:
        return Err(PowerChangeTooLarge { change: power_change, limit })

    Ok(ValidatorSet { validators: candidate, total_power: new_total })
```

### Power Delta Computation
```
fn compute_power_delta(old, new) -> u64:
    delta = 0
    for id in old.keys() ∪ new.keys():
        old_p = old.get(id).map(|v| v.voting_power).unwrap_or(0)
        new_p = new.get(id).map(|v| v.voting_power).unwrap_or(0)
        delta += (old_p as i128 - new_p as i128).unsigned_abs() as u64
    delta
```

### Quorum Threshold
```
fn quorum_threshold(total_power: u64) -> u64:
    (total_power * 2 / 3) + 1    // integer arithmetic only — no floats
```

## 8. Persistence Model

The current validator set is stored in the state store under `"validator_set/current"`. During `EndBlock`, if updates produce a new set, it is staged under `"validator_set/next"`. During `Commit`, the new set is promoted to `"validator_set/current"` and a snapshot is written to `"validator_set/height/{H}"`. Historical snapshots are retained for `pruning_window` heights (default: 1000) to support equivocation evidence and light client verification. The state machine does not write directly — it returns the changeset to the node binary which routes it to the storage thread.

## 9. Concurrency Model

`ValidatorSetManager` has no async code and no shared mutable state. It is called synchronously from the state machine thread during `EndBlock`. The resulting `ValidatorSet` for height H+1 is included in the `BlockExecutionResult` and forwarded to the consensus core thread via channel before the next height begins. The consensus core loads it with `state.validator_set = new_set` at height boundary. No `Arc`, no `Mutex`.

## 10. Configuration

```toml
[chain_params]  # in genesis.json
admin_addresses = ["0xalice..."]  # Addresses authorized to submit validator updates

[storage]
validator_history_window = 1000  # Heights of validator set history to retain
```

## 11. Observability

- `rustbft_state_validator_set_size` (Gauge) — number of active validators
- `rustbft_state_validator_set_total_power` (Gauge) — total voting power
- `rustbft_state_validator_updates_total{action}` (Counter) — add/remove/update-power operations applied
- `rustbft_state_validator_update_rejections_total{reason}` (Counter) — rejected update batches by error type
- Log INFO `"Validator set updated"` with fields `{height, added, removed, updated, new_total_power}`
- Log WARN when an update batch is rejected `"Validator update batch rejected"` with `{reason, height}`
- Log ERROR and HALT on quorum underflow that passes validation (should never happen)

## 12. Testing Strategy

- **`test_validator_set_hash_deterministic`**: same set of validators always produces the same hash regardless of insertion order
- **`test_add_validator_success`**: add a new validator with valid args and admin signer → new set includes validator
- **`test_add_validator_already_exists`**: add a validator that is already in the set → `Err(ValidatorAlreadyExists)`
- **`test_remove_validator_success`**: remove an existing validator → set shrinks, total_power decreases
- **`test_remove_validator_not_found`**: remove a validator not in the set → `Err(ValidatorNotFound)`
- **`test_remove_last_validator_rejected`**: attempt to remove the only validator → `Err(ZeroTotalPower)`
- **`test_update_power_success`**: change voting power of existing validator → new power reflected
- **`test_zero_power_rejected`**: set validator power to 0 → `Err(ZeroVotingPower)`
- **`test_power_change_exceeds_limit`**: change > total/3 in one block → `Err(PowerChangeTooLarge)`
- **`test_unauthorized_update_rejected`**: update submitted by non-admin address → `Err(Unauthorized)`
- **`test_quorum_threshold_integer_math`**: assert (267*2/3)+1 = 179; (300*2/3)+1 = 201; no floats
- **`test_batch_rejected_atomically`**: batch with one valid and one invalid update → entire batch rejected, set unchanged
- **`test_conflicting_updates_last_wins`**: add then remove same validator in one batch → validator removed
- **`test_self_removal`**: validator submits transaction to remove itself → takes effect at H+1, participates at H
- **`test_apply_updates_produces_new_set`**: verify apply_updates does not mutate the original set (immutable transition)

## 13. Open Questions

- **Governance beyond admin addresses**: For production, validator updates should require on-chain governance (multisig or DAO vote). MVP uses a flat admin address list. Future work.
- **Validator history pruning**: Pruning old validator sets after `history_window` heights removes the ability to verify ancient equivocation evidence. The window may need tuning based on evidence expiry policy.
