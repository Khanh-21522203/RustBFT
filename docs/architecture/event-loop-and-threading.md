# RustBFT — Event Loop & Threading Model

**Purpose:** Define the hybrid async/sync execution model, thread responsibilities, channel topology, and the rules that enforce determinism in the consensus core.
**Audience:** Engineers implementing the node binary and understanding the concurrency architecture.

---

## 1. Design Rationale

### Why Hybrid?

BFT consensus requires **determinism**: given the same sequence of events, every node must make the same decisions. Async runtimes introduce non-determinism through task scheduling order, timer resolution, and interleaving. Therefore:

- **Consensus core** runs on a **single dedicated thread**, processing events **sequentially** with no `.await`, no async runtime, and no shared mutable state.
- **Everything else** (networking, RPC, timers, metrics) runs in the **Tokio async runtime**, which is well-suited for I/O-bound, concurrent workloads.

### Why Not Fully Sync?

A fully synchronous design would require the consensus core to poll sockets, manage timers, and handle HTTP requests — mixing I/O concerns with consensus logic. This violates layered separation and makes the consensus core untestable in isolation.

### Why Not Fully Async?

A fully async design would place the consensus state machine inside a Tokio task. Even with a single-task executor, the consensus logic would need to `.await` on channels, introducing:
- Non-deterministic wake ordering between multiple `select!` branches.
- Difficulty reasoning about event processing order.
- Risk of accidentally introducing concurrent access.

The hybrid model eliminates these risks by construction.

---

## 2. Thread Map

