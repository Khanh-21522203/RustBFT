# RustBFT — State Transition Table

**Purpose:** Provide a complete, unambiguous finite state machine (FSM) specification for the consensus protocol, enumerating every (state, event) → (action, new_state) transition.
**Audience:** Engineers implementing the consensus core event handler.

---

## 1. Consensus States

The consensus core is always in exactly one of these states:

```
States:
    NewHeight       # Transitioning to a new height (loading validator set, resetting state)
    Propose         # Waiting for or constructing a proposal
    Prevote         # Waiting for prevotes to accumulate
    Precommit       # Waiting for precommits to accumulate
    Commit          # Block committed, executing and persisting
```

The state is parameterized by `(height: u64, round: u32)`.

---

## 2. Events

```
Events:
    EnterRound(height, round)           # Internal: advance to a new round
    ProposalReceived(proposal)          # From P2P: signed proposal
    ProposalTimeout(height, round)      # From Timer: no proposal received in time
    VoteReceived(vote)                  # From P2P: prevote or precommit
    PrevoteTimeout(height, round)       # From Timer: prevote phase timed out
    PrecommitTimeout(height, round)     # From Timer: precommit phase timed out
    BlockExecuted(result)               # From State Machine: block execution complete
    TxsAvailable(txs)                   # From Mempool: transactions for proposal
```

---

## 3. Conditions (Guards)

These boolean conditions are referenced in the transition table:

```
Guards:
    IS_PROPOSER             # This node is the proposer for (height, round)
    VALID_PROPOSAL          # Received proposal is valid (signature, block validity)
    HAS_POLKA(B)            # >2/3 prevotes for block B at current round
    HAS_POLKA_NIL           # >2/3 prevotes for nil at current round
    HAS_POLKA_ANY           # >2/3 prevotes total (for any value including nil)
    HAS_COMMIT(B)           # >2/3 precommits for block B at current round
    HAS_PRECOMMIT_ANY       # >2/3 precommits total (for any value including nil)
    LOCKED                  # locked_block is Some
    LOCKED_ON(B)            # locked_block == Some(B)
    CAN_UNLOCK(B, vr)       # proposal.valid_round >= locked_round AND has_polka(B) at valid_round
    HAS_VALID_BLOCK         # valid_block is Some
    STALE_EVENT             # Event's (height, round) does not match current state
```

---

## 4. Complete State Transition Table

### 4.1 NewHeight

| Current State | Event | Guard | Actions | Next State |
|--------------|-------|-------|---------|------------|
| NewHeight(H) | EnterRound(H, 0) | — | Load ValidatorSet(H). Reset locked/valid. Schedule propose. | Propose(H, 0) |

### 4.2 Propose

| Current State | Event | Guard | Actions | Next State |
|--------------|-------|-------|---------|------------|
| Propose(H, R) | EnterRound(H, R) | IS_PROPOSER ∧ HAS_VALID_BLOCK | Re-propose valid_block with valid_round. Broadcast proposal. WAL: ProposalSent. Schedule timeout_propose. | Propose(H, R) |
| Propose(H, R) | EnterRound(H, R) | IS_PROPOSER ∧ ¬HAS_VALID_BLOCK | Request txs from mempool. | Propose(H, R) (waiting for txs) |
| Propose(H, R) | TxsAvailable(txs) | IS_PROPOSER | Construct block. Sign proposal. Broadcast. WAL: ProposalSent. Prevote for own block. WAL: VoteSent. Broadcast prevote. | Prevote(H, R) |
| Propose(H, R) | EnterRound(H, R) | ¬IS_PROPOSER | Schedule timeout_propose. | Propose(H, R) |
| Propose(H, R) | ProposalReceived(P) | VALID_PROPOSAL ∧ ¬LOCKED | Prevote for P.block. WAL: VoteSent. Broadcast prevote. Cancel timeout_propose. | Prevote(H, R) |
| Propose(H, R) | ProposalReceived(P) | VALID_PROPOSAL ∧ LOCKED_ON(P.block) | Prevote for P.block. WAL: VoteSent. Broadcast prevote. Cancel timeout_propose. | Prevote(H, R) |
| Propose(H, R) | ProposalReceived(P) | VALID_PROPOSAL ∧ LOCKED ∧ ¬LOCKED_ON(P.block) ∧ CAN_UNLOCK(P.block, P.valid_round) | Prevote for P.block. WAL: VoteSent. Broadcast prevote. Cancel timeout_propose. | Prevote(H, R) |
| Propose(H, R) | ProposalReceived(P) | VALID_PROPOSAL ∧ LOCKED ∧ ¬LOCKED_ON(P.block) ∧ ¬CAN_UNLOCK | Prevote nil. WAL: VoteSent. Broadcast prevote. Cancel timeout_propose. | Prevote(H, R) |
| Propose(H, R) | ProposalReceived(P) | ¬VALID_PROPOSAL | Prevote nil. WAL: VoteSent. Broadcast prevote. Cancel timeout_propose. | Prevote(H, R) |
| Propose(H, R) | ProposalTimeout(H, R) | — | Prevote nil. WAL: VoteSent. Broadcast prevote. | Prevote(H, R) |
| Propose(H, R) | VoteReceived(V) | — | Buffer vote in VoteSet. Check if polka/commit formed (see Prevote/Precommit rules). | Propose(H, R) or jump (see §5) |
| Propose(H, R) | * | STALE_EVENT | Drop event. | Propose(H, R) |

