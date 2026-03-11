# Feature: BFT Consensus Core

## 1. Purpose

The Consensus Core is the safety-critical heart of RustBFT. It implements a three-phase, round-based Byzantine Fault Tolerant protocol (Propose → Prevote → Precommit → Commit) that guarantees deterministic finality: once a block is committed, it is final and irreversible.

The module runs on a single dedicated OS thread with no `.await`, no Tokio dependency, and no shared mutable state. Events arrive via a blocking `crossbeam::channel::recv()` call; commands are emitted on outbound channels. This design eliminates entire classes of concurrency bugs from the most critical path in the system.

The protocol tolerates up to `f < n/3` Byzantine validators without violating safety, and maintains liveness when more than `2n/3` of voting power is honest and connected.

## 2. Responsibilities

- Maintain consensus state `(height, round, step, locked_block, valid_block)`
- Process all consensus events: `ProposalReceived`, `VoteReceived`, `Timeout*`, `BlockExecuted`, `TxsAvailable`
- Emit consensus commands: `BroadcastProposal`, `BroadcastVote`, `ExecuteBlock`, `ReapTxs`, `EvictTxs`, `PersistBlock`, `WriteWAL`, `ScheduleTimeout`, `CancelTimeout`
- Accumulate votes in a deterministic `BTreeMap`-backed `VoteSet` with integer-only quorum math
- Implement the locking mechanism: lock on polka, unlock on nil polka, refuse to prevote conflicting block
- Select proposer via deterministic weighted round-robin over the current `ValidatorSet`
- Detect equivocation: two different votes from same validator for same `(height, round, step)`
- Handle cross-state transitions: commit from any step when `>2/3` precommits arrive, round skip on `>1/3` votes from higher round
- Emit `WALEntry` before every state-mutating action
- Advance to the next height after `BlockExecuted` and apply validator set updates

## 3. Non-Responsibilities

- Does not import `tokio`, `state`, `storage`, or `contracts` modules
- Does not call storage or network functions directly — only emits commands
- Does not manage timers — only emits `ScheduleTimeout` / `CancelTimeout` commands
- Does not execute transactions or compute state roots
- Does not persist WAL entries — only emits `WriteWAL` commands

## 4. Architecture Design

```
+---------------------------------------------------+
|         Async Outer Shell (Tokio Runtime)          |
|  P2P Layer   Timer Service   Mempool   RPC         |
+---------+-----------+-----------+------------------+
          |           |           |
     crossbeam bounded mpsc channels (inbound)
          |           |           |
+---------v-----------v-----------v------------------+
|         Consensus Core Thread (std::thread)        |
|                                                    |
|  loop {                                            |
|      event = inbound.recv()   // blocking          |
|      commands = process_event(&mut state, event)   |
|      flush_commands(commands) // non-blocking send |
|  }                                                 |
|                                                    |
|  Owns: ConsensusState  (no Arc, no Mutex)          |
|  No .await  No tokio::  No shared state            |
+----+----------------------------+------------------+
     |                            |
 crossbeam outbound          crossbeam outbound
 (to P2P, Storage,           (to State Machine)
  Mempool, Timers)
```

## 5. Core Data Structures (Rust)

