# RustBFT — Consensus Protocol

**Purpose:** Define the BFT consensus protocol used by RustBFT, including round structure, voting rules, locking mechanism, and safety/liveness arguments.
**Audience:** Core engineers implementing the consensus layer.

---

## 1. Protocol Overview

RustBFT uses a **three-phase, round-based BFT consensus protocol** with deterministic finality. The protocol is original but draws on well-understood BFT principles (the "propose-prevote-precommit" pattern).

### Key Properties

| Property | Guarantee |
|----------|-----------|
| **Safety** | No two different blocks are committed at the same height, provided f < n/3 Byzantine validators. |
| **Liveness** | The system commits new blocks when >2/3 of voting power is honest and connected. |
| **Deterministic finality** | A committed block is final. No forks, no reorgs. |
| **Accountability** | Equivocation (double-voting) is detectable and provable. |

### Threat Model

- **n** = total voting power
- **f** = Byzantine voting power
- Safety holds when **f < n/3**
- Liveness holds when honest voting power **> 2n/3** and the network is eventually synchronous

---

## 2. Consensus State

The consensus core maintains the following state, owned exclusively by a single thread:

```
ConsensusState:
    height:        u64           # Current block height being decided
    round:         u32           # Current round within this height
    step:          Step          # Current phase: {Propose, Prevote, Precommit, Commit}

    locked_round:  Option<u32>   # Round at which this node locked on a block
    locked_block:  Option<Hash>  # Hash of the block this node is locked on
    valid_round:   Option<u32>   # Round at which a valid block was seen with polka
    valid_block:   Option<Hash>  # Hash of that valid block

    proposal:      Option<SignedProposal>   # Current round's proposal (if received)
    votes:         VoteSet       # All votes for current height, indexed by (round, step, validator)
    validator_set: ValidatorSet  # Fixed for this height
```

**Invariant:** `locked_round <= valid_round <= round` (when all are Some).

---

## 3. Round Structure

Each height proceeds through one or more rounds. Each round has three phases:

```
┌──────────────────────────────────────────────────────────────┐
│                     ROUND r at HEIGHT h                      │
│                                                              │
│  ┌──────────┐    ┌──────────┐    ┌─────────────┐            │
│  │ PROPOSE  │───▶│ PREVOTE  │───▶│ PRECOMMIT   │            │
│  │          │    │          │    │             │            │
│  │ Proposer │    │ All vote │    │ All vote    │            │
│  │ sends    │    │ for block│    │ to commit   │            │
│  │ block    │    │ or nil   │    │ or nil      │            │
│  └──────────┘    └──────────┘    └─────────────┘            │
│       │               │               │                      │
│   timeout_propose  timeout_prevote  timeout_precommit        │
│   (if no proposal) (if no polka)   (if no commit)           │
│                                        │                     │
│                              ┌─────────▼──────────┐         │
│                              │ COMMIT or NEXT ROUND│         │
│                              └────────────────────┘         │
└──────────────────────────────────────────────────────────────┘
```

### Phase Details

#### Phase 1: Propose

1. Determine proposer for (height, round) using deterministic selection (see §6).
2. If this node is the proposer:
   a. If `valid_block` is set, re-propose `valid_block` with `valid_round`.
   b. Otherwise, request transactions from mempool, construct a new block, sign a Proposal.
   c. Broadcast the Proposal.
3. Start `timeout_propose(height, round)`.

#### Phase 2: Prevote

Upon entering Prevote (either by receiving a proposal or by timeout):