```
┌─────────────────────────────────────────────────────────────────┐
│                         PROCESS                                  │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │              Tokio Runtime (multi-threaded)                 │  │
│  │                                                            │  │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────────┐ │  │
│  │  │ P2P      │ │ RPC      │ │ Timer    │ │ Metrics      │ │  │
│  │  │ Listener │ │ Server   │ │ Service  │ │ Exporter     │ │  │
│  │  │          │ │          │ │          │ │              │ │  │
│  │  │ Per-peer │ │ Per-req  │ │ Timeout  │ │ Prometheus   │ │  │
│  │  │ tasks    │ │ handlers │ │ futures  │ │ /metrics     │ │  │
│  │  └────┬─────┘ └────┬─────┘ └────┬─────┘ └──────────────┘ │  │
│  │       │             │            │                         │  │
│  │       └─────────────┴────────────┘                         │  │
│  │                     │                                      │  │
│  │          bounded mpsc channels (crossbeam)                 │  │
│  │                     │                                      │  │
│  └─────────────────────┼──────────────────────────────────────┘  │
│                        │                                         │
│  ┌─────────────────────▼──────────────────────────────────────┐  │
│  │         Consensus Core Thread (std::thread::spawn)          │  │
│  │                                                             │  │
│  │   loop {                                                    │  │
│  │       event = inbound.recv()   // crossbeam blocking recv   │  │
│  │       process(event)           // pure synchronous logic    │  │
│  │       flush_commands()         // send to outbound channels │  │
│  │   }                                                         │  │
│  │                                                             │  │
│  │   Owns: ConsensusState (no Arc, no Mutex)                   │  │
│  │   No .await, no async, no Tokio dependency                  │  │
│  │                                                             │  │
│  └─────────────────────┬──────────────────────────────────────┘  │
│                        │                                         │
│          bounded mpsc channels (crossbeam)                       │
│                        │                                         │
│  ┌─────────────────────▼──────────────────────────────────────┐  │
│  │         State Machine / Execution Thread                    │  │
│  │                                                             │  │
│  │   Receives: ExecuteBlock commands                           │  │
│  │   Executes: BeginBlock → DeliverTx × N → EndBlock → Commit │  │
│  │   Returns: BlockExecutionResult                             │  │
│  │                                                             │  │
│  │   Synchronous, deterministic, no .await                     │  │
│  │                                                             │  │
│  └─────────────────────┬──────────────────────────────────────┘  │
│                        │                                         │
│  ┌─────────────────────▼──────────────────────────────────────┐  │
│  │         Storage Thread                                      │  │
│  │                                                             │  │
│  │   Receives: PersistBlock, WriteWAL commands                 │  │
│  │   Executes: RocksDB writes, WAL fsync                       │  │
│  │   Returns: completion acknowledgments                       │  │
│  │                                                             │  │
│  └─────────────────────────────────────────────────────────────┘  │
│                                                                  │
│  ┌─────────────────────────────────────────────────────────────┐  │
│  │         Pruning Thread (background, low priority)           │  │
│  │                                                             │  │
│  │   Periodically prunes old state versions                    │  │
│  │   Does not interact with consensus or execution             │  │
│  │                                                             │  │
│  └─────────────────────────────────────────────────────────────┘  │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

---

## 3. Channel Topology

### 3.1 Channel Types

| Channel | Library | Reason |
|---------|---------|--------|
| Tokio ↔ Consensus Core | `crossbeam::channel` (bounded mpsc) | Consensus core uses blocking `recv()`. Tokio tasks use non-blocking `send()` or `try_send()`. crossbeam channels work across async/sync boundaries. |
| Consensus Core → State Machine | `crossbeam::channel` (bounded mpsc) | Both are sync threads. |
| Consensus Core → Storage | `crossbeam::channel` (bounded mpsc) | Both are sync threads. |
| Within Tokio (e.g., P2P → PeerManager) | `tokio::sync::mpsc` (bounded) | Both ends are async tasks. |
| Request-response (e.g., Mempool reap) | `crossbeam::channel` or `oneshot` | Consensus sends request + oneshot sender; mempool responds on the oneshot. |

### 3.2 Channel Capacities

| Channel | Capacity | Rationale |
|---------|----------|-----------|
| P2P → Consensus | 256 | Buffer for incoming network messages. Larger than needed to absorb bursts. |
| Timer → Consensus | 16 | At most a few timeouts pending at once. |
| Mempool → Consensus | 16 | Only reap responses and tx notifications. |
| Consensus → P2P (outbound) | 64 | Proposals + votes. |
| Consensus → State Machine | 4 | At most one block being executed at a time. |
| Consensus → Storage | 16 | WAL entries + block persistence. |
| RPC → Mempool (tx submission) | 256 | Buffer for incoming transactions. |

### 3.3 Backpressure Behavior

```
When a bounded channel is full:

  Async sender (Tokio task):
    → try_send() returns Err(Full)
    → Message is dropped
    → Metric incremented: channel_drops_total{channel="p2p_to_consensus"}
    → Warning logged

  Sync sender (Consensus core):
    → send() blocks until space is available
    → This is acceptable because the consensus core should not
      produce faster than downstream can consume
    → If blocking exceeds a threshold (e.g., 5s), log an error
      (indicates a downstream thread is stuck)
