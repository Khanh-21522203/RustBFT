# Consensus BFT Engine

## Purpose

Implements the Tendermint BFT consensus algorithm: a deterministic, round-based protocol that achieves agreement among `f+1` of `3f+1` validators where `f` is the maximum number of Byzantine faults tolerated. Runs on a dedicated OS thread isolated from I/O.

## Scope

**In scope:**
- State machine for steps: `NewHeight → Propose → Prevote → Precommit → Commit`
- Vote aggregation and quorum detection (>2/3 of total voting power)
- Proposer selection via weighted round-robin
- Equivocation detection
- Timeout scheduling via `ConsensusCommand`
- WAL writes before every broadcast

**Out of scope:**
- Cryptographic signature verification (delegated to `ConsensusDeps` callbacks)
- Network I/O (handled by `P2pManager`)
- Block execution (handled by `StateExecutor` via `CommandRouter`)
- Storage persistence (handled by `BlockStore` via `CommandRouter`)

## Primary User Flow

1. Node starts, `ConsensusCore::new` is created with height and validator set.
2. `ConsensusCore::run` blocks on `rx: Receiver<ConsensusEvent>`.
3. For each height: enters `Propose` → waits for proposal/votes/timeouts → progresses through `Prevote → Precommit → Commit`.
4. On commit, sends `ConsensusCommand::ExecuteBlock`; waits for `ConsensusEvent::BlockExecuted`.
5. Advances to `height + 1`, applies any validator set updates.

## System Flow

```
ConsensusEvent (from P2P / timers / state machine)
    │
    ▼
ConsensusCore::process_event  (src/consensus/state.rs)
    ├── ProposalReceived  → handle_proposal → enter_prevote
    ├── VoteReceived      → insert into VoteSet → check quorums
    │       ├── check_any_round_commit   (cross-round precommit quorum)
    │       ├── check_round_skip         (>1/3 at higher round)
    │       ├── check_prevote_quorums    (polka → enter_precommit)
    │       └── check_commit_quorum
    ├── TimeoutPropose    → enter_prevote (prevote nil)
    ├── TimeoutPrevote    → enter_precommit (precommit nil)
    ├── TimeoutPrecommit  → enter_round(round+1)
    ├── TxsAvailable      → build Block → BroadcastProposal
    └── BlockExecuted     → PersistBlock → enter_new_height
```

```
Consensus step transitions:
  NewHeight
    └─► enter_propose
          ├── [is proposer] → ReapTxs → [TxsAvailable] → BroadcastProposal
          └── ScheduleTimeout(Propose)
    └─► enter_prevote  (on proposal OR TimeoutPropose)
          └── broadcast_vote(Prevote, compute_prevote())
    └─► enter_precommit (on polka OR TimeoutPrevote)
          └── broadcast_vote(Precommit, compute_precommit())
              ├── [polka found] → lock block, precommit hash
              └── [no polka / nil polka] → unlock, precommit nil
    └─► commit_block_hash → ExecuteBlock
    └─► [BlockExecuted] → PersistBlock → enter_new_height (height+1)
```

## Data Model

`ConsensusCore` (in-memory only, `src/consensus/state.rs`):
- `height: u64`, `round: u32`, `step: Step` — current position in protocol
- `locked_round: Option<u32>`, `locked_block: Option<Hash>` — safety lock
- `valid_round: Option<u32>`, `valid_block: Option<Hash>` — most recently polka'd block
- `proposal: Option<SignedProposal>` — current round's proposal
- `votes: VoteSet` — all votes for current height

`VoteSet` (`src/consensus/vote_set.rs`):
- `votes: BTreeMap<(u32, VoteType, ValidatorId), SignedVote>` — keyed by (round, type, validator)
- `evidence: Vec<Evidence>` — equivocating vote pairs collected during insertion
- `quorum_threshold(total) = (total * 2 / 3) + 1`

`ProposerState` (`src/consensus/proposer.rs`):
- `priorities: BTreeMap<ValidatorId, i128>` — incremental priority accumulator
- Each selection step: add voting_power to all, pick highest (tie-break by ValidatorId), subtract total_power
- `select_proposer(vset, height, round)` runs `height + round + 1` steps from zero state (stateless, recomputed each call)