### 4.3 Prevote

| Current State | Event | Guard | Actions | Next State |
|--------------|-------|-------|---------|------------|
| Prevote(H, R) | VoteReceived(prevote) | HAS_POLKA(B) ∧ have_block(B) ∧ valid(B) | Set valid_block=B, valid_round=R. Set locked_block=B, locked_round=R. WAL: Locked. Precommit B. WAL: VoteSent. Broadcast precommit. Cancel timeout_prevote. | Precommit(H, R) |
| Prevote(H, R) | VoteReceived(prevote) | HAS_POLKA(B) ∧ (¬have_block(B) ∨ ¬valid(B)) | Set valid_block=B, valid_round=R. Precommit nil. WAL: VoteSent. Broadcast precommit. Cancel timeout_prevote. | Precommit(H, R) |
| Prevote(H, R) | VoteReceived(prevote) | HAS_POLKA_NIL | Unlock: locked_block=None, locked_round=None. WAL: Unlocked. Precommit nil. WAL: VoteSent. Broadcast precommit. Cancel timeout_prevote. | Precommit(H, R) |
| Prevote(H, R) | VoteReceived(prevote) | HAS_POLKA_ANY ∧ ¬HAS_POLKA(B) ∧ ¬HAS_POLKA_NIL | Schedule timeout_prevote (if not already scheduled). | Prevote(H, R) |
| Prevote(H, R) | VoteReceived(prevote) | ¬HAS_POLKA_ANY | Buffer vote. | Prevote(H, R) |
| Prevote(H, R) | PrevoteTimeout(H, R) | — | Precommit nil. WAL: VoteSent. Broadcast precommit. | Precommit(H, R) |
| Prevote(H, R) | ProposalReceived(P) | VALID_PROPOSAL ∧ HAS_POLKA(P.block) already | Process as if polka just formed (same as VoteReceived + HAS_POLKA). | Precommit(H, R) |
| Prevote(H, R) | VoteReceived(precommit) | — | Buffer in VoteSet. Check for commit (see §5). | Prevote(H, R) or jump |
| Prevote(H, R) | * | STALE_EVENT | Drop event. | Prevote(H, R) |

### 4.4 Precommit

| Current State | Event | Guard | Actions | Next State |
|--------------|-------|-------|---------|------------|
| Precommit(H, R) | VoteReceived(precommit) | HAS_COMMIT(B) | WAL: CommitStarted. Emit ExecuteBlock(B). | Commit(H, R) |
| Precommit(H, R) | VoteReceived(precommit) | HAS_PRECOMMIT_ANY ∧ ¬HAS_COMMIT | Schedule timeout_precommit (if not already scheduled). | Precommit(H, R) |
| Precommit(H, R) | VoteReceived(precommit) | ¬HAS_PRECOMMIT_ANY | Buffer vote. | Precommit(H, R) |
| Precommit(H, R) | PrecommitTimeout(H, R) | — | WAL: RoundStarted(H, R+1). | Propose(H, R+1) via EnterRound |
| Precommit(H, R) | VoteReceived(prevote) | — | Buffer in VoteSet (late prevote). May trigger polka detection for future reference. | Precommit(H, R) |
| Precommit(H, R) | ProposalReceived(P) | — | Buffer proposal (may be needed if commit happens and we need the block). | Precommit(H, R) |
| Precommit(H, R) | * | STALE_EVENT | Drop event. | Precommit(H, R) |