```

---

## 4. Event Ordering Guarantees

### 4.1 Single-Source Ordering

Messages from a single source (e.g., one peer connection) arrive at the consensus core in the order they were sent. This is guaranteed by the mpsc channel (FIFO).

### 4.2 Cross-Source Ordering

Messages from different sources (e.g., peer A's vote vs. timer timeout) arrive in **non-deterministic** order. This is inherent in any concurrent system.

**How determinism is preserved despite non-deterministic arrival order:**

The consensus protocol is designed to be **order-independent** for correctness:
- Receiving a prevote before a proposal is safe (the vote is buffered).
- Receiving a timeout before votes is safe (the node votes nil).
- Receiving votes out of order is safe (they are accumulated in the VoteSet).

The consensus core produces the **same commit decision** regardless of the order in which events arrive, as long as the same set of events is eventually delivered. This is a fundamental property of the BFT protocol.

**What IS order-dependent:** The exact round at which a commit happens may vary (e.g., if a timeout arrives before a proposal, the node may vote nil and delay the commit to a later round). This affects **liveness** (speed) but not **safety** (correctness).

---

## 5. Consensus Core — Detailed Event Loop

```
fn run_consensus(
    inbound: crossbeam::Receiver<ConsensusEvent>,
    outbound: CommandSink,
    initial_state: ConsensusState,
) {
    let mut state = initial_state;
    let mut pending_commands: Vec<ConsensusCommand> = Vec::new();

    loop {
        // 1. Blocking receive — waits for next event
        let event = match inbound.recv() {
            Ok(event) => event,
            Err(_) => break,  // Channel closed → shutdown
        };

        // 2. Validate event relevance
        if !is_relevant(&state, &event) {
            continue;  // Stale event (wrong height/round)
        }

        // 3. Process event — pure synchronous logic
        let commands = process_event(&mut state, event);
        pending_commands.extend(commands);

        // 4. Drain any additional events available without blocking
        //    (batch processing for efficiency)
        while let Ok(event) = inbound.try_recv() {
            if is_relevant(&state, &event) {
                let commands = process_event(&mut state, event);
                pending_commands.extend(commands);
            }
        }

        // 5. Flush all pending commands
        for cmd in pending_commands.drain(..) {
            outbound.send(cmd);
        }
    }
}
```

### Why `try_recv()` After `recv()`?

After processing one event, additional events may have arrived. Processing them in a batch before flushing commands reduces the number of outbound messages (e.g., if multiple votes arrive simultaneously, the node can detect a quorum in one batch rather than sending intermediate state).

This is a **performance optimization** that does not affect correctness. The consensus core would produce the same final state whether events are processed one-at-a-time or in batches.

---

## 6. Timer Service

The timer service runs in the Tokio runtime and manages consensus timeouts:

```
┌─────────────────────────────────────────────────────┐
│                Timer Service (Tokio)                  │
│                                                      │
│  Receives from Consensus Core:                       │
│    ScheduleTimeout { kind, height, round, duration } │
│    CancelTimeout { kind, height, round }             │
│                                                      │
│  Implementation:                                     │
│    - Maintains a set of active timers                │
│    - Each timer is a tokio::time::sleep future       │
│    - On expiry, sends TimeoutX event to consensus    │
│    - On cancel, drops the future (no event sent)     │
│                                                      │
│  Sends to Consensus Core:                            │
│    TimeoutPropose { height, round }                  │
│    TimeoutPrevote { height, round }                  │
│    TimeoutPrecommit { height, round }                │
│                                                      │
└─────────────────────────────────────────────────────┘
```

### Timer Determinism

Timers are **not deterministic** — they depend on wall-clock time. This is acceptable because:
- Timers only affect **liveness** (when a timeout fires), not **safety** (what is committed).
- The consensus protocol is safe regardless of when (or if) timeouts fire.
- In testing, timers can be replaced with a mock that fires deterministically.

---

## 7. Shutdown Sequence

```
Shutdown signal (SIGTERM / SIGINT):

1. Signal all Tokio tasks to stop:
   - P2P: stop accepting connections, close existing connections
   - RPC: stop accepting requests, drain in-flight requests
   - Timers: cancel all pending timers

2. Close the inbound channel to the consensus core:
   - Consensus core's recv() returns Err → exits loop

3. Consensus core cleanup:
   - Flush any pending WAL entries
   - Do NOT attempt to complete the current round
     (other nodes will timeout and proceed)

4. State machine cleanup:
   - If a block is being executed, wait for completion (bounded timeout)
   - Flush state to storage

5. Storage cleanup:
   - Flush all pending writes
   - Close database handles

6. Exit process
```

### Graceful vs. Forced Shutdown

| Phase | Timeout | Action if exceeded |
|-------|---------|-------------------|
| Drain RPC requests | 5s | Drop remaining requests |
| Close P2P connections | 5s | Force-close sockets |
| Consensus core exit | 1s | Thread is joined with timeout |
| State machine completion | 10s | Abort execution, state may be incomplete (WAL handles recovery) |
| Storage flush | 10s | Force-close (risk of data loss, WAL handles recovery) |
| Total shutdown | 30s | SIGKILL |

---

## 8. Rust Enforcement of the Model

### 8.1 No Tokio in Consensus Module

The `consensus` module does NOT use `tokio`. It contains no `use tokio::` imports. This is enforced by convention and code review — if anyone adds `.await` to the consensus module, it should be caught in review. Additionally, the consensus module's public API accepts and returns only synchronous types (no `Future`, no `async fn`).

### 8.2 Ownership Prevents Shared State

```
// The consensus state is MOVED into the thread closure.
// No Arc, no Mutex, no Clone.

