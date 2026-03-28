# RustBFT — Design Index

RustBFT is a Byzantine Fault Tolerant blockchain node written in Rust. It implements the Tendermint BFT consensus protocol on a dedicated OS thread, bridged via the `CommandRouter` to async Tokio subsystems: TCP-based P2P networking with encrypted gossip, a WASM smart contract runtime, an account-based state machine, RocksDB-backed storage, a JSON-RPC 2.0 API, and Prometheus metrics. The binary is a single node process; multi-node operation requires seed configuration and Docker-based cluster tooling.

## Mental Map

```
┌─ Consensus BFT ─────────────────────────────────┐  ┌─ P2P Networking ───────────────────────────────┐
│ Owns: round/step state machine, vote quorums    │  │ Owns: TCP connections, handshake, gossip       │
│ Entry: src/consensus/state.rs (ConsensusCore)   │  │ Entry: src/p2p/manager.rs (P2pManager::run)    │
│ Key:   vote_set.rs, proposer.rs, timer.rs       │  │ Key:   peer.rs, codec.rs, gossip.rs            │
│ Uses:  Command Router (via crossbeam channels)  │  │ Uses:  Command Router (mpsc), Core Types       │
└─────────────────────────────────────────────────┘  └────────────────────────────────────────────────┘

┌─ Command Router ────────────────────────────────┐  ┌─ State Machine ────────────────────────────────┐
│ Owns: crossbeam↔Tokio bridge, block exec flow   │  │ Owns: AppState, tx execution, gas metering     │
│ Entry: src/router/mod.rs (CommandRouter::run)   │  │ Entry: src/state/executor.rs (execute_block)   │
│ Uses:  State Machine, Storage, Smart Contracts, │  │ Key:   accounts.rs, tx.rs, merkle.rs           │
│        P2P Networking, Metrics                  │  │ Uses:  Smart Contracts                         │
└─────────────────────────────────────────────────┘  └────────────────────────────────────────────────┘

┌─ Smart Contracts ───────────────────────────────┐  ┌─ Storage ──────────────────────────────────────┐
│ Owns: WASM validation, compilation, host API    │  │ Owns: RocksDB blocks/state, WAL                │
│ Entry: src/contracts/runtime.rs                 │  │ Entry: src/storage/block_store.rs, wal.rs      │
│ Key:   host_api.rs, validation.rs               │  │ Key:   state_store.rs                          │
│ Uses:  State Machine (AppState moved in/out)    │  └────────────────────────────────────────────────┘
└─────────────────────────────────────────────────┘

┌─ RPC Server ────────────────────────────────────┐  ┌─ Metrics and Observability ────────────────────┐
│ Owns: JSON-RPC 2.0 HTTP API, health probes      │  │ Owns: Prometheus registry, metrics server      │
│ Entry: src/rpc/server.rs (RpcServer::run)       │  │ Entry: src/metrics/server.rs, registry.rs      │
│ Key:   handlers.rs, types.rs                    │  │ Uses: (Arc<Metrics> shared with Command Router)│
│ Uses:  Storage (reads), State Machine (reads)   │  └────────────────────────────────────────────────┘
└─────────────────────────────────────────────────┘

┌─ Shared ────────────────────────────────────────┐  ┌─ DevOps and Deployment ────────────────────────┐
│ Owns: Block/Vote/ValidatorSet types, codec      │  │ Owns: Docker build, 4-node local cluster       │
│ Key:   src/types/, src/crypto/, src/config/     │  │ Entry: devops/Dockerfile, docker-compose.yml   │
└─────────────────────────────────────────────────┘  │ Key:   scripts/init-cluster.sh,                │
                                                     │        scripts/simulate-failure.sh             │
                                                     └────────────────────────────────────────────────┘
```

## Feature Matrix