### 4.5 Commit

| Current State | Event | Guard | Actions | Next State |
|--------------|-------|-------|---------|------------|
| Commit(H, R) | BlockExecuted(result) | — | WAL: CommitCompleted. Emit PersistBlock. Emit EvictTxs to mempool. Apply validator_updates. Advance height. | NewHeight(H+1) → Propose(H+1, 0) |
| Commit(H, R) | * | — | Buffer or drop. No state changes until BlockExecuted. | Commit(H, R) |

---

## 5. Cross-State Jump Rules

Certain events can trigger state transitions regardless of the current step:

### 5.1 Commit from Any State

If at any point during height H, the node accumulates >2/3 precommits for a block B (from any round, not just the current round):

```
IF HAS_COMMIT(B) at any round R':
    Cancel all pending timeouts for height H
    WAL: CommitStarted
    Emit ExecuteBlock(B)
    → Commit(H, R')
```

This handles the case where a node is behind (e.g., still in Propose) but receives enough precommits to commit directly.

### 5.2 Round Skip

If the node receives votes from a higher round R' > R (indicating other validators have moved ahead):

```
IF received >1/3 votes (prevote or precommit) at round R' > current round R:
    This is a "round skip" signal
    → EnterRound(H, R')
    → Propose(H, R')
```

**Rationale:** If >1/3 of voting power is in round R', then a quorum cannot form in round R (since >2/3 is needed). The node should catch up to avoid delaying consensus.

---

## 6. State Transition Diagram

```
                          EnterRound(H,0)
                    ┌──────────────────────┐
                    │                      │
                    ▼                      │
              ┌──────────┐                 │
              │ NewHeight │                 │
              │   (H)    │                 │
              └────┬─────┘                 │
                   │                       │
                   │ EnterRound            │
                   ▼                       │
              ┌──────────┐                 │
         ┌───▶│ Propose  │                 │
         │    │ (H, R)   │                 │
         │    └────┬─────┘                 │
         │         │                       │
         │         │ Prevote cast          │
         │         ▼                       │
         │    ┌──────────┐                 │
         │    │ Prevote  │                 │
         │    │ (H, R)   │                 │
         │    └────┬─────┘                 │
         │         │                       │
         │         │ Precommit cast        │
         │         ▼                       │
         │    ┌──────────┐                 │
         │    │Precommit │                 │
         │    │ (H, R)   │                 │
         │    └────┬─────┘                 │
         │         │         │             │
         │         │ Timeout │ >2/3        │
         │         │         │ precommits  │
         │         ▼         ▼             │
         │    ┌────────┐ ┌────────┐        │
         │    │Round+1 │ │ Commit │        │
         │    │        │ │ (H, R) │        │
         │    └───┬────┘ └───┬────┘        │
         │        │          │             │
         └────────┘          │ BlockExecuted
                             │             │
                             └─────────────┘
                              → NewHeight(H+1)
```

---

## 7. Timeout Scheduling Summary

| Transition | Timeout Scheduled |
|------------|-------------------|
| EnterRound(H, R) where IS_PROPOSER | None (proposer constructs block) |
| EnterRound(H, R) where ¬IS_PROPOSER | timeout_propose(H, R) |
| → Prevote (after casting prevote) | None initially |
| HAS_POLKA_ANY but no specific polka | timeout_prevote(H, R) |
| → Precommit (after casting precommit) | None initially |
| HAS_PRECOMMIT_ANY but no commit | timeout_precommit(H, R) |
| → Commit | Cancel all timeouts for height H |
| → NewHeight(H+1) | None (EnterRound will schedule) |

---

## 8. WAL Entry Summary per Transition