```rust
// src/consensus/state.rs

pub struct ConsensusState {
    pub height: u64,
    pub round: u32,
    pub step: Step,
    pub locked_round: Option<u32>,
    pub locked_block: Option<Hash>,
    pub valid_round: Option<u32>,
    pub valid_block: Option<Hash>,
    pub proposal: Option<SignedProposal>,
    pub votes: VoteSet,
    pub validator_set: ValidatorSet,
    pub proposer_state: ProposerState,  // for weighted round-robin
    pub node_id: ValidatorId,
    pub private_key: Ed25519PrivateKey,
}

pub enum Step { NewHeight, Propose, Prevote, Precommit, Commit }

// src/consensus/vote_set.rs

pub struct VoteSet {
    height: u64,
    // Key: (round, VoteType, ValidatorId) → deterministic BTreeMap
    votes: BTreeMap<(u32, VoteType, ValidatorId), SignedVote>,
    // Accumulated voting power per (round, type, block_hash)
    power_for: BTreeMap<(u32, VoteType, Option<Hash>), u64>,
    total_power: u64,
    evidence: Vec<EquivocationEvidence>,
}

pub struct EquivocationEvidence {
    pub vote_a: SignedVote,
    pub vote_b: SignedVote,
}

// src/consensus/proposer.rs

pub struct ProposerState {
    // Priority accumulator per validator, BTreeMap for determinism
    priorities: BTreeMap<ValidatorId, i64>,
}

// src/consensus/events.rs

pub enum ConsensusEvent {
    EnterRound { height: u64, round: u32 },
    ProposalReceived { proposal: SignedProposal },
    VoteReceived { vote: SignedVote },
    TimeoutPropose { height: u64, round: u32 },
    TimeoutPrevote { height: u64, round: u32 },
    TimeoutPrecommit { height: u64, round: u32 },
    BlockExecuted { height: u64, state_root: Hash, validator_updates: Vec<ValidatorUpdate> },
    TxsAvailable { txs: Vec<SignedTransaction> },
}

pub enum ConsensusCommand {
    BroadcastProposal { proposal: SignedProposal },
    BroadcastVote { vote: SignedVote },
    ExecuteBlock { block: Block },
    ReapTxs { max_bytes: usize, responder: oneshot::Sender<Vec<SignedTransaction>> },
    EvictTxs { tx_hashes: Vec<Hash> },
    PersistBlock { block: Block },
    WriteWAL { entry: WALEntry },
    ScheduleTimeout { kind: TimeoutKind, height: u64, round: u32, duration_ms: u64 },
    CancelTimeout { kind: TimeoutKind, height: u64, round: u32 },
    RecordEvidence { evidence: EquivocationEvidence },
}

pub enum WALEntry {
    RoundStarted { height: u64, round: u32 },
    ProposalSent { height: u64, round: u32, block_hash: Hash },
    VoteSent { height: u64, round: u32, vote_type: VoteType, block_hash: Option<Hash> },
    Locked { height: u64, round: u32, block_hash: Hash },
    Unlocked { height: u64, round: u32 },
    CommitStarted { height: u64, block_hash: Hash },
    CommitCompleted { height: u64, state_root: Hash },
}

pub enum TimeoutKind { Propose, Prevote, Precommit }
```

## 6. Public Interfaces

```rust
// src/consensus/mod.rs

/// Start the consensus core on a dedicated OS thread.
/// The thread owns ConsensusState exclusively; no other thread can access it.
pub fn run(
    inbound: crossbeam::channel::Receiver<ConsensusEvent>,
    outbound: crossbeam::channel::Sender<ConsensusCommand>,
    initial_state: ConsensusState,
) -> std::thread::JoinHandle<()>;

/// Build the initial ConsensusState from genesis config and WAL recovery output.
pub fn initial_state(
    genesis: &GenesisConfig,
    node_id: ValidatorId,
    private_key: Ed25519PrivateKey,
    height: u64,
    round: u32,
) -> ConsensusState;

// VoteSet public API
impl VoteSet {
    pub fn add_vote(&mut self, vote: SignedVote, voting_power: u64) -> VoteAddResult;
    pub fn has_polka(&self, round: u32, block_hash: Hash) -> bool;
    pub fn has_polka_nil(&self, round: u32) -> bool;
    pub fn has_polka_any(&self, round: u32) -> bool;
    pub fn has_commit(&self, round: u32, block_hash: Hash) -> bool;
    pub fn has_precommit_any(&self, round: u32) -> bool;
    pub fn quorum_threshold(&self) -> u64;  // (total_power * 2 / 3) + 1
    pub fn take_evidence(&mut self) -> Vec<EquivocationEvidence>;
}

// ProposerState public API
impl ProposerState {
    pub fn new() -> Self;
    pub fn select(&mut self, validator_set: &ValidatorSet) -> ValidatorId;
    pub fn proposer_for(height: u64, round: u32, validator_set: &ValidatorSet) -> ValidatorId;
}
```

