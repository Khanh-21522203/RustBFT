# RustBFT — Dynamic Validator Sets

**Purpose:** Define how validator sets are managed, updated, and transitioned safely within the replicated state machine.
**Audience:** Engineers implementing validator set logic and consensus integration.

---

## 1. Core Invariants

These invariants are **non-negotiable**. Any design change that violates them MUST be rejected.

| # | Invariant |
|---|-----------|
| V1 | The validator set is part of the replicated state. All honest nodes agree on the validator set at every height. |
| V2 | Validator set updates decided at height H take effect at height H+1. |
| V3 | The validator set is **fixed** for the entire duration of a height. No mid-height changes. |
| V4 | No update may reduce total voting power below the minimum quorum threshold. |
| V5 | The validator set hash is included in every block header for verification. |

---

## 2. Validator Data Model

### 2.1 Validator

```
Validator:
    id:           ValidatorId       # Unique identifier (derived from public key)
    public_key:   Ed25519PublicKey
    voting_power: u64               # Must be > 0 for active validators
    address:      Address           # On-chain address for rewards/admin (future)
```

### 2.2 ValidatorSet

```
ValidatorSet:
    validators:      BTreeMap<ValidatorId, Validator>   # Ordered for determinism
    total_power:     u64                                 # Sum of all voting_power
    proposer_state:  ProposerState                       # For weighted round-robin
```

**BTreeMap** is used (not HashMap) to ensure deterministic iteration order.

### 2.3 ValidatorSetHash

```
validator_set_hash = sha256(
    canonical_serialize(
        sorted_validators.map(|v| (v.id, v.voting_power))
    )
)
```

This hash is included in every block header. Nodes verify that the validator set they are using matches the hash in the block.

---

## 3. Validator Update Operations

### 3.1 Supported Operations

| Operation | Description |
|-----------|-------------|
| **Add** | Add a new validator with a specified voting power. |
| **Remove** | Remove an existing validator. |
| **UpdatePower** | Change an existing validator's voting power. |

### 3.2 ValidatorUpdate Structure

```
ValidatorUpdate:
    action:     ValidatorAction     # Add | Remove | UpdatePower
    validator_id: ValidatorId
    public_key:   Option<Ed25519PublicKey>   # Required for Add
    new_power:    Option<u64>                # Required for Add and UpdatePower
```

### 3.3 Update Submission

Validator updates are submitted as **special transactions** (type: `ValidatorUpdate`). They are:

1. Included in a block by the proposer (like any other transaction).
2. **Not** executed during `DeliverTx`. They are collected and deferred.
3. Processed during `EndBlock`, after all regular transactions.
4. Applied to the state, taking effect at the **next** height.

```
Block H execution:
    BeginBlock(H)
    DeliverTx(tx1)      # Regular tx
    DeliverTx(tx2)      # Regular tx
    DeliverTx(vup1)     # ValidatorUpdate tx — collected, not applied yet
    DeliverTx(vup2)     # ValidatorUpdate tx — collected, not applied yet
    EndBlock(H)
        → Process vup1, vup2
        → Validate safety (quorum preserved)
        → Compute new ValidatorSet for H+1
    Commit
        → Store ValidatorSet(H+1) in state
```

---

## 4. Safety Rules for Updates

### 4.1 Quorum Preservation

Before applying any batch of validator updates, the system MUST verify:

```
new_total_power = compute_total_power(new_validator_set)
quorum_threshold = (new_total_power * 2 / 3) + 1

// The new set must have enough validators to form a quorum
assert(new_validator_set.len() >= 1)
assert(new_total_power > 0)
```

### 4.2 Maximum Change Per Height

To prevent catastrophic validator set transitions that could compromise safety:

```
max_power_change_per_height = total_power(H) / 3

sum_of_power_changes = sum(|old_power - new_power| for each changed validator)
                     + sum(power for each added validator)
                     + sum(power for each removed validator)

assert(sum_of_power_changes <= max_power_change_per_height)
```

**Rationale:** If more than 1/3 of voting power changes in a single height, an attacker who controls the old validators could potentially equivocate on the old chain while the new validators operate on the new chain. Limiting changes to 1/3 per height ensures that at least 2/3 of the new set overlaps with the old set.

### 4.3 Validation Rules

| Rule | Check |
|------|-------|
| No duplicate validator IDs | `new_set` has no duplicate entries |
| Voting power > 0 for all active validators | `v.voting_power > 0` for all v in new_set |
| Public key format valid | Ed25519 key validation |
| Validator being removed exists | `old_set.contains(validator_id)` |
| Validator being added does not exist | `!old_set.contains(validator_id)` |
| Total power does not overflow | `new_total_power <= u64::MAX` |
| Power change within limit | See §4.2 |

### 4.4 Authorization

In the MVP, validator updates are authorized by checking the transaction sender against a **genesis-configured admin address list**:

```
genesis.json:
    admin_addresses: ["0xadmin1...", "0xadmin2..."]
```

Only transactions signed by an admin address are accepted as validator updates. This is a simple permissioned model suitable for the MVP. Future versions may introduce on-chain governance.

---

## 5. Transition Timing

```
Timeline:
    Height H-1: ValidatorSet(H-1) is active
    Height H:   ValidatorSet(H) is active (unchanged from H-1 unless updated at H-1)
                Block H contains ValidatorUpdate txs
                EndBlock(H) processes updates → computes ValidatorSet(H+1)
                Commit stores ValidatorSet(H+1)
    Height H+1: ValidatorSet(H+1) is active

                ┌──────────┐    ┌──────────┐    ┌──────────┐
                │ Height H │    │Height H+1│    │Height H+2│
                │          │    │          │    │          │
    ValSet:     │  VS(H)   │    │  VS(H+1) │    │  VS(H+2) │
                │          │    │  (new)   │    │          │
    Updates:    │ [vup1,   │    │          │    │          │
                │  vup2]   │    │          │    │          │
                │          │    │          │    │          │
    EndBlock:   │ compute  │    │          │    │          │
                │ VS(H+1)  │    │          │    │          │
                └──────────┘    └──────────┘    └──────────┘
```

**Key point:** The validator set used for consensus at height H is determined **before** height H begins. It is loaded from the committed state at height H-1. There is no ambiguity about which validators are active during any height.

---

## 6. Proposer Selection with Dynamic Sets

The proposer selection algorithm (weighted round-robin) is reset when the validator set changes:

```
On transition to height H+1 with a new ValidatorSet:
    1. Initialize proposer priorities for the new set
    2. Each validator's priority = 0
    3. Run the weighted round-robin from scratch for (H+1, round 0)
```

This ensures that proposer selection is deterministic and depends only on the validator set, not on historical proposer state from a different validator set.

---

## 7. Consensus Integration

### 7.1 At Height Boundary

```
When consensus advances from height H to H+1:
    1. Receive BlockExecutionResult from state machine
       - Includes: validator_updates: Vec<ValidatorUpdate>
    2. If validator_updates is non-empty:
       - Load current ValidatorSet(H)
       - Apply updates to produce ValidatorSet(H+1)
       - Validate safety rules (§4)
       - If validation fails: HALT NODE (this is a consensus-breaking bug)
    3. Store ValidatorSet(H+1) in state
    4. Consensus core loads ValidatorSet(H+1) for the new height
    5. Reset proposer selection state
```

### 7.2 Vote Validation

When validating a vote at height H:
- The vote's signer MUST be in ValidatorSet(H).
- The vote's signer MUST have voting_power > 0 in ValidatorSet(H).
- Votes from validators not in the current set are rejected.

### 7.3 Quorum Calculation

```
quorum(height H) = (ValidatorSet(H).total_power * 2 / 3) + 1
```

All quorum calculations use the validator set for the **current height**. Integer arithmetic only.

---

## 8. Storage

The validator set is stored as part of the application state:

```
Storage keys:
    "validator_set/current"     → serialized ValidatorSet (for current height)
    "validator_set/next"        → serialized ValidatorSet (for next height, if pending)
    "validator_set/height/{H}"  → serialized ValidatorSet (historical, for auditing)
```

Historical validator sets are retained for at least `N` heights (configurable, default: 1000) to support:
- Light client verification
- Evidence processing (equivocation proofs reference a specific height)
- Debugging

---

## 9. Edge Cases

### 9.1 Single Validator

- The system supports a single validator (for testing).
- The single validator is always the proposer.
- Quorum is trivially met (1/1 > 2/3).
- Removing the last validator is **rejected** (violates V4).

### 9.2 Validator Removes Itself

- A validator may submit a transaction to remove itself.
- The removal takes effect at H+1.
- The validator continues participating in consensus at height H.
- At H+1, the validator is no longer in the set and its votes are ignored.

### 9.3 All Updates in One Block Fail Validation

- If the batch of updates in a block fails safety validation (§4), the updates are **rejected as a batch**.
- The block itself is still valid (the updates are simply not applied).
- A receipt records the failure.
- This prevents a proposer from including malicious updates that would break quorum.

### 9.4 Conflicting Updates

- If two updates in the same block conflict (e.g., add and remove the same validator), they are processed in order.
- The last update wins.
- This is deterministic because transaction order within a block is fixed.

---

## 10. Verification Checklist

For any validator set transition from VS(H) to VS(H+1):

```
[1] VS(H+1) is derived deterministically from VS(H) + updates
[2] All updates were authorized (admin signature)
[3] Power change ≤ total_power(H) / 3
[4] total_power(VS(H+1)) > 0
[5] No duplicate validator IDs in VS(H+1)
[6] All voting_power > 0 in VS(H+1)
[7] validator_set_hash(VS(H+1)) matches what will be in block H+1's header
[8] Proposer selection is reset for the new set
```

---

## Definition of Done — Validator Sets

- [x] Core invariants (V1–V5) stated
- [x] Data model for Validator and ValidatorSet
- [x] All update operations (add, remove, update power) defined
- [x] Safety rules with quorum preservation and max change limit
- [x] Authorization model (admin addresses)
- [x] Transition timing with ASCII diagram
- [x] Proposer selection reset on set change
- [x] Consensus integration (vote validation, quorum calculation)
- [x] Storage scheme for validator sets
- [x] Edge cases (single validator, self-removal, batch failure, conflicts)
- [x] Verification checklist
- [x] No Rust source code included