let consensus_state = ConsensusState::new(genesis);

std::thread::spawn(move || {
    run_consensus(inbound_rx, outbound_tx, consensus_state);
    // consensus_state is owned by this thread exclusively
});

// consensus_state is no longer accessible here — Rust's move semantics enforce this
```

### 8.3 Channel Types Enforce Boundaries

```
// crossbeam channels are Send but not Sync.
// The Receiver is moved into the consensus thread.
// The Sender is moved into the Tokio tasks.
// Neither end can be shared without explicit cloning.

let (tx, rx) = crossbeam::channel::bounded::<ConsensusEvent>(256);

// tx is cloned for each Tokio task that needs to send events
let p2p_tx = tx.clone();
let timer_tx = tx.clone();
let mempool_tx = tx.clone();
drop(tx); // Original sender dropped

// rx is moved into the consensus thread (only one receiver)
std::thread::spawn(move || {
    run_consensus(rx, ...);
});
```

### 8.4 No HashMap in Consensus

The crate-level `#![deny(clippy::disallowed_types)]` bans `HashMap` and `HashSet`. All map/set types must be `BTreeMap`/`BTreeSet` for deterministic iteration. This applies to the entire crate, but is especially critical in the `consensus` module.

---

## 9. Testing the Model

### 9.1 Unit Testing Consensus Core

The consensus core can be tested without any async runtime:

```
Test setup:
    1. Create crossbeam channels
    2. Construct ConsensusState with test validator set
    3. Send events manually via the sender
    4. Assert commands emitted on the receiver
    5. No Tokio, no network, no disk
```

### 9.2 Deterministic Replay

```
Replay test:
    1. Record a sequence of ConsensusEvents (from a real run or constructed)
    2. Feed them to the consensus core in order
    3. Assert the same sequence of ConsensusCommands is emitted
    4. This proves determinism: same inputs → same outputs
```

### 9.3 Integration Testing

```
Integration test:
    1. Spawn 4 consensus core threads
    2. Connect them via in-memory channels (no real network)
    3. Simulate proposals, votes, timeouts
    4. Assert all 4 nodes commit the same blocks
```

---

## 10. Performance Considerations

| Concern | Mitigation |
|---------|------------|
| Consensus core is single-threaded | Consensus logic is CPU-light (hashing, signature verification). The bottleneck is network latency, not CPU. |
| Blocking recv() wastes CPU | `crossbeam::channel::recv()` parks the thread (no busy-wait). Zero CPU when idle. |
| Channel backpressure slows consensus | Channels are sized to absorb bursts. Persistent backpressure indicates a downstream bottleneck (logged as error). |
| State machine execution blocks consensus | Execution runs on a separate thread. Consensus core sends `ExecuteBlock` and continues processing events. The `BlockExecuted` result arrives as an event. |
| WAL fsync latency | WAL writes are on the storage thread. Consensus core does not wait for fsync (fire-and-forget with ordering guarantees from the channel). |

---

## Definition of Done — Event Loop & Threading

- [x] Hybrid model rationale explained (why not fully sync, why not fully async)
- [x] Complete thread map with all threads and their responsibilities
- [x] Channel topology with types, capacities, and backpressure behavior
- [x] Event ordering guarantees and determinism argument
- [x] Consensus core event loop with batch processing
- [x] Timer service design
- [x] Shutdown sequence with timeouts
- [x] Rust enforcement mechanisms (no tokio dep, ownership, clippy deny)
- [x] Testing strategies for the model
- [x] Performance considerations
- [x] ASCII diagrams included
- [x] No Rust source code included