| Feature | Description | File | Status |
|---------|-------------|------|--------|
| Consensus BFT Engine | Tendermint round-based BFT state machine with vote collection, proposer selection, and timeout management | [consensus-bft.md](consensus-bft.md) | Stable |
| P2P Networking | TCP transport with X25519+ChaCha20 encryption, Ed25519 handshake, and gossip fan-out | [p2p-networking.md](p2p-networking.md) | In Progress |
| State Machine | Account-based world state with snapshot/revert, gas metering, and four transaction types | [state-machine.md](state-machine.md) | Stable |
| Smart Contracts | WebAssembly contract runtime via wasmtime with sandboxed host API and fuel-based gas | [smart-contracts.md](smart-contracts.md) | In Progress |
| Storage | RocksDB block store, JSON state snapshots, and hex-encoded WAL for crash recovery | [storage.md](storage.md) | In Progress |
| RPC Server | JSON-RPC 2.0 HTTP API for block queries, account state, validator set, and tx submission | [rpc-server.md](rpc-server.md) | Stable |
| Metrics and Observability | Prometheus metrics server and structured tracing-based logging | [metrics-observability.md](metrics-observability.md) | In Progress |
| Command Router | Async bridge between synchronous consensus core and all Tokio subsystems | [command-router.md](command-router.md) | Stable |
| Node Configuration | TOML-based configuration loading with defaults for all subsections | [node-config.md](node-config.md) | Stable |
| Core Types and Serialization | Shared data structures (Block, Vote, ValidatorSet, etc.) and binary P2P/storage codec | [core-types.md](core-types.md) | Stable |
| DevOps and Deployment | Docker multi-stage build, 4-node local testnet via docker-compose, and operational scripts | [devops-deployment.md](devops-deployment.md) | Stable |

## Cross-Cutting Concerns

**Threading model:** `ConsensusCore` runs on a dedicated OS thread communicating via bounded `crossbeam_channel`. All other subsystems (P2P, timers, RPC, storage, metrics) run on the multi-threaded Tokio runtime. `CommandRouter` runs as a Tokio task that blocks the thread with `crossbeam::recv()` — this is a known issue.

**Channel topology:**
- `ConsensusCore → CommandRouter`: `crossbeam_channel<ConsensusCommand>` (bounded 1024)
- `CommandRouter → ConsensusCore`: `crossbeam_channel<ConsensusEvent>` (bounded 256)
- `CommandRouter → P2pManager`: `tokio::mpsc<ConsensusCommand>` (1024)
- `CommandRouter → TimerService`: `tokio::mpsc<TimerCommand>` (256)
- `P2pManager → ConsensusCore`: `crossbeam_channel<ConsensusEvent>` (try_send, bounded 256)
- `RpcServer → (dropped)`: `tokio::mpsc<Vec<u8>>` for tx submission — receiver is leaked in MVP

**Shared state (Arc-wrapped):**
- `Arc<BlockStore>` — read by RPC, written by CommandRouter
- `Arc<RwLock<AppState>>` — read by RPC (read lock), written by CommandRouter (write lock during execution)
- `Arc<RwLock<ValidatorSet>>` — read by RPC, written by CommandRouter after each block
- `Arc<tokio::sync::Mutex<ContractRuntime>>` — exclusive during block execution
- `Arc<tokio::sync::Mutex<WAL>>` — exclusive during WAL writes and truncation
- `Arc<Metrics>` — shared by CommandRouter and MetricsServer

**Error handling convention:** All subsystem errors during block execution are logged via `tracing::error!` but do not halt the node, except `execute_block` failure which causes `ConsensusCore` to hang in `Step::Commit` indefinitely.

**Determinism requirements:** All state transitions must be deterministic across nodes. WASM contracts enforce this by banning floats, SIMD, threads, and requiring NaN canonicalization. Transaction decoding uses big-endian fixed-width binary. `BTreeMap` used throughout for sorted iteration order.

**Crypto:** Ed25519 (ed25519-dalek v2.1) for validator identity and signing. SHA-256 (sha2) for hashing. X25519 (x25519-dalek v2) for P2P session key exchange. ChaCha20-Poly1305 (chacha20poly1305 v0.10) for P2P encryption. In MVP, all signature verifications are bypassed (`|_| true`).

**MVP gaps:** No mempool (blocks always empty), no WAL replay on startup, no tx signature verification, no peer reconnection, no block sync, no commit signatures stored, no Merkle proofs.

## Notes

None.
