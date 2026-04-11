# Block Commit Sequence (Happy Path)

Source: `src/consensus/state.rs`, `src/router/mod.rs`, `src/state/executor.rs`, `src/storage/block_store.rs`

## Proposer Path — Single Round

```mermaid
sequenceDiagram
    participant Net as P2P Network
    participant Consensus as ConsensusCore
    participant Router as CommandRouter
    participant Timer as TimerService
    participant Executor as StateExecutor
    participant Store as BlockStore and StateStore
    participant WAL as WAL

    Note over Consensus: Step::NewHeight, enter_propose()

    Consensus->>Router: ReapTxs
    Router-->>Consensus: TxsAvailable (empty in MVP)
    Note over Consensus: build Block, create SignedProposal

    Consensus->>Router: BroadcastProposal
    Consensus->>Router: WriteWAL Proposal
    Router->>WAL: write_entry Proposal
    Router->>Net: fanout to all peers

    Note over Consensus: enter_prevote(), compute_prevote() returns block_hash
    Consensus->>Router: BroadcastVote Prevote
    Consensus->>Router: WriteWAL Prevote
    Router->>WAL: write_entry Prevote
    Router->>Net: fanout Prevote

    Net-->>Consensus: VoteReceived Prevote x N
    Note over Consensus: check_prevote_quorums(), polka reached, enter_precommit()

    Note over Consensus: compute_precommit(), lock block, return block_hash
    Consensus->>Router: BroadcastVote Precommit
    Consensus->>Router: WriteWAL Precommit
    Router->>WAL: write_entry Precommit
    Router->>Net: fanout Precommit

    Net-->>Consensus: VoteReceived Precommit x N
    Note over Consensus: check_commit_quorum(), quorum reached, commit_block_hash()
    Note over Consensus: step = Commit, cancel timeouts

    Consensus->>Router: CancelTimeout
    Router->>Timer: TimerCommand Cancel
    Consensus->>Router: ExecuteBlock

    rect rgb(220, 235, 255)
        Note over Router,Executor: Block Execution, write-lock on AppState
        Router->>Executor: execute_block(state, contracts, block)
        Note over Executor: BeginBlock, no-op in MVP
        Note over Executor: DeliverTx for each tx
        Note over Executor: EndBlock, collect ValidatorUpdates
        Note over Executor: compute_state_root via Merkle tree
        Executor-->>Router: state_root and validator_updates
    end

    Router-->>Consensus: BlockExecuted with height, state_root, validator_updates

    Consensus->>Router: PersistBlock

    rect rgb(220, 255, 220)
        Note over Router,Store: Persistence
        Router->>Store: block_store.save_block
        Router->>Store: state_store.save_state
        Router->>WAL: wal.truncate()
    end

    Note over Consensus: height += 1, apply validator_updates, enter_new_height()
```

## Timeout Path (No Proposal or No Quorum)

```mermaid
sequenceDiagram
    participant Consensus as ConsensusCore
    participant Router as CommandRouter
    participant Timer as TimerService

    Consensus->>Router: ScheduleTimeout Propose
    Router->>Timer: TimerCommand Schedule

    Note over Timer: timeout_propose_ms + round x delta_ms elapses
    Timer-->>Consensus: TimeoutPropose

    Note over Consensus: handle_timeout_propose, enter_prevote with nil
    Consensus->>Router: BroadcastVote Prevote nil

    Consensus->>Router: ScheduleTimeout Prevote
    Timer-->>Consensus: TimeoutPrevote
    Note over Consensus: enter_precommit with nil
    Consensus->>Router: BroadcastVote Precommit nil

    Consensus->>Router: ScheduleTimeout Precommit
    Timer-->>Consensus: TimeoutPrecommit
    Note over Consensus: enter_round(round + 1), new Propose phase
```

> **Verified against:** `src/consensus/state.rs` — full `run()` loop; `src/router/mod.rs` — `ExecuteBlock`, `PersistBlock`, `WriteWAL` arms; `src/state/executor.rs` — `execute_block()`.
