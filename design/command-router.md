# Command Router

## Purpose

Bridges the synchronous consensus core (which emits `ConsensusCommand` via crossbeam) to the async subsystems running on the Tokio runtime: P2P, timers, state machine, storage, and WAL.

## Scope

**In scope:**
- Receiving `ConsensusCommand` from `ConsensusCore` via bounded crossbeam channel
- Dispatching each command to the appropriate async subsystem
- Calling `StateExecutor::execute_block` synchronously and sending `BlockExecuted` back to consensus
- Persisting blocks and state after execution
- Truncating WAL after successful persist
- Answering `ReapTxs` with empty tx list (mempool placeholder)
- Updating shared `validator_set` after block execution
- Recording metrics for block execution and storage operations

**Out of scope:**
- Any consensus logic or state
- Direct P2P connection management (delegated to `P2pManager`)
- Timer scheduling logic (delegated to `TimerService`)

## Primary User Flow

1. `ConsensusCore` sends `ConsensusCommand` via crossbeam `tx_consensus_cmd`.
2. `CommandRouter::run` (spawned as Tokio task) blocks on `rx_cmd.recv()`.
3. Routes command to the appropriate subsystem.
4. For `ExecuteBlock`: executes synchronously, then sends `ConsensusEvent::BlockExecuted` back to consensus.
5. For `PersistBlock`: saves block + state, truncates WAL, logs height, updates `consensus_height` metric.
6. For `WriteWAL`: writes entry and records WAL write duration metric.

## System Flow

```
ConsensusCore (OS thread, crossbeam sender)
    │ ConsensusCommand
    ▼
CommandRouter::run (Tokio task, crossbeam receiver)
    │
    ├── BroadcastProposal / BroadcastVote
    │       → to_p2p.send(cmd).await   [P2pManager mpsc]
    │
    ├── ScheduleTimeout / CancelTimeout
    │       → to_timer.send(TimerCommand::Schedule/Cancel).await   [TimerService mpsc]
    │
    ├── ExecuteBlock { block }
    │       app_state.write().await           [RwLock write - blocks RPC reads]
    │       contracts.lock().await            [Mutex]
    │       executor.execute_block(state, contracts, block)
    │         → Ok((state_root, validator_updates))
    │             metrics.state_block_execution_duration.observe(elapsed)
    │             validator_set.write().await.apply_updates(...)
    │             to_consensus.send(BlockExecuted { height, state_root, validator_updates })
    │         → Err(e) → error!(...) [no retry, no halt]
    │
    ├── PersistBlock { block, state_root }
    │       block_store.save_block(block, state_root, &vs)
    │       metrics.storage_block_persist_duration.observe(elapsed)
    │       state_store.save_state(height, &state)
    │       wal.lock().await.truncate()
    │       info!("Block committed and persisted")
    │       metrics.consensus_height.set(height)
    │
    ├── WriteWAL { entry }
    │       wal.lock().await.write_entry(&entry)
    │       metrics.storage_wal_write_duration.observe(elapsed)
    │
    ├── ReapTxs { max_bytes }
    │       to_consensus.send(TxsAvailable { txs: vec![] })
    │       [mempool placeholder - always empty]
    │
    └── EvictTxs { tx_hashes }
            [no-op placeholder]
```

## Data Model

`CommandRouter` holds `Arc` references shared with other subsystems:
- `app_state: Arc<RwLock<AppState>>` — shared read access with RPC handlers
- `contracts: Arc<tokio::sync::Mutex<ContractRuntime>>` — exclusive access during execution
- `block_store: Arc<BlockStore>` — shared read access with RPC handlers
- `state_store: Arc<StateStore>` — write-only in router; `StateStore` has no async reads in RPC
- `wal: Arc<tokio::sync::Mutex<WAL>>` — exclusive access for writes and truncation
- `validator_set: Arc<RwLock<ValidatorSet>>` — shared read access with RPC
- `metrics: Arc<Metrics>` — shared with metrics server

## Interfaces and Contracts

**Input:** `crossbeam_channel::Receiver<ConsensusCommand>` — unbounded recv (blocks until command available).

**Channels to subsystems:**
- `to_p2p: mpsc::Sender<ConsensusCommand>` — Tokio mpsc to P2pManager
- `to_timer: mpsc::Sender<TimerCommand>` — Tokio mpsc to TimerService
- `to_consensus: crossbeam_channel::Sender<ConsensusEvent>` — back to ConsensusCore

**`CommandRouter` layer invariant:** consensus module must not import `state`, `storage`, or `contracts`. The router is the only bridge across this boundary.

## Dependencies

**Internal modules:**
- `src/consensus/events.rs` — `ConsensusCommand` (input), `ConsensusEvent` (output)
- `src/consensus/timer.rs` — `TimerCommand` for timer dispatch
- `src/state/executor.rs` — `StateExecutor::execute_block`
- `src/state/accounts.rs` — `AppState` (via `RwLock`)
- `src/contracts/runtime.rs` — `ContractRuntime` (via `Mutex`)
- `src/storage/block_store.rs` — `BlockStore`
- `src/storage/state_store.rs` — `StateStore`
- `src/storage/wal.rs` — `WAL` (via `Mutex`)
- `src/metrics/registry.rs` — `Metrics`
- `src/types/validator.rs` — `ValidatorSet` (via `RwLock`) + `ValidatorSetPolicy`

**External crates:**
- `crossbeam-channel` — `Receiver<ConsensusCommand>` blocking recv
- `tokio::sync` — `RwLock`, `Mutex`, `mpsc`
- `tracing` — `info!`, `warn!`, `error!`
- `anyhow` — error handling

## Failure Modes and Edge Cases

- **`execute_block` error:** Logs `error!` but does not send `BlockExecuted` back to consensus. ConsensusCore remains in `Step::Commit` indefinitely — **node hangs**.
- **`save_block` error:** Logged but not retried — block may not be persisted while consensus advances to the next height.
- **`save_state` error after `save_block` success:** Block is persisted but state snapshot is missing — partial persistence.
- **WAL truncate failure:** Logged as `warn!` but execution continues — WAL may grow unboundedly.
- **`to_p2p.send` failure:** Silently dropped (`.await` without error handling) — proposal/vote not broadcast.
- **`app_state.write()` contention:** Blocks all RPC account reads while `execute_block` runs.
- **`to_consensus.send(BlockExecuted)` failure (crossbeam full):** Returns `Err` but is silently ignored via `.ok()` — consensus does not receive the event and hangs in `Step::Commit`.

## Observability and Debugging

- `info!(height = height, "Block committed and persisted")` — main progress signal.
- `error!(height, error, "Block execution failed")` — critical failure.
- `error!(height, error, "Block persist failed")` — storage failure.
- `warn!(error, "WAL truncate failed")` — WAL warning.
- Debugging: `CommandRouter::run` at `src/router/mod.rs:run` is the central dispatch point.

## Risks and Notes

- `execute_block` error causes an irrecoverable hang — consensus never exits `Step::Commit`.
- `CommandRouter::run` uses `crossbeam_channel::recv()` which blocks the Tokio thread — should be `spawn_blocking` or replaced with a `try_recv` + `sleep` pattern.
- Validator set is updated twice: once in `CommandRouter` after `execute_block` and again in `ConsensusCore` after `BlockExecuted` — the `Arc<RwLock<ValidatorSet>>` in the router is separate from the `ValidatorSet` owned by `ConsensusCore`.
- `ReapTxs` always returns empty — `allow_empty_blocks = true` in default config means blocks are proposed anyway.

Changes:

