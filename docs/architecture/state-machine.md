# RustBFT — State Machine

**Purpose:** Define the replicated state machine: block execution pipeline, state model, transaction processing, and determinism guarantees.
**Audience:** Engineers implementing the state machine and block execution layers.

---

## 1. Responsibilities

The state machine layer is responsible for:

1. **Block execution** — process committed blocks through the BeginBlock → DeliverTx → EndBlock → Commit pipeline.
2. **State management** — maintain the canonical application state (accounts, balances, nonces, contract storage).
3. **State root computation** — produce a Merkle root hash of the entire state after each block.
4. **Transaction validation** — perform stateful validation (nonce, balance, gas) during execution.
5. **Validator set updates** — collect and apply validator changes during EndBlock.

The state machine MUST NOT:

- Initiate consensus decisions.
- Access the network.
- Use wall-clock time, randomness, or any non-deterministic input.
- Perform async operations.

---

## 2. State Model

### 2.1 Account State

```
Account:
    address:       Address          # 20-byte derived from public key
    balance:       u128             # Native token balance (integer, no decimals)
    nonce:         u64              # Monotonically increasing, prevents replay
    code_hash:     Option<Hash>     # If contract account, hash of deployed bytecode
    storage_root:  Hash             # Merkle root of contract storage (empty for EOA)
```

### 2.2 Global State

```
AppState:
    accounts:        MerkleTree<Address, Account>
    contract_code:   KVStore<Hash, Bytes>           # code_hash → bytecode
    contract_storage: MerkleTree<(Address, Key), Value>
    validator_set:   ValidatorSet
    chain_params:    ChainParams                    # gas limits, block size, etc.
```

All state is stored in a **Merkle tree** (sparse Merkle trie or similar) to enable:
- O(log n) state root computation after updates.
- Inclusion/exclusion proofs for any key.
- Deterministic root hash regardless of insertion order.

### 2.3 State Versioning

```
State at height H = result of executing blocks 0..H

state_root(H) = merkle_root(AppState after block H)
```

The system maintains at minimum:
- **Latest committed state** (height H)
- **Previous committed state** (height H-1) for queries during block execution

---

## 3. Block Execution Pipeline

```
┌─────────────────────────────────────────────────────────────┐
│                  BLOCK EXECUTION PIPELINE                    │
│                                                              │
│  Consensus commits block B at height H                       │
│                                                              │
│  ┌──────────────────────────────────────────────────────┐   │
│  │ 1. BeginBlock(B.header)                              │   │
│  │    - Record block metadata                           │   │
│  │    - Process evidence (equivocation)                  │   │
│  │    - Update per-block context                        │   │
│  └──────────────────────┬───────────────────────────────┘   │
│                         │                                    │
│  ┌──────────────────────▼───────────────────────────────┐   │
│  │ 2. DeliverTx(tx) × N  (for each tx in B.txs)        │   │
│  │    - Stateful validation (nonce, balance, gas)       │   │
│  │    - Execute transaction                             │   │
│  │    - If success: apply state changes                 │   │
│  │    - If failure: revert tx state, keep gas charge    │   │
│  │    - Record receipt (success/failure, gas used, logs)│   │
│  └──────────────────────┬───────────────────────────────┘   │
│                         │                                    │
│  ┌──────────────────────▼───────────────────────────────┐   │
│  │ 3. EndBlock(B.header)                                │   │
│  │    - Process validator set update transactions       │   │
│  │    - Compute pending validator changes               │   │
│  │    - Finalize block-level state changes              │   │
│  └──────────────────────┬───────────────────────────────┘   │
│                         │                                    │
│  ┌──────────────────────▼───────────────────────────────┐   │
│  │ 4. Commit                                            │   │
│  │    - Compute state_root = merkle_root(AppState)      │   │
│  │    - Flush state to persistent storage               │   │
│  │    - Return (state_root, validator_updates)          │   │
│  └──────────────────────────────────────────────────────┘   │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

---

## 4. Transaction Types

### 4.1 Transfer

```
Transfer:
    from:      Address
    to:        Address
    amount:    u128
    nonce:     u64
    gas_limit: u64
    signature: Signature
```

Execution:
1. Verify signature.
2. Check `from.nonce == tx.nonce`.
3. Check `from.balance >= amount + gas_cost`.
4. Deduct `amount` from `from.balance`, add to `to.balance`.
5. Increment `from.nonce`.
6. Charge gas.

### 4.2 Contract Deploy

```
ContractDeploy:
    from:      Address
    code:      Bytes           # Contract bytecode (WASM)
    init_args: Bytes           # Constructor arguments
    nonce:     u64
    gas_limit: u64
    signature: Signature
```

Execution:
1. Verify signature, nonce, balance.
2. Compute `contract_address = hash(from, nonce)`.
3. Store bytecode at `contract_code[code_hash]`.
4. Create account with `code_hash`.
5. Execute constructor (`init` function) with gas metering.
6. If constructor fails: revert all state changes for this tx.

### 4.3 Contract Call

```
ContractCall:
    from:      Address
    to:        Address         # Contract address
    input:     Bytes           # ABI-encoded function call
    amount:    u128            # Value transfer (can be 0)
    nonce:     u64
    gas_limit: u64
    signature: Signature
```

### 4.4 Validator Update (Special Transaction)

```
ValidatorUpdate:
    from:       Address        # Must be an authorized admin address
    action:     ValidatorAction  # Add | Remove | UpdatePower
    validator:  ValidatorId
    new_power:  Option<u64>
    nonce:      u64
    gas_limit:  u64
    signature:  Signature