| Transition | WAL Entry |
|------------|-----------|
| EnterRound(H, R) | RoundStarted(H, R) |
| Proposer sends proposal | ProposalSent(H, R, block_hash) |
| Node casts prevote | VoteSent(H, R, Prevote, block_hash_or_nil) |
| Node casts precommit | VoteSent(H, R, Precommit, block_hash_or_nil) |
| Node locks on block | Locked(H, R, block_hash) |
| Node unlocks | Unlocked(H, R) |
| Commit begins | CommitStarted(H, block_hash) |
| Commit completes | CommitCompleted(H, state_root) |

---

## 9. Invariants (Must Hold at All Times)

| # | Invariant |
|---|-----------|
| I1 | A node casts at most one prevote per (height, round). |
| I2 | A node casts at most one precommit per (height, round). |
| I3 | If locked_block = Some(B) at round R, the node will not prevote for any block ≠ B in rounds > R unless it sees a polka for that block at a round ≥ locked_round. |
| I4 | If the node precommits B, it has seen a polka for B at the current round. |
| I5 | locked_round ≤ valid_round ≤ round (when all are Some). |
| I6 | The node never processes events for a height it has already committed. |
| I7 | WAL entries are written before the corresponding action is taken. |
| I8 | State transitions are deterministic: same (state, event) → same (actions, new_state). |

---

## 10. Stale Event Handling

Events may arrive for heights or rounds that the node has already passed:

```
Stale event rules:
    IF event.height < current_height:
        DROP (already committed)

    IF event.height == current_height AND event.round < current_round:
        BUFFER vote in VoteSet (may be needed for polka/commit detection)
        Do NOT change step or round

    IF event.height > current_height:
        BUFFER (node may be behind; will process after catching up)

    IF event.round > current_round:
        Check for round skip (§5.2)
```

---

## 11. Example Walkthrough: Happy Path

```
Node 0 (proposer), Nodes 1-3 (validators), Height 5, Round 0

1. All nodes: EnterRound(5, 0)
   Node 0: IS_PROPOSER → request txs from mempool
   Nodes 1-3: schedule timeout_propose

2. Node 0: TxsAvailable → construct block B5
   Node 0: broadcast Proposal(B5)
   Node 0: prevote B5, broadcast prevote

3. Nodes 1-3: ProposalReceived(B5)
   Nodes 1-3: validate B5, prevote B5, broadcast prevote

4. All nodes: receive 4/4 prevotes for B5
   HAS_POLKA(B5) → lock on B5
   All nodes: precommit B5, broadcast precommit

5. All nodes: receive 4/4 precommits for B5
   HAS_COMMIT(B5) → emit ExecuteBlock(B5)
   → Commit(5, 0)

6. All nodes: BlockExecuted(result)
   → PersistBlock, EvictTxs
   → NewHeight(6) → Propose(6, 0)
```

---

## 12. Example Walkthrough: Proposer Failure

```
Node 0 (proposer, crashed), Nodes 1-3, Height 5, Round 0

1. All nodes: EnterRound(5, 0)
   Nodes 1-3: schedule timeout_propose (Node 0 is proposer but offline)

2. Nodes 1-3: timeout_propose fires
   Nodes 1-3: prevote nil, broadcast

3. Nodes 1-3: receive 3/3 prevotes for nil (3/4 total power, >2/3)
   HAS_POLKA_NIL → unlock
   Nodes 1-3: precommit nil, broadcast

4. Nodes 1-3: receive 3/3 precommits for nil (>2/3)
   HAS_PRECOMMIT_ANY but no HAS_COMMIT(B) → schedule timeout_precommit

5. Nodes 1-3: timeout_precommit fires
   → EnterRound(5, 1)
   Node 1 is now proposer for (5, 1)

6. Node 1: construct block B5, broadcast proposal
   ... normal flow continues ...
```

---

## Definition of Done — State Transition Table

- [x] All consensus states enumerated
- [x] All events enumerated
- [x] All guard conditions defined
- [x] Complete transition table for every (state, event, guard) combination
- [x] Cross-state jump rules (commit from any state, round skip)
- [x] State transition diagram (ASCII)
- [x] Timeout scheduling summary
- [x] WAL entry mapping per transition
- [x] Invariants listed
- [x] Stale event handling rules
- [x] Happy path walkthrough
- [x] Failure path walkthrough
- [x] No Rust source code included
