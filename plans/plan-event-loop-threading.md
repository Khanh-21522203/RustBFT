# Feature: Event Loop & Threading Model

## 1. Purpose

The Event Loop & Threading module defines the hybrid async/sync execution model that is the structural backbone of the entire node. It establishes which code runs where, how modules communicate across thread boundaries, and the rules that keep the deterministic consensus core free of async runtimes, shared mutable state, and non-deterministic scheduling. Getting this model right is a precondition for both safety and testability.

## 2. Responsibilities

- Spawn and own every OS thread in the node process (consensus, state machine, storage, pruning)
- Initialise the Tokio async runtime for I/O-bound components (P2P, RPC, timer service, metrics)
- Create all bounded `crossbeam` channels that connect the Tokio layer to the sync consensus core
- Implement the timer service: receive `ScheduleTimeout`/`CancelTimeout` commands from the consensus core and deliver `Timeout*` events back
- Enforce the rule: no `tokio` import anywhere in `src/consensus/` — the consensus core is fully synchronous
- Route `ConsensusCommand`s emitted by the consensus core to the correct downstream consumer (P2P, storage, state machine, mempool)
- Implement graceful shutdown: drain in-flight requests, stop Tokio tasks, join all threads
- Monitor channel depths and log errors if a sync thread blocks for more than a threshold duration

## 3. Non-Responsibilities

- Does not implement consensus logic, transaction execution, or storage writes
- Does not manage TCP connections directly — that is the P2P module's job
- Does not manage the WAL or database files — that is the storage module's job
- Does not parse or validate messages from the network

## 4. Architecture Design

```
┌───────────────────────────────────────────────────────────────┐
│  Tokio Runtime (multi-threaded, src/node/async_shell.rs)       │
│                                                                │
│  ┌──────────┐  ┌──────────┐  ┌────────────┐  ┌───────────┐   │
│  │ P2P      │  │ RPC      │  │  Timer     │  │  Metrics  │   │
│  │ per-peer │  │ per-req  │  │  Service   │  │  Server   │   │
│  │ tasks    │  │ handlers │  │  (Tokio)   │  │  :26660   │   │
│  └─────┬────┘  └────┬─────┘  └─────┬──────┘  └───────────┘   │
│        │            │              │                          │
│  crossbeam::channel::Sender<ConsensusEvent> clones            │
└────────┼────────────┼──────────────┼──────────────────────────┘
         │            │              │
         └────────────▼──────────────┘
                 inbound channel (cap=256)
                      │
┌─────────────────────▼─────────────────────────────────────────┐
│  Consensus Core Thread  (std::thread::spawn)                   │
│  src/consensus/mod.rs                                          │
│                                                                │
│  loop {                                                        │
│      event = inbound.recv()          // blocking               │
│      process_event(&mut state, event)                          │
│      while let Ok(e) = inbound.try_recv() { process(e) }      │
│      flush_commands(outbound)        // non-blocking           │
│  }                                                             │
│                                                                │
│  Owns: ConsensusState (no Arc, no Mutex, no .await)            │
└─────────────────────┬─────────────────────────────────────────┘
                      │ outbound ConsensusCommand channel
                      │
┌─────────────────────▼─────────────────────────────────────────┐
│  Command Router  (node binary, main thread)                    │
│  Routes each ConsensusCommand to the correct consumer:         │
│    BroadcastProposal/Vote  → P2P outbound channel              │
│    ExecuteBlock            → State Machine channel             │
│    PersistBlock, WriteWAL  → Storage channel                   │
│    ReapTxs, EvictTxs       → Mempool channel                   │
│    ScheduleTimeout         → Timer Service channel             │
│    RecordEvidence          → P2P Evidence channel              │
└─────────────────────────────────────────────────────────────────┘
```

## 5. Core Data Structures (Rust)