```

Processed during EndBlock, not DeliverTx. See `docs/architecture/validator-sets.md`.

---

## 5. Transaction Execution Semantics

### 5.1 Gas Metering

Every operation costs gas:

| Operation | Gas Cost |
|-----------|----------|
| Base transaction | 21,000 |
| Transfer | 0 (included in base) |
| Contract deploy (per byte) | 200 |
| Contract call (base) | 10,000 |
| State read (per key) | 500 |
| State write (per key) | 5,000 |
| State delete (per key) | 2,500 (refund) |
| Hash computation (per 32 bytes) | 100 |
| Signature verification | 3,000 |

Gas is charged **before** execution. If `gas_used > gas_limit`, execution halts immediately.

### 5.2 Failure Semantics

```
execute_tx(tx, state):
    snapshot = state.snapshot()
    
    result = try_execute(tx, state)
    
    IF result is Error:
        state.revert_to(snapshot)
        // Gas is still charged (deducted from sender)
        // Failed tx remains in the block
        RETURN Receipt { success: false, gas_used, error }
    
    RETURN Receipt { success: true, gas_used, logs }
```

**Critical invariants:**
- A failed transaction MUST NOT corrupt global state.
- A failed transaction MUST still be included in the block (for gas accounting and replay determinism).
- The gas charge for a failed transaction is applied to the sender's balance (deducted from the snapshot-reverted state).

### 5.3 Ordering

Transactions within a block are executed **in the order specified by the proposer**. There is no re-ordering, no parallel execution, and no dependency analysis. Sequential execution is required for determinism.

---

## 6. State Transitions

### 6.1 Snapshot and Revert

The state machine supports **snapshots** for transaction-level rollback:

```
Snapshot:
    depth:    u32              # Nesting level
    changes:  Vec<StateChange> # All writes since snapshot

StateChange:
    key:       Bytes
    old_value: Option<Bytes>   # For revert
    new_value: Option<Bytes>
```

Snapshots are stack-based. Contract-to-contract calls create nested snapshots. Revert unwinds the stack to the target depth.

### 6.2 State Root Computation

After all transactions in a block are executed:

1. Collect all modified keys.
2. Update the Merkle tree with new values.
3. Recompute the root hash.
4. This root hash is the `state_root` included in the **next** block's header (since the proposer must know the state root of the parent block).

**Wait — clarification on state root timing:**

The `state_root` in block H's header is the state root **after executing block H**. This means:
- The proposer proposes block H with `state_root = None` (or a placeholder).
- After commit, all nodes execute block H and compute the state root.
- The state root is stored alongside block H in storage.
- Block H+1's header includes `prev_state_root` which validators verify.

This avoids requiring the proposer to execute the block before proposing it (which would add latency and complexity).

---

## 7. Genesis State

The genesis state is loaded from a genesis file:

```
genesis.json:
    chain_id:       "rustbft-testnet-1"
    genesis_time:   1700000000
    initial_accounts:
        - address: "0xabc..."
          balance:  1000000
        - address: "0xdef..."
          balance:  500000
    initial_validators:
        - pubkey:  "ed25519:..."
          power:   100
        - pubkey:  "ed25519:..."
          power:   100
    chain_params:
        max_block_gas:    10000000
        max_block_bytes:  1048576
        max_tx_bytes:     65536
```

Genesis state root is computed by inserting all initial accounts into an empty Merkle tree.

---

## 8. Determinism Guarantees

| Concern | Mitigation |
|---------|------------|
| Map iteration order | Use `BTreeMap` exclusively. No `HashMap`. |
| Floating-point | Prohibited. All arithmetic is integer-based. |
| Serialization | Canonical binary format. No serde defaults. |
| Timestamp | Block timestamp is proposer-set, advisory. Not used in state transitions. |
| Randomness | Prohibited. No `rand` crate in execution paths. |
| System calls | Prohibited. No file I/O, network, or clock access during execution. |
| Integer overflow | All arithmetic uses checked operations. Overflow is a deterministic error. |
| String encoding | UTF-8 only. Validated at input boundaries. |

---

## 9. Interface with Consensus

The state machine receives commands from the consensus core and returns results:

```
// Command from consensus
ExecuteBlock:
    block: Block

// Response to consensus
BlockExecutionResult:
    height:            u64
    state_root:        Hash
    tx_receipts:       Vec<Receipt>
    validator_updates: Vec<ValidatorUpdate>
    gas_used:          u64
```

The state machine is invoked **synchronously** from the node's main coordination logic (not from within the consensus core thread). The consensus core emits an `ExecuteBlock` command; the node binary routes it to the state machine; the result is sent back to consensus as a `BlockExecuted` event.

---

## 10. Error Handling

| Error | Handling |
|-------|----------|
| Invalid transaction (bad nonce) | Skip tx, include in block with failed receipt |
| Insufficient balance | Skip tx, include in block with failed receipt |
| Gas exhaustion | Halt execution, revert tx state, charge gas |
| Contract panic | Revert tx state, charge gas, log error |
| State root mismatch (between nodes) | **HALT NODE.** This indicates a determinism bug. |
| Storage I/O error | **HALT NODE.** Cannot guarantee state consistency. |

A state root mismatch is a **critical bug** — it means two honest nodes diverged, which violates the fundamental invariant. The node must halt and an operator must investigate.

---

## Definition of Done — State Machine

- [x] State model (accounts, contracts, storage) defined
- [x] Block execution pipeline (BeginBlock/DeliverTx/EndBlock/Commit) specified
- [x] All transaction types defined with execution semantics
- [x] Gas metering table provided
- [x] Failure semantics (revert, gas charge) specified
- [x] Snapshot/revert mechanism for rollback
- [x] State root computation timing clarified
- [x] Genesis state format defined
- [x] Determinism guarantees enumerated
- [x] Interface with consensus specified
- [x] Error handling with halt conditions
- [x] No Rust source code included