## 7. Internal Algorithms

### Event Processing Loop
```
fn run(inbound, outbound, initial_state):
    state = initial_state
    // Kick off first round
    handle_enter_round(&mut state, state.height, 0)
    loop:
        event = inbound.recv()   // BLOCKING — parks thread when idle (zero CPU)
        match event:
            Err(_) => break       // channel closed → shutdown
            Ok(e)  => process_event(&mut state, e, &mut pending_cmds)
        // Drain additional events without blocking (batch for efficiency)
        while let Ok(e) = inbound.try_recv():
            process_event(&mut state, e, &mut pending_cmds)
        // Send all pending commands
        for cmd in pending_cmds.drain(..):
            outbound.send(cmd)
```

### Prevote Decision
```
fn decide_prevote(state, proposal):
    if proposal is None OR proposal is invalid:
        return nil

    block = proposal.block
    if state.locked_block is None:
        return block.hash
    if state.locked_block == Some(block.hash):
        return block.hash
    if proposal.valid_round >= 0
       AND proposal.valid_round >= state.locked_round
       AND votes.has_polka(proposal.valid_round, block.hash):
        return block.hash
    return nil
```

### Polka Detection → Precommit
```
fn on_polka(state, round, block_hash):
    state.valid_block = Some(block_hash)
    state.valid_round = Some(round)
    if have_full_block AND block_is_valid:
        state.locked_block = Some(block_hash)
        state.locked_round = Some(round)
        emit WriteWAL(Locked)
        emit BroadcastVote(Precommit(block_hash))
    else:
        emit BroadcastVote(Precommit(nil))
    emit CancelTimeout(Prevote)

fn on_polka_nil(state, round):
    state.locked_block = None
    state.locked_round = None
    emit WriteWAL(Unlocked)
    emit BroadcastVote(Precommit(nil))
    emit CancelTimeout(Prevote)
```

### Commit Detection
```
fn check_commit(state):
    for each round R where votes.has_commit(R, B):
        emit WriteWAL(CommitStarted)
        emit CancelTimeout for all pending timeouts at state.height
        emit ExecuteBlock(B)
        state.step = Commit
        return
```

### Round Skip
```
fn check_round_skip(state, incoming_round):
    if incoming_round > state.round:
        // Count total votes (any type) at incoming_round
        if total_vote_power_at_round(incoming_round) > total_power / 3:
            emit EnterRound(state.height, incoming_round)
```

### Weighted Round-Robin Proposer Selection
```
fn select(priorities, validator_set) -> ValidatorId:
    // Step 1: add voting_power to each validator's priority
    for (id, v) in validator_set.validators.iter():
        priorities[id] += v.voting_power as i64
    // Step 2: pick highest priority (BTreeMap sorted → stable tie-breaking by id)
    winner = priorities.iter().max_by_key(|(_, &p)| p).unwrap().0
    // Step 3: subtract total power from winner
    priorities[winner] -= validator_set.total_power as i64
    return winner

fn proposer_for(height, round, validator_set) -> ValidatorId:
    // Run from scratch: (height - 1) * estimated_rounds_per_height + round steps
    let mut ps = ProposerState::new()
    for _ in 0..((height - 1) * MAX_ROUNDS + round + 1):
        ps.select(validator_set)
    return last_selected
```

### Timeout Durations
```
fn timeout_propose(round: u32) -> u64  { 1000 + round as u64 * 500 }
fn timeout_prevote(round: u32) -> u64  { 1000 + round as u64 * 500 }
fn timeout_precommit(round: u32) -> u64 { 1000 + round as u64 * 500 }
```

### Quorum Threshold (Integer Math Only)
```
fn quorum_threshold(total_power: u64) -> u64 {
    (total_power * 2 / 3) + 1   // integer division, no floats
}
```

## 8. Persistence Model

The consensus core does not persist data directly. It emits `WriteWAL(entry)` commands before every state-mutating action (vote sent, lock/unlock, commit). The storage thread performs the actual fsync. On restart, WAL recovery reconstitutes `locked_block`, `locked_round`, and current `(height, round)` before the consensus core resumes.

## 9. Concurrency Model

The `ConsensusState` struct is moved into the consensus thread via `std::thread::spawn(move || ...)`. Rust's ownership system guarantees no other thread can access it. There is no `Arc`, no `Mutex`, and no `RwLock` anywhere in this module.

All cross-thread communication uses bounded `crossbeam::channel`:
- Inbound: single `Receiver<ConsensusEvent>` (P2P, Timers, Mempool, State Machine all send to it)
- Outbound: single `Sender<ConsensusCommand>` fanned out to appropriate consumers by the node binary

The module contains zero `tokio` imports. Adding `.await` anywhere in `src/consensus/` is a compile error by convention enforced via code review and CI.

## 10. Configuration

```toml
[consensus]
timeout_propose_ms = 1000
timeout_prevote_ms = 1000
timeout_precommit_ms = 1000
timeout_delta_ms = 500
create_empty_blocks = true
```

## 11. Observability

- Emit `WriteWAL` before every critical action (WAL provides audit trail)
- Metrics emitted via the shared metrics module (not directly — commands route to metrics layer):
  - `rustbft_consensus_height` (Gauge)
  - `rustbft_consensus_round` (Gauge)
  - `rustbft_consensus_step` (Gauge)
  - `rustbft_consensus_block_commit_duration_seconds` (Histogram)
  - `rustbft_consensus_rounds_per_height` (Histogram)
  - `rustbft_consensus_equivocations_total` (Counter)
- Log at DEBUG for every state transition; INFO for each committed block; ERROR for equivocation

## 12. Testing Strategy

- **`test_happy_path_4_validators`**: inject Proposal + 4 Prevotes + 4 Precommits, assert `ExecuteBlock` emitted, assert step = `Commit`
- **`test_proposer_timeout`**: no Proposal arrives → `TimeoutPropose` fires → assert `BroadcastVote(Prevote(nil))` emitted
- **`test_locking_refuses_conflicting_block`**: polka for A in round 0, lock; round 1 proposes B → assert prevote(nil) for B
- **`test_unlock_on_nil_polka`**: locked on A; inject nil polka → assert `WriteWAL(Unlocked)` emitted, prevote for B accepted
- **`test_equivocation_detection`**: two different prevotes from same validator → `RecordEvidence` emitted, vote counted once
- **`test_round_skip`**: inject votes from round 5 with >1/3 power → assert `EnterRound(height, 5)` emitted
- **`test_commit_from_propose_step`**: inject `>2/3` precommits while in Propose step → assert commit proceeds
- **`test_invalid_proposal_bad_signature`**: inject proposal with wrong signature → assert `BroadcastVote(Prevote(nil))`
- **`test_duplicate_vote_not_double_counted`**: same vote injected twice → `quorum_threshold` computed correctly
- **`test_stale_event_dropped`**: event for old height → state unchanged
- **`test_proposer_selection_deterministic`**: same `(height, round, ValidatorSet)` → always same proposer
- **`test_quorum_integer_math`**: 267/400 < quorum (268); 268/400 ≥ quorum; no floats anywhere
- **`test_safety_no_double_commit`** (integration): 4 in-process consensus cores, 100 heights, assert no height committed twice with different blocks
- **`test_liveness_3_of_4`** (integration): one core offline, assert remaining 3 commit continuously
- **`test_deterministic_replay`**: record event sequence, replay into fresh state, assert identical command sequence

## 13. Open Questions

None.