1. **If** a valid proposal for (height, round) was received **AND** the block is valid **AND** (the node is not locked, or the node is locked on this block, or the proposal's `valid_round` ≥ `locked_round` with a polka at that round):
   - Prevote for the proposed block hash.
2. **Else:**
   - Prevote nil.
3. Broadcast the prevote.
4. Start `timeout_prevote(height, round)`.

#### Phase 3: Precommit

Upon collecting >2/3 prevotes (a "polka"):

1. **If** polka is for a specific block B:
   - Set `valid_block = B`, `valid_round = round`.
   - If the node has the full block and it is valid:
     - Set `locked_block = B`, `locked_round = round`.
     - Precommit for B.
   - Else: Precommit nil.
2. **If** polka is for nil:
   - Unlock: set `locked_round = None`, `locked_block = None`.
   - Precommit nil.
3. **If** timeout_prevote fires without polka:
   - Precommit nil.
4. Broadcast the precommit.
5. Start `timeout_precommit(height, round)`.

#### Commit

Upon collecting >2/3 precommits for a specific block B:

1. Commit block B.
2. Emit `ExecuteBlock` command to the state machine.
3. Wait for execution result (state root, validator updates).
4. Emit `PersistBlock` command to storage.
5. Advance to (height+1, round=0, step=Propose).
6. Reset `locked_round`, `locked_block`, `valid_round`, `valid_block` to None.
7. Apply validator set updates (effective at new height).

#### Round Advancement

If `timeout_precommit` fires without a commit:

1. Advance to (height, round+1, step=Propose).
2. **Do NOT** reset `locked_round`, `locked_block`, `valid_round`, `valid_block`.
3. These values carry across rounds within the same height.

---

## 4. Voting Rules

### 4.1 Prevote Rules (Detailed)

```
PREVOTE(height, round):
    proposal = get_proposal(height, round)

    IF proposal is None:
        RETURN Prevote(nil)

    block = proposal.block
    IF NOT valid_block(block):
        RETURN Prevote(nil)

    // Locking rules
    IF locked_block is None:
        RETURN Prevote(block.hash)

    IF locked_block == block.hash:
        RETURN Prevote(block.hash)

    IF proposal.valid_round >= 0
       AND proposal.valid_round >= locked_round
       AND has_polka(height, proposal.valid_round, block.hash):
        RETURN Prevote(block.hash)

    RETURN Prevote(nil)
```

### 4.2 Precommit Rules (Detailed)

```
PRECOMMIT(height, round):
    IF has_polka_for_block(height, round, B):
        valid_block  = B
        valid_round  = round
        IF have_full_block(B) AND valid_block(B):
            locked_block = B
            locked_round = round
            RETURN Precommit(B)
        ELSE:
            RETURN Precommit(nil)

    IF has_polka_for_nil(height, round):
        locked_block = None
        locked_round = None
        RETURN Precommit(nil)

    // Timeout path
    RETURN Precommit(nil)
```

### 4.3 Vote Validity

A vote is valid if and only if:
1. It is signed by a current validator.
2. The signature is correct.
3. The height matches the current height.
4. The validator has not already voted in this (round, step).
5. The vote type matches the expected step.

Duplicate votes from the same validator for the same (height, round, step) with **different** block hashes constitute **equivocation** and MUST be recorded as evidence.

---

## 5. Locking Mechanism

The locking mechanism prevents safety violations across rounds.

### Why Locking is Necessary

Without locking, a validator could prevote for block A in round 1, then prevote for block B in round 2. If different subsets of validators see different polkas, two blocks could be committed at the same height.

### Lock Invariants

1. A validator locks on block B at round r when it sees a polka for B at round r.
2. A locked validator MUST NOT prevote for any block other than B in subsequent rounds, **unless** it sees a polka for a different block at a round ≥ locked_round.
3. A validator unlocks when it sees a polka for nil.

### Safety Argument (Sketch)

Assume for contradiction that blocks B₁ and B₂ (B₁ ≠ B₂) are both committed at height h.

- B₁ committed ⟹ >2/3 precommitted B₁ at some round r₁.
- B₂ committed ⟹ >2/3 precommitted B₂ at some round r₂.
- Precommit requires a polka ⟹ >2/3 prevoted B₁ at r₁, >2/3 prevoted B₂ at r₂.
- Since f < n/3, the honest sets overlap: some honest validator V prevoted B₁ at r₁ and B₂ at r₂.
- WLOG r₁ < r₂. V locked on B₁ at r₁. V can only prevote B₂ at r₂ if it saw a polka for B₂ at some round r' where r₁ ≤ r' < r₂.
- But that polka for B₂ requires >2/3 prevotes for B₂, which conflicts with >2/3 being locked on B₁.
- Contradiction. ∎

---

## 6. Proposer Selection

The proposer for (height, round) is selected deterministically:

```
proposer_index = weighted_round_robin(height, round, validator_set)
```

### Weighted Round Robin Algorithm

Each validator has a `voting_power` weight. The algorithm:

1. Each validator maintains a `priority` accumulator (initialized to 0).
2. At each selection step:
   a. Add `voting_power` to each validator's `priority`.
   b. Select the validator with the highest `priority`.
   c. Subtract `total_voting_power` from the selected validator's `priority`.
3. This ensures proportional selection over time.

**Critical:** The priority state is deterministic and derived solely from the validator set and the number of selection steps. It is recomputed from genesis (or from the last committed validator set) — never stored in consensus state.

---

## 7. Timeouts

Timeouts drive liveness. They are managed by the async outer shell and delivered as events.

### Timeout Values

```
timeout_propose(round)   = BASE_TIMEOUT_PROPOSE   + round * TIMEOUT_DELTA
timeout_prevote(round)   = BASE_TIMEOUT_PREVOTE   + round * TIMEOUT_DELTA
timeout_precommit(round) = BASE_TIMEOUT_PRECOMMIT + round * TIMEOUT_DELTA
```

Default values:
- `BASE_TIMEOUT_PROPOSE`   = 1000ms
- `BASE_TIMEOUT_PREVOTE`   = 1000ms
- `BASE_TIMEOUT_PRECOMMIT` = 1000ms
- `TIMEOUT_DELTA`          = 500ms

Timeouts grow linearly with round number, ensuring liveness under increasing asynchrony.

### Timeout Rules

| Timeout | Triggered When | Action |
|---------|---------------|--------|
| `timeout_propose` | No proposal received | Prevote nil |
| `timeout_prevote` | >2/3 prevotes received but no polka for a specific block | Precommit nil |
| `timeout_precommit` | >2/3 precommits received but no commit | Move to next round |

**Important:** Timeouts are **not** part of the consensus core. The consensus core emits `ScheduleTimeout` commands; the async timer service delivers `Timeout*` events back.

---

## 8. Message Types

### Proposal

```
Proposal:
    height:      u64
    round:       u32
    block:       Block
    valid_round: i32          # -1 if no prior valid round; else the round number
    proposer:    ValidatorId
    signature:   Signature
```

### Vote (Prevote or Precommit)

```
Vote:
    vote_type:   VoteType     # Prevote | Precommit
    height:      u64
    round:       u32
    block_hash:  Option<Hash> # None = nil vote
    validator:   ValidatorId
    signature:   Signature
```

### Block

```
Block:
    header:      BlockHeader
    txs:         Vec<Transaction>
    last_commit: CommitInfo   # >2/3 precommit signatures for previous block

BlockHeader:
    height:           u64
    timestamp:        u64          # Unix millis, advisory only
    prev_block_hash:  Hash
    proposer:         ValidatorId
    validator_set_hash: Hash
    state_root:       Hash         # After executing this block
    tx_merkle_root:   Hash
```

---

## 9. Vote Aggregation

### VoteSet Structure

```
VoteSet:
    height: u64
    votes:  BTreeMap<(u32, VoteType, ValidatorId), SignedVote>
```

Using `BTreeMap` ensures deterministic iteration order.

### Quorum Calculation

```
quorum_threshold = (total_voting_power * 2 / 3) + 1

has_quorum(votes_for_X) = sum(voting_power of validators who voted for X) >= quorum_threshold
```

**No floating-point arithmetic.** Quorum is computed with integer arithmetic only.

### Polka

A "polka" for block B at round r means: `has_quorum(prevotes for B at round r)`.

A "polka for nil" means: `has_quorum(prevotes for nil at round r)`.

A "polka any" means: `sum(all prevotes at round r) >= quorum_threshold` (used for timeout_prevote trigger).

---

## 10. Equivocation Detection

If a validator sends two different votes for the same (height, round, step), this is equivocation.

```
Evidence:
    vote_a: SignedVote
    vote_b: SignedVote
    # Where vote_a.validator == vote_b.validator
    #   AND vote_a.height == vote_b.height
    #   AND vote_a.round == vote_b.round
    #   AND vote_a.vote_type == vote_b.vote_type
    #   AND vote_a.block_hash != vote_b.block_hash
```

Evidence is:
1. Recorded locally.
2. Gossiped to peers.
3. Included in future block proposals.
4. Processed during EndBlock (MVP: logged; future: slashing).

---

## 11. Consensus Core — Event Processing Loop

```
loop {
    event = inbound_channel.recv()    // blocking recv, NOT async

    match event {
        ProposalReceived(p)     => handle_proposal(p),
        VoteReceived(v)         => handle_vote(v),
        TimeoutPropose(h, r)    => handle_timeout_propose(h, r),
        TimeoutPrevote(h, r)    => handle_timeout_prevote(h, r),
        TimeoutPrecommit(h, r)  => handle_timeout_precommit(h, r),
        BlockExecuted(result)   => handle_block_executed(result),
        TxsAvailable(txs)      => handle_txs_available(txs),
    }

    // After processing, emit any pending commands
    flush_outbound_commands()
}
```

**No `.await` anywhere in this loop.** The `recv()` call is a synchronous blocking receive on a crossbeam channel (not a Tokio channel).

---

## 12. Consensus Core — Interaction Diagram

```
    P2P          Timers        Consensus Core       State Machine     Storage
     │             │                │                     │              │
     │──Proposal──▶│                │                     │              │
     │             │                │                     │              │
     │             │◀──ScheduleTO───│                     │              │
     │             │                │                     │              │
     │◀──Prevote───────────────────│                     │              │
     │             │                │                     │              │
     │──Prevotes──▶│                │                     │              │
     │             │                │                     │              │
     │◀──Precommit─────────────────│                     │              │
     │             │                │                     │              │
     │──Precommits▶│                │                     │              │
     │             │                │                     │              │
     │             │                │──ExecuteBlock──────▶│              │
     │             │                │                     │──Persist────▶│
     │             │                │◀──BlockExecuted─────│              │
     │             │                │                     │              │
     │             │                │──PersistBlock──────────────────────▶│
     │             │                │                     │              │
     │             │                │ (advance height)    │              │
```

---

## 13. Height Lifecycle Summary

```
Height H:
  1. Load validator set for H (from state at H-1)
  2. Round 0:
     a. Propose (or timeout)
     b. Prevote (block or nil)
     c. Precommit (block or nil)
     d. If commit: execute block, persist, go to Height H+1
     e. If timeout: go to Round 1
  3. Round 1..N: repeat (a)-(e)
  4. On commit:
     - Execute block (BeginBlock, DeliverTx×N, EndBlock)
     - Compute state root
     - Apply validator updates → new validator set for H+1
     - Persist block + state
     - Advance to Height H+1, Round 0
```

---

## Definition of Done — Consensus

- [x] Three-phase round structure fully specified
- [x] Prevote and precommit rules with pseudocode
- [x] Locking mechanism explained with safety argument
- [x] Proposer selection algorithm defined
- [x] Timeout strategy with concrete defaults
- [x] Message types specified
- [x] Vote aggregation with integer-only quorum math
- [x] Equivocation detection defined
- [x] Event processing loop described (no .await)
- [x] Interaction diagram between components
- [x] Height lifecycle documented
- [x] No Rust source code included