```rust
// src/node/threads.rs

pub struct NodeThreads {
    pub consensus_handle: std::thread::JoinHandle<()>,
    pub state_machine_handle: std::thread::JoinHandle<()>,
    pub storage_handle: std::thread::JoinHandle<()>,
    pub pruning_handle: std::thread::JoinHandle<()>,
    pub tokio_runtime: tokio::runtime::Runtime,
}

pub struct ChannelSet {
    // Inbound to consensus core
    pub consensus_inbound_tx: crossbeam::channel::Sender<ConsensusEvent>,
    pub consensus_inbound_rx: crossbeam::channel::Receiver<ConsensusEvent>,

    // Outbound from consensus core
    pub consensus_outbound_tx: crossbeam::channel::Sender<ConsensusCommand>,
    pub consensus_outbound_rx: crossbeam::channel::Receiver<ConsensusCommand>,

    // State machine
    pub execute_block_tx: crossbeam::channel::Sender<ExecuteBlockCommand>,
    pub execute_block_rx: crossbeam::channel::Receiver<ExecuteBlockCommand>,

    // Storage
    pub storage_tx: crossbeam::channel::Sender<StorageCommand>,

    // Mempool
    pub mempool_tx: crossbeam::channel::Sender<MempoolCommand>,

    // Timer service (Tokio mpsc: consensus sends commands, timer sends events)
    pub timer_cmd_tx: tokio::sync::mpsc::Sender<TimerCommand>,
}

// Channel capacities
pub const CAP_CONSENSUS_INBOUND: usize = 256;
pub const CAP_CONSENSUS_OUTBOUND: usize = 64;
pub const CAP_EXECUTE_BLOCK: usize = 4;
pub const CAP_STORAGE: usize = 16;
pub const CAP_MEMPOOL_RPC: usize = 256;
pub const CAP_TIMER: usize = 16;

// Timer service types
pub enum TimerCommand {
    Schedule { kind: TimeoutKind, height: u64, round: u32, duration_ms: u64 },
    Cancel   { kind: TimeoutKind, height: u64, round: u32 },
}
```

## 6. Public Interfaces

```rust
// src/node/threads.rs

/// Allocate all channels with their configured capacities.
pub fn create_channels() -> ChannelSet;

/// Spawn the consensus core on a dedicated OS thread.
/// The ConsensusState is moved into the thread; no other code can access it.
pub fn spawn_consensus_thread(
    inbound:  crossbeam::channel::Receiver<ConsensusEvent>,
    outbound: crossbeam::channel::Sender<ConsensusCommand>,
    state:    ConsensusState,
) -> std::thread::JoinHandle<()>;

/// Spawn the state machine on a dedicated OS thread.
pub fn spawn_state_machine_thread(
    execute_rx: crossbeam::channel::Receiver<ExecuteBlockCommand>,
    result_tx:  crossbeam::channel::Sender<ConsensusEvent>,  // sends BlockExecuted
    executor:   StateExecutor,
) -> std::thread::JoinHandle<()>;

/// Spawn the storage thread.
pub fn spawn_storage_thread(
    storage_rx: crossbeam::channel::Receiver<StorageCommand>,
    config:     StorageConfig,
) -> std::thread::JoinHandle<()>;

/// Start the timer service inside the Tokio runtime.
pub fn start_timer_service(
    cmd_rx:    tokio::sync::mpsc::Receiver<TimerCommand>,
    event_tx:  crossbeam::channel::Sender<ConsensusEvent>,
);

/// Start the command router loop (runs on the main thread or a dedicated thread).
/// Reads ConsensusCommands and fans them out to the correct consumers.
pub fn run_command_router(
    outbound: crossbeam::channel::Receiver<ConsensusCommand>,
    channels: &ChannelSet,
    p2p_cmd_tx: tokio::sync::mpsc::Sender<P2PCommand>,
);

/// Initiate and wait for graceful shutdown.
pub fn shutdown(threads: NodeThreads, timeout: Duration);
```

## 7. Internal Algorithms