## Interfaces and Contracts

**Input channel:** `crossbeam_channel::Receiver<ConsensusEvent>`:
- `ProposalReceived { proposal: SignedProposal }`
- `VoteReceived { vote: SignedVote }`
- `TimeoutPropose/Prevote/Precommit { height, round }`
- `TxsAvailable { txs: Vec<Vec<u8>> }` — response to `ReapTxs`
- `BlockExecuted { height, state_root, validator_updates }` — response to `ExecuteBlock`

**Output channel:** `crossbeam_channel::Sender<ConsensusCommand>`:
- `BroadcastProposal { proposal }` / `BroadcastVote { vote }`
- `ExecuteBlock { block }` / `PersistBlock { block, state_root }`
- `ReapTxs { max_bytes }` / `EvictTxs { tx_hashes }`
- `WriteWAL { entry }` / `ScheduleTimeout { timeout }` / `CancelTimeout { timeout }`

**`ConsensusDeps` (injected at construction, `src/consensus/state.rs:ConsensusDeps`):**
- `verify_proposal_sig`, `verify_vote_sig` — both currently `|_| true` (no-op in MVP)
- `validate_block`, `have_full_block`, `block_hash` — lightweight deterministic checks

## Dependencies

**Internal modules:**
- `src/consensus/vote_set.rs` — vote storage and quorum queries
- `src/consensus/proposer.rs` — deterministic proposer selection
- `src/consensus/timer.rs` — timeout scheduling via `TimerService`
- `src/storage/wal.rs` — `WalEntry` / `WalEntryKind` types used in `WriteWAL` commands

**External crates:**
- `crossbeam-channel` — bounded MPSC between consensus thread and async `CommandRouter`

## Failure Modes and Edge Cases

- **No proposal arrives in time:** `TimeoutPropose` fires → `enter_prevote` → prevote nil → likely precommit nil → `TimeoutPrecommit` → next round.
- **Split votes (no polka):** `TimeoutPrevote` fires → `enter_precommit` with nil → `TimeoutPrecommit` → next round with `round + 1`; timeout grows by `timeout_delta_ms` per round.
- **Round skip:** If >1/3 voting power seen at a higher round (via `check_round_skip`), jumps to that round immediately — prevents liveness stall.
- **Cross-round commit:** `check_any_round_commit` scans all rounds on every vote; a node that was behind can commit when it accumulates enough precommits.
- **Equivocation:** `VoteSet::insert_vote` returns `VoteSetError::Equivocation` and appends to `evidence`; currently logged to stderr — no on-chain slashing in MVP.
- **Validator set updates rejected:** `ValidatorSetPolicy` enforces min 1 validator, max 150, max 1/3 power change per block; violation keeps old validator set and prints to stderr.
- **Stale votes:** Votes for `height < self.height` are silently dropped; `height > self.height` are also dropped (block sync not yet implemented).
- **WAL write failure:** `WriteWAL` command sends `WalEntry` to `CommandRouter`; if WAL write fails, router logs error but consensus does not retry or halt.

## Observability and Debugging

- Equivocation logs to stderr via `eprintln!` at `src/consensus/state.rs:handle_vote`.
- Validator set rejection printed to stderr at `src/consensus/state.rs:handle_block_executed`.
- `Metrics.consensus_height` and `consensus_round` updated by `CommandRouter` after `PersistBlock`.
- Entry point for debugging: `ConsensusCore::process_event` at `src/consensus/state.rs:process_event`.

## Risks and Notes

- Signature verification callbacks are no-ops (`|_| true`) — any forged proposal/vote will be accepted in MVP.
- `select_proposer` recomputes from scratch every call — O(n × (height + round)); for tall chains this is expensive. A stateful `ProposerState` should be cached per height.
- `check_any_round_commit` scans rounds 0..=current+10 on every vote — linear scan that grows with round count.
- `block_hash` uses `serde_json::to_vec` which is non-deterministic across library versions; canonical encoding needed for production.
- No mempool: `ReapTxs` is answered with empty `txs` by `CommandRouter` — all blocks are empty in MVP.

Changes:

