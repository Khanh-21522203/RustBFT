# RustBFT — Architecture Overview

**Purpose:** Define the system-level architecture, module layout, data flow, and ownership boundaries.
**Audience:** Core engineers implementing or reviewing the system.

---

## 1. Architectural Philosophy

RustBFT follows three non-negotiable principles:

1. **Deterministic finality** — committed blocks are irreversible.
2. **Strict layered separation** — each layer has a single responsibility; no cross-layer leakage.
3. **Hybrid threading** — async I/O shell wrapping a synchronous, single-threaded consensus core.

Every design decision is evaluated against these principles. If a choice improves performance but weakens determinism or layer isolation, it is rejected.

---

## 2. System Layers

```
┌─────────────────────────────────────────────────────────────┐
│                        LAYER STACK                          │
├─────────────────────────────────────────────────────────────┤
│  Layer 7:  Observability    (metrics, logs, health)         │
│  Layer 6:  RPC / API        (HTTP JSON-RPC, WebSocket)      │
│  Layer 5:  Networking       (P2P, gossip, peer mgmt)        │
│  Layer 4:  Mempool          (tx intake, validation, reap)   │
│  Layer 3:  Consensus        (BFT protocol, single-thread)   │
│  Layer 2:  State Machine    (block exec, contract engine)   │
│  Layer 1:  Storage          (blocks, state, WAL)            │
│  Layer 0:  Crypto / Types   (hashing, signing, primitives)  │
└─────────────────────────────────────────────────────────────┘
```

### Layer Interaction Rules

- Layers communicate **only** with adjacent layers via explicit interfaces (traits + channels).
- No layer may import the internal types of a non-adjacent layer.
- The consensus core (Layer 3) is the **sole driver** of state transitions. No other layer may mutate consensus state, apply blocks, or modify the validator set.

---

## 3. Single-Crate Module Layout

RustBFT is organized as a **single Rust crate** with a flat module hierarchy. This avoids the overhead of managing multiple crates (separate `Cargo.toml` files, cross-crate versioning, longer compile times from dependency graph resolution) while still enforcing clear boundaries via Rust's module visibility system.

```
rustbft/
├── Cargo.toml                    # Single crate
├── src/
│   ├── main.rs                   # Binary entry point: wires all modules
│   ├── lib.rs                    # Library root: re-exports public modules
│   ├── types/                    # Layer 0: shared types, primitives
│   │   ├── mod.rs
│   │   ├── block.rs
│   │   ├── transaction.rs
│   │   ├── vote.rs
│   │   ├── proposal.rs
│   │   ├── validator.rs
│   │   └── serialization.rs      # Canonical deterministic encoding
│   ├── crypto/                   # Layer 0: signing, verification, hashing
│   │   ├── mod.rs
│   │   ├── ed25519.rs
│   │   └── hash.rs
│   ├── storage/                  # Layer 1: block store, state store, WAL
│   │   ├── mod.rs
│   │   ├── block_store.rs
│   │   ├── state_store.rs
│   │   └── wal.rs
│   ├── state/                    # Layer 2: state machine, block execution
│   │   ├── mod.rs
│   │   ├── executor.rs
│   │   ├── accounts.rs
│   │   └── merkle.rs
│   ├── contracts/                # Layer 2: contract execution engine
│   │   ├── mod.rs
│   │   ├── runtime.rs
│   │   ├── host_api.rs
│   │   └── validation.rs
│   ├── consensus/                # Layer 3: BFT consensus core
│   │   ├── mod.rs
│   │   ├── state.rs
│   │   ├── vote_set.rs
│   │   ├── proposer.rs
│   │   └── events.rs
│   ├── mempool/                  # Layer 4: transaction pool
│   │   ├── mod.rs
│   │   └── pool.rs
│   ├── p2p/                      # Layer 5: networking, peer management
│   │   ├── mod.rs
│   │   ├── transport.rs
│   │   ├── peer.rs
│   │   └── gossip.rs
│   ├── rpc/                      # Layer 6: HTTP/WS API
│   │   ├── mod.rs
│   │   ├── server.rs
│   │   └── handlers.rs
│   └── metrics/                  # Layer 7: Prometheus metrics, logging
│       ├── mod.rs
│       └── prometheus.rs
├── tests/
│   ├── integration/              # Multi-node integration tests
│   └── determinism/              # Replay and determinism tests
├── devops/
│   ├── docker-compose.yml
│   ├── Dockerfile
│   ├── prometheus.yml
│   └── grafana/
└── docs/                         # This documentation
```

### Module Dependency Rules

Layer isolation is enforced via Rust's `pub` / `pub(crate)` visibility:

```
main.rs (binary entry point)
  │
  ├── uses rpc::       (starts HTTP server)
  ├── uses p2p::       (starts P2P listener)
  ├── uses mempool::   (initializes tx pool)
  ├── uses consensus:: (spawns consensus core thread)
  ├── uses state::     (initializes state machine)
  ├── uses contracts:: (initializes WASM runtime)
  ├── uses storage::   (opens database)
  ├── uses metrics::   (starts Prometheus exporter)
  ├── uses crypto::    (loads keys)
  └── uses types::     (shared by all modules)

Module visibility rules:
  types::     → no internal deps (used by all other modules)
  crypto::    → uses types::
  storage::   → uses types::
  state::     → uses types::, crypto::, contracts::
  contracts:: → uses types::
  consensus:: → uses types::, crypto::  (MUST NOT use state::, storage::, contracts::)
  mempool::   → uses types::, crypto::
  p2p::       → uses types::, crypto::
  rpc::       → uses types::
  metrics::   → uses types::
```

**Key rule:** The `consensus` module MUST NOT import `state`, `storage`, or `contracts`. It emits commands that `main.rs` routes to those modules. This enforces the boundary between consensus decisions and execution.

### Why Single Crate?

| Concern | Multi-Crate Workspace | Single Crate with Modules |
|---------|----------------------|---------------------------|
| Build time | Parallel crate compilation | Faster incremental builds, no cross-crate overhead |
| Boundary enforcement | Compile-time via crate deps | `pub(crate)` + module visibility + code review |
| Boilerplate | Separate `Cargo.toml` per crate | Single `Cargo.toml` |
| Refactoring | Cross-crate renames are painful | Simple module moves |
| Testing | Per-crate `cargo test` | Single `cargo test`, easy integration tests |
| Dependency management | Each crate declares deps | One place for all deps |

The trade-off is that module boundaries are enforced by **convention and visibility modifiers** rather than by the crate system. This is acceptable for an MVP and can be split into a workspace later if the codebase grows significantly.

---

## 4. Data Flow — Happy Path (Single Block)

```
Step 1: Transaction Arrival
  Client ──HTTP──▶ RPC ──channel──▶ Mempool
  Peer   ──TCP───▶ P2P ──channel──▶ Mempool

Step 2: Proposal
  Timer fires ──▶ Consensus receives NewRound event
  Consensus determines: "I am proposer"
  Consensus ──RequestTxs──▶ Mempool (via callback/channel)
  Mempool returns ordered tx list
  Consensus constructs Block, signs Proposal
  Consensus ──BroadcastProposal──▶ P2P ──TCP──▶ Peers

Step 3: Prevote
  Peer receives Proposal ──▶ P2P validates signature
  P2P ──ProposalReceived──▶ Consensus
  Consensus validates block, emits Prevote
  Consensus ──BroadcastVote──▶ P2P ──TCP──▶ Peers

Step 4: Precommit
  Consensus collects >2/3 prevotes for block B
  Consensus emits Precommit for B
  Consensus ──BroadcastVote──▶ P2P ──TCP──▶ Peers

Step 5: Commit
  Consensus collects >2/3 precommits for block B
  Consensus emits CommitBlock command
  Node routes CommitBlock to:
    State Machine: BeginBlock → DeliverTx(×N) → EndBlock → Commit
    Storage: persist block + state
    Mempool: evict committed txs
  Consensus advances to height+1

Step 6: Observability
  Each step emits metrics + structured logs
```

---

## 5. Ownership Boundaries (Rust Enforcement)

The Rust type system enforces layer isolation:

### 5.1 Consensus Core Owns Its State

The consensus state struct (`ConsensusState`) is **owned** by the consensus core thread. No `Arc<Mutex<_>>` wrapping. No shared references. Other components interact via message passing only.

```
ConsensusState {
    height: u64,
    round: u32,
    step: Step,                    // Propose | Prevote | Precommit | Commit
    locked_round: Option<u32>,
    locked_block: Option<BlockHash>,
    valid_round: Option<u32>,
    valid_block: Option<BlockHash>,
    votes: VoteSet,                // BTreeMap-based, deterministic
    validator_set: ValidatorSet,
}
```

This struct is **never** behind a lock. It lives on a single thread. Rust's ownership system guarantees no other thread can access it.

### 5.2 Channel-Based Boundaries

```
                    ┌──────────────┐
  P2P ──(mpsc)────▶│              │──(mpsc)──▶ P2P (outbound)
  Mempool ─(mpsc)─▶│  Consensus   │──(mpsc)──▶ State Machine
  Timers ──(mpsc)─▶│    Core      │──(mpsc)──▶ Mempool
                    │              │──(mpsc)──▶ Metrics
                    └──────────────┘
```

All channels are **bounded** `mpsc` channels (Tokio or crossbeam). Bounded channels provide:
- **Backpressure:** If the consensus core is slow, senders block (preventing OOM).
- **Explicit failure:** A full channel is a detectable error, not silent data loss.

### 5.3 No Shared Mutable State

| Pattern | Allowed? | Rationale |
|---------|----------|-----------|
| `Arc<Mutex<T>>` across layers | **No** | Violates single-ownership, introduces lock contention and non-determinism. |
| `mpsc::channel` | **Yes** | Explicit, ordered, backpressure-aware. |
| `oneshot::channel` for request-response | **Yes** | Used for Mempool.reap() and Storage.query(). |
| `Arc<T>` for immutable config | **Yes** | Read-only, no mutation. |
| Thread-local state | **Yes** | Consensus core state is thread-local by construction. |

---

## 6. Message Types Between Layers

### 6.1 Inbound to Consensus (Events)

```
enum ConsensusEvent {
    // From P2P
    ProposalReceived { proposal: SignedProposal },
    VoteReceived { vote: SignedVote },

    // From Timers
    TimeoutPropose { height: u64, round: u32 },
    TimeoutPrevote { height: u64, round: u32 },
    TimeoutPrecommit { height: u64, round: u32 },

    // From State Machine (after block execution)
    BlockExecuted { height: u64, state_root: Hash, validator_updates: Vec<ValidatorUpdate> },

    // From Mempool (response to reap request)
    TxsAvailable { txs: Vec<SignedTransaction> },
}
```

### 6.2 Outbound from Consensus (Commands)

```
enum ConsensusCommand {
    // To P2P
    BroadcastProposal { proposal: SignedProposal },
    BroadcastVote { vote: SignedVote },

    // To State Machine
    ExecuteBlock { block: Block },

    // To Mempool
    ReapTxs { max_bytes: usize },
    EvictTxs { tx_hashes: Vec<Hash> },

    // To Storage
    PersistBlock { block: Block, state: AppState },
    WriteWAL { entry: WALEntry },

    // To Timers
    ScheduleTimeout { timeout: Timeout },
    CancelTimeout { timeout: Timeout },
}
```

---

## 7. Startup Sequence

```
1. Load config (TOML + env overrides)
2. Initialize storage backend
3. Load last committed state (or genesis if fresh)
4. Initialize validator set from state
5. Initialize consensus core with (height, round=0, step=Propose)
6. Start P2P listener, connect to seed peers
7. Start mempool
8. Start RPC server
9. Start metrics exporter
10. Start timer service
11. Enter main event loop:
    - Consensus core recv() from all inbound channels
    - Process event
    - Emit commands to outbound channels
12. On SIGTERM/SIGINT:
    - Stop accepting new connections
    - Flush WAL
    - Complete current round if possible
    - Persist state
    - Exit
```

---

## 8. Key Design Decisions and Trade-offs

| Decision | Trade-off | Rationale |
|----------|-----------|-----------|
| Single-threaded consensus | Lower throughput ceiling | Eliminates concurrency bugs in the most critical path. Determinism > performance. |
| Bounded channels everywhere | Potential stalls under load | Prevents OOM. Stalls are detectable and debuggable. |
| No `HashMap` in consensus | Slightly slower lookups | `BTreeMap` has deterministic iteration order. |
| Single crate with module boundaries | Boundaries enforced by visibility, not crate system | Simpler build, faster iteration, easy refactoring. Acceptable for MVP. |
| Consensus module does not import state/storage/contracts | Indirection via commands | Consensus can be tested in complete isolation. |
| WAL for crash recovery | Disk I/O on every round | Safety requires durability. WAL writes are sequential and fast. |
| Canonical serialization (not serde defaults) | Must define custom serialization | Prevents platform-dependent encoding from breaking determinism. |

---

## 9. Security Boundaries

```
┌─────────────────────────────────────────────────┐
│              UNTRUSTED BOUNDARY                  │
│                                                  │
│  Internet ──▶ RPC (rate-limited, validated)      │
│  Peers    ──▶ P2P (authenticated, sig-checked)   │
│                                                  │
├─────────────────────────────────────────────────┤
│              TRUSTED BOUNDARY                    │
│                                                  │
│  Consensus Core (only processes validated msgs)  │
│  State Machine  (only executes committed blocks) │
│  Storage        (only persists committed data)   │
│                                                  │
└─────────────────────────────────────────────────┘
```

- All data crossing the untrusted boundary MUST be validated (signature, format, size limits).
- The consensus core MUST NOT process raw network bytes.
- Contract execution is sandboxed: no host access beyond the defined host API.

---

## Definition of Done — Architecture Overview

- [x] All layers enumerated with responsibilities
- [x] Module layout with dependency rules
- [x] Data flow for happy path
- [x] Ownership boundaries documented with Rust-specific enforcement
- [x] Channel types and backpressure strategy defined
- [x] Message types between layers specified
- [x] Startup and shutdown sequences
- [x] Design decisions with explicit trade-offs
- [x] Security boundaries identified
- [x] ASCII diagrams included
- [x] No Rust source code included