### Consensus Core Event Loop
```
fn run_consensus(inbound, outbound, mut state):
    // Emit initial EnterRound to kick off height 1
    outbound.send(ConsensusCommand::ScheduleTimeout { ... })

    let mut pending: Vec<ConsensusCommand> = Vec::new()

    loop:
        // 1. Blocking recv — parks thread with zero CPU when idle
        event = match inbound.recv():
            Ok(e)  => e
            Err(_) => break  // channel closed → shutdown

        // 2. Discard stale events
        if !is_relevant(&state, &event): continue

        // 3. Process synchronously
        process_event(&mut state, event, &mut pending)

        // 4. Batch drain — avoid round-trips for simultaneous arrivals
        while let Ok(e) = inbound.try_recv():
            if is_relevant(&state, &e):
                process_event(&mut state, e, &mut pending)

        // 5. Flush all commands accumulated during batch
        for cmd in pending.drain(..):
            outbound.send(cmd)  // blocks if full — downstream must keep up
```

### Timer Service
```
async fn run_timer_service(cmd_rx, event_tx):
    // Active timers: (kind, height, round) → JoinHandle<()>
    let mut active: HashMap<(TimeoutKind, u64, u32), AbortHandle> = HashMap::new()

    loop:
        cmd = cmd_rx.recv().await.unwrap_or_break
        match cmd:
            Schedule { kind, height, round, duration_ms }:
                cancel any existing timer for (kind, height, round)
                handle = tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(duration_ms)).await
                    let _ = event_tx.send(timeout_event(kind, height, round))
                })
                active.insert((kind, height, round), handle.abort_handle())
            Cancel { kind, height, round }:
                if let Some(handle) = active.remove(&(kind, height, round)):
                    handle.abort()   // timer cancelled — no event will fire
```

### Command Router
```
fn run_command_router(outbound_rx, channels, p2p_cmd_tx):
    loop:
        cmd = outbound_rx.recv() else break

        match cmd:
            BroadcastProposal { proposal } =>
                p2p_cmd_tx.blocking_send(P2PCommand::BroadcastProposal { proposal })
            BroadcastVote { vote } =>
                p2p_cmd_tx.blocking_send(P2PCommand::BroadcastVote { vote })
            ExecuteBlock { block } =>
                channels.execute_block_tx.send(ExecuteBlockCommand { block })
            PersistBlock { block } =>
                channels.storage_tx.send(StorageCommand::PersistBlock { block, ... })
            WriteWAL { entry } =>
                channels.storage_tx.send(StorageCommand::WriteWAL { entry })
            ReapTxs { max_bytes, responder } =>
                channels.mempool_tx.send(MempoolCommand::ReapTxs { max_bytes, responder })
            EvictTxs { tx_hashes } =>
                channels.mempool_tx.send(MempoolCommand::EvictTxs { tx_hashes })
            ScheduleTimeout { kind, height, round, duration_ms } =>
                channels.timer_cmd_tx.blocking_send(TimerCommand::Schedule { ... })
            CancelTimeout { kind, height, round } =>
                channels.timer_cmd_tx.blocking_send(TimerCommand::Cancel { ... })
            RecordEvidence { evidence } =>
                p2p_cmd_tx.blocking_send(P2PCommand::RecordEvidence { evidence })
```

### Graceful Shutdown Sequence
```
fn shutdown(threads, timeout):
    // 1. Stop Tokio tasks
    close p2p listener; send Disconnect to all peers
    stop RPC from accepting new requests; drain in-flight (5s)
    cancel all pending timers

    // 2. Close inbound channel → consensus core exits its recv() loop
    drop(channels.consensus_inbound_tx)

    // 3. Join consensus thread (1s timeout)
    consensus_handle.join_timeout(1s)

    // 4. Close execute_block channel → state machine exits
    drop(channels.execute_block_tx)
    state_machine_handle.join_timeout(10s)

    // 5. Send flush command to storage → sync WAL and DB
    storage_tx.send(StorageCommand::Flush)
    storage_handle.join_timeout(10s)

    // 6. Shutdown Tokio runtime (5s timeout for remaining tasks)
    tokio_runtime.shutdown_timeout(5s)

    // 7. Exit process
```

## 8. Persistence Model

The event loop and threading module itself has no persistent state. Its job is structural: wiring channels, spawning threads, routing commands. The channels are in-memory message queues that are rebuilt on every startup.

## 9. Concurrency Model

**Consensus core thread**: owns `ConsensusState` exclusively via Rust's move semantics. No `Arc`, no `Mutex`, no `Clone`. Blocked on `crossbeam::channel::recv()` when idle — zero CPU usage.

**State machine thread**: owns `StateExecutor` exclusively. Blocked on `crossbeam::channel::recv()`. Synchronous execution only.

**Storage thread**: owns the RocksDB write handle and the WAL file exclusively. Blocked on `crossbeam::channel::recv()`.

**Tokio runtime**: runs P2P tasks, RPC handlers, timer service, metrics server. All use `tokio::sync::mpsc` for intra-runtime communication. `crossbeam::channel::Sender::try_send` (non-blocking) bridges from Tokio tasks to sync threads; dropped messages increment `rustbft_channel_drops_total`.

**Channel direction invariant**: Tokio tasks may only use `try_send` (non-blocking) when sending to the consensus inbound channel. They must never block the async executor.

## 10. Configuration

```toml
[node]
command_router_channel_warning_threshold_ms = 5000  # Log error if consensus blocks this long

# Channel capacities (rarely need tuning)
channel_consensus_inbound_cap = 256
channel_consensus_outbound_cap = 64
channel_execute_block_cap = 4
channel_storage_cap = 16

# Shutdown timeouts
shutdown_rpc_drain_ms = 5000
shutdown_consensus_ms = 1000
shutdown_state_machine_ms = 10000
shutdown_storage_ms = 10000
shutdown_tokio_ms = 5000
```

## 11. Observability

- `rustbft_channel_length{channel}` (Gauge) — current pending items in each channel
- `rustbft_channel_capacity{channel}` (Gauge) — static capacity of each channel
- `rustbft_channel_drops_total{channel}` (Counter) — messages dropped due to full channel
- Log WARN `"Channel full, message dropped"` with `{channel, capacity}` on any `try_send` failure
- Log ERROR `"Consensus core blocked on outbound send"` if any outbound `send()` takes > 5s
- Log INFO at startup listing all thread IDs for debugging
- Log INFO at each shutdown step with elapsed time

## 12. Testing Strategy

- **`test_consensus_thread_isolation`**: construct ConsensusState, spawn thread, send events via channel, assert commands arrive — never access ConsensusState from outside thread
- **`test_timer_schedule_fires`**: schedule a timer for 50ms, assert timeout event arrives within 100ms
- **`test_timer_cancel_suppresses_event`**: schedule then immediately cancel a timer → no event arrives after 200ms
- **`test_timer_cancel_stale_height`**: schedule timer for (h=5, r=0), advance to h=6, cancel (h=5, r=0) → no event (already cancelled implicitly by new height)
- **`test_command_router_broadcast_proposal`**: send BroadcastProposal command → P2P channel receives P2PCommand::BroadcastProposal
- **`test_command_router_execute_block`**: send ExecuteBlock → state machine channel receives ExecuteBlockCommand
- **`test_channel_backpressure_try_send_fails`**: fill inbound channel, `try_send` → Err(Full) returned, no panic
- **`test_shutdown_drains_in_flight`**: start shutdown with one in-progress command → shutdown waits for it (up to timeout) before exiting
- **`test_consensus_exits_on_channel_close`**: drop all senders to inbound channel → consensus thread exits loop cleanly
- **`test_no_tokio_in_consensus`**: grep source of src/consensus/ for "tokio" → zero matches (compile-time enforcement)
- **`test_four_consensus_threads_in_process`**: spawn 4 consensus threads connected via in-memory channels, run 10 heights → all threads agree on same committed blocks

## 13. Open Questions

- **Command router on main thread vs dedicated thread**: The command router can run on the main thread (blocking) or be moved to a dedicated thread. For MVP, main thread is simplest. A dedicated thread allows better monitoring of router latency.
- **State machine result routing**: `BlockExecuted` from the state machine is sent back to the consensus inbound channel. If that channel is full (e.g., during a burst of votes), the state machine blocks. Channel capacity `4` for execute_block and `256` for consensus inbound should prevent this in practice.
