# RustBFT Codebase Review Guide

This guide explains the **current implementation** feature by feature so you can quickly build working context before fixing issues.

It is written for this repository state (LLM-generated code), not the target architecture in plans.

---

## 1) How to use this guide

Use this in 3 passes:

1. Read `Section 2` and `Section 3` to understand startup/runtime wiring.
2. Read feature sections (`Section 4` through `Section 15`) in order.
3. Use `Section 16` and `Section 17` when you start making fixes (what to touch first, where risk is highest).

If you only have 30 minutes, read:
- `src/main.rs`
- `src/consensus/state.rs`
- `src/router/mod.rs`
- `src/storage/wal.rs`
- `src/p2p/manager.rs`
- `src/state/executor.rs`
- `src/rpc/handlers.rs`

---

## 2) High-level architecture (as implemented)

Current architecture is a hybrid of sync + async:

- Sync thread:
  - Consensus core (`src/consensus/state.rs`) via blocking crossbeam `recv()`.
- Async Tokio tasks:
  - Router (`src/router/mod.rs`) bridging consensus commands to async subsystems.
  - P2P manager (`src/p2p/manager.rs`).
  - Timer service (`src/consensus/timer.rs`).
  - RPC server (`src/rpc/server.rs`).
  - Metrics server (`src/metrics/server.rs`).
- Persistent storage:
  - RocksDB block store (`src/storage/block_store.rs`)
  - RocksDB state store (`src/storage/state_store.rs`)
  - WAL file (`src/storage/wal.rs`)

Key runtime channels:

- `crossbeam::bounded<ConsensusEvent>(256)` from async layers -> consensus
- `crossbeam::bounded<ConsensusCommand>(1024)` from consensus -> router
- Tokio mpsc for router -> timer/p2p and RPC tx submit

Important practical note:
- Router runs as `async fn` but uses blocking `crossbeam recv()` internally.

---

## 3) Startup and runtime wiring (real flow in `main.rs`)

File: `src/main.rs`

Startup steps:

1. Load config path from `argv[1]`, default `config/node.toml`.
2. `NodeConfig::load_or_default()`:
   - Parses TOML if possible, otherwise silently falls back to defaults.
3. Initialize logging (`init_logging`).
4. Initialize metrics registry and optional metrics server.
5. Create consensus channels:
   - `tx_consensus_ev/rx_consensus_ev`
   - `tx_consensus_cmd/rx_consensus_cmd`
6. Load or generate Ed25519 key at `{data_dir}/node_key.json`.
7. Bootstrap validator set as a **single validator (self only)**.
8. Open storage paths:
   - `{data_dir}/blocks`
   - `{data_dir}/state`
   - `{data_dir}/consensus.wal`
9. Load latest app state from state store (or default chain params).
10. Start timer service task.
11. Build P2P manager with verify closures currently set to `|_| true`.
12. Build router and spawn it as Tokio task.
13. Spawn consensus core thread.
14. Start RPC server task.
15. Wait on `tokio::select!` between P2P run and Ctrl-C signal.

Shutdown path today:
- Only Ctrl-C (`SIGINT`) is handled.
- Logs shutdown messages, but no full staged drain/join sequence.

Why this matters:
- Most production behavior you will debug starts in `main.rs` wiring assumptions.

---

## 4) Feature: Foundation Types and Serialization

### Files
- `src/types/mod.rs`
- `src/types/hash.rs`
- `src/types/address.rs`
- `src/types/block.rs`
- `src/types/proposal.rs`
- `src/types/vote.rs`
- `src/types/validator.rs`
- `src/types/transaction.rs`
- `src/types/serialization.rs`

### What exists now

Core wrappers:
- `Hash([u8;32])`, `Address([u8;20])`, `ValidatorId([u8;32])`.

Consensus objects:
- `Block`, `BlockHeader`, `CommitInfo`
- `Proposal`/`SignedProposal`
- `Vote`/`SignedVote`
- `Evidence`

Validator set:
- `ValidatorSet` internally uses `BTreeMap<ValidatorId, Validator>`.
- Has deterministic iteration and update policy checks.

Serialization:
- Custom encoder/decoder in `types/serialization.rs`.
- Used for block/proposal/vote wire encoding.

### Important implementation details

- Block encoding currently omits `last_commit` in `encode_block/decode_block` (decode sets `last_commit: None`).
- Several components still hash blocks via `serde_json::to_vec()` instead of canonical encoder.
- Transaction type in `types/transaction.rs` is not the one used by executor path.

### What to read first

1. `types/serialization.rs` (format primitives and object codecs)
2. `types/block.rs`, `types/vote.rs`, `types/proposal.rs`
3. `types/validator.rs`
4. `types/transaction.rs`

### Why this feature is central

Every other subsystem depends on these structures. Any hash/signature bug here propagates to consensus, P2P, storage, and RPC.

---

## 5) Feature: Crypto and Node Identity

### Files
- `src/crypto/hash.rs`
- `src/crypto/ed25519.rs`

### What exists now

Hashing:
- `sha256(data) -> Hash`

Ed25519:
- Key generation
- Sign/verify helpers
- `load_or_generate_keypair(path)` using 32-byte raw private key file

Identity model:
- Node ID currently uses verify key bytes directly in `main.rs` (`ValidatorId(verify_key.to_bytes())`).

### Practical review notes

- Crypto helpers are straightforward and usable.
- Integration is currently incomplete in consensus/P2P signature flow (many placeholders).

---

## 6) Feature: Consensus Core FSM

### Files
- `src/consensus/state.rs`
- `src/consensus/events.rs`
- `src/consensus/vote_set.rs`
- `src/consensus/proposer.rs`
- `src/consensus/mod.rs`

### What exists now

Consensus state includes:
- `height`, `round`, `step`
- lock state (`locked_round`, `locked_block`)
- valid state (`valid_round`, `valid_block`)
- proposal cache
- vote set
- validator set

Event inputs (`ConsensusEvent`):
- proposal/vote received
- timeout events
- txs available
- block executed

Command outputs (`ConsensusCommand`):
- broadcast proposal/vote
- execute/persist block
- write WAL
- reap/evict txs
- schedule/cancel timeout

### Core flow today

- On startup -> `enter_new_height()` -> `enter_propose()`.
- If node is proposer, sends `ReapTxs`.
- `TxsAvailable` builds block with placeholders:
  - `prev_block_hash = Hash::ZERO`
  - `state_root = Hash::ZERO`
  - `tx_merkle_root = Hash::ZERO`
- Proposal signature is placeholder `[0u8;64]`.
- Votes are also placeholder signature `[0u8;64]`.
- Commit path emits `ExecuteBlock`, then on `BlockExecuted` emits `PersistBlock`.

### Vote handling

`VoteSet`:
- deterministic `BTreeMap` keyed by `(round, vote_type, validator)`
- duplicate and equivocation detection
- quorum math `(2/3 * total) + 1`

Equivocation behavior:
- evidence is stored in vote set
- consensus currently logs via `eprintln!` on equivocation path

### Key assumptions in current code

- Signature verification callbacks are externally injected and currently permissive.
- Block hash callback uses JSON serialization in `main.rs` default deps.
- WAL writes are requested via outbound command channel.

### Best reading order

1. `consensus/events.rs`
2. `consensus/vote_set.rs`
3. `consensus/state.rs` from `enter_new_height` through handlers
4. `consensus/proposer.rs`

---

## 7) Feature: Timer Service

### File
- `src/consensus/timer.rs`

### What exists now

- Tokio task receives `TimerCommand::{Schedule, Cancel}`.
- Maintains active timers keyed by `(height, round, kind)`.
- On fire, sends corresponding `ConsensusEvent::Timeout*` into crossbeam channel.

### Important behavior

- Uses `watch` channels for cancellation signaling.
- One schedule call can replace existing timer for same key.

### Why it matters

Timeouts drive liveness and round advancement.

---

## 8) Feature: Command Router (Consensus bridge)

### File
- `src/router/mod.rs`

### Responsibility

Takes `ConsensusCommand` and fans out to:
- P2P manager
- timer service
- state executor
- storage components
- WAL
- consensus event channel (for immediate responses like `TxsAvailable`)

### Current command handling summary

- Broadcast commands forwarded to P2P via Tokio mpsc.
- Timeout schedule/cancel forwarded to timer service.
- ExecuteBlock:
  - acquires write lock on app state
  - locks contract runtime
  - calls `StateExecutor::execute_block`
  - sends `ConsensusEvent::BlockExecuted`
- PersistBlock:
  - saves block/state to RocksDB
  - truncates WAL
  - updates `consensus_height` metric
- WriteWAL:
  - writes entry through WAL object
  - records WAL write duration metric
- ReapTxs/EvictTxs currently placeholder behavior.

### Critical runtime nuance

`run()` is async but blocks on `self.rx_cmd.recv()` (crossbeam blocking call).

---

## 9) Feature: State Machine and Transaction Execution

### Files
- `src/state/accounts.rs`
- `src/state/executor.rs`
- `src/state/tx.rs`
- `src/state/merkle.rs`

### App state model (`accounts.rs`)

`AppState` contains:
- `accounts: BTreeMap<Address, Account>`
- `contract_code: BTreeMap<Hash, Vec<u8>>`
- `contract_storage: BTreeMap<(Address, Vec<u8>), Vec<u8>>`
- `params: ChainParams`

Snapshot system:
- `snapshot()`, `revert_to()`, `commit_snapshot()`
- Uses stack of recorded state changes

### Transaction decode path (`state/tx.rs`)

- Decodes raw bytes into `DecodedTx` variants.
- This is a separate transaction model from `types/transaction.rs`.

### Block execution (`executor.rs`)

Pipeline:
1. `begin_block` (no-op currently)
2. Iterate tx bytes and decode
3. Execute each tx variant
4. Collect validator updates
5. `end_block` converts updates to `ValidatorUpdate`
6. Compute state root via `compute_state_root`

Tx variant behavior:
- Transfer: nonce/balance checks + gas charge
- Deploy: code deploy + optional constructor call
- Call: value transfer + contract call
- ValidatorUpdate: collected for `end_block`

Gas model:
- intrinsic + per-type rules
- gas is charged via balance subtraction

State root (`state/merkle.rs`):
- not trie-based; concatenates sorted state bytes and hashes once.

---

## 10) Feature: Smart Contract Runtime (WASM)

### Files
- `src/contracts/runtime.rs`
- `src/contracts/host_api.rs`
- `src/contracts/validation.rs`
- `src/contracts/mod.rs`

### Runtime architecture

- Uses Wasmtime engine with deterministic-oriented settings:
  - fuel enabled
  - SIMD/threads disabled
  - NaN canonicalization enabled

`ContractRuntime` stores modules in memory map by `code_hash`.

Call flow:
1. Validate and load module
2. Snapshot app state
3. Build `HostState` with environment and hooks
4. Instantiate module + write input to memory
5. Call exported `call` function
6. Read return buffer format
7. Commit or revert snapshot based on success

### Host API (`host_api.rs`)

Exposed functions include:
- storage read/write/delete/has
- context getters (caller, origin, block metadata)
- `host_sha256`, `host_verify_ed25519`
- `host_emit_event`
- `host_call_contract`
- `host_abort`

Gas accounting:
- each host function charges gas by reducing store fuel.

Cross-contract calls:
- implemented through a closure hook that recursively invokes runtime `call`.
- call depth check exists in host call path (`>=64` returns failure).

### Validation (`validation.rs`)

Rejects modules with:
- float instructions
- SIMD instructions
- threads atomics
- bulk memory ops
- disallowed imports
- start section
- oversized memory/code

---

## 11) Feature: Storage (Blocks, State, WAL)

### Files
- `src/storage/block_store.rs`
- `src/storage/state_store.rs`
- `src/storage/wal.rs`
- `src/storage/mod.rs`

### Block store (`block_store.rs`)

RocksDB column families:
- `blocks`
- `block_hash`
- `state_root`
- `valset`
- `meta`

Capabilities:
- save/load block
- load hash/root/validator set by height
- `last_height()`
- `prune_below()`

### State store (`state_store.rs`)

RocksDB CFs:
- `state_snapshot` (full JSON state per height)
- `state_meta` (`latest` height)

Capabilities:
- save/load state by height
- load latest
- prune below height

### WAL (`wal.rs`)

Current format:
- Encoded entry bytes -> hex string line per entry in file
- Entry structure includes height/round/kind/data and SHA-256 checksum

Capabilities:
- append entry
- read all entries (stop at corrupt tail)
- truncate whole file
- truncate below height

### Storage model summary

- No dedicated storage thread abstraction yet.
- Router writes to stores directly in async task.
- State persistence is snapshot-based, not merkle-trie versioned.

---

## 12) Feature: P2P Networking and Gossip

### Files
- `src/p2p/config.rs`
- `src/p2p/msg.rs`
- `src/p2p/codec.rs`
- `src/p2p/peer.rs`
- `src/p2p/gossip.rs`
- `src/p2p/manager.rs`

### What exists now

P2P manager responsibilities:
- listen for inbound TCP
- connect to configured seeds once at startup
- spawn per-peer reader/writer tasks
- deduplicate gossip using LRU seen set
- relay proposal/vote events to consensus via `try_send`
- fanout consensus broadcast commands to peers

Handshake (`peer.rs`):
- plaintext handshake with signed challenge
- protocol and chain_id checks
- derives symmetric key and upgrades to encrypted framing

Message types (`msg.rs`):
- handshake/ack, proposal, prevote, precommit, tx, evidence
- block request/response, ping/pong, disconnect

### Notable current behavior

- Seed parser currently extracts only `host:port` from `node_id@host:port`.
- Invalid proposal/vote is dropped (no peer scoring).
- No explicit reconnect/backoff manager loop.
- No active periodic ping task despite message support.

### Read order

1. `p2p/msg.rs` (protocol shape)
2. `p2p/peer.rs` (handshake/auth)
3. `p2p/manager.rs` (runtime behavior)
4. `p2p/gossip.rs` (dedup logic)

---

## 13) Feature: JSON-RPC API

### Files
- `src/rpc/server.rs`
- `src/rpc/handlers.rs`
- `src/rpc/types.rs`
- `src/rpc/mod.rs`

### Server (`server.rs`)

Routes:
- `POST /` -> JSON-RPC dispatcher
- `GET /health`
- `GET /ready`

Config currently includes:
- `listen_addr`
- `max_request_size` (declared but not enforced by layer)

### Dispatcher (`handlers.rs`)

Implemented methods:
- `broadcast_tx`
- `get_block`
- `get_block_hash`
- `get_account`
- `get_validators`
- `status`
- `get_latest_height`

Request handling style:
- simple method string match
- returns `JsonRpcResponse` success/error

Tx submission path:
- `broadcast_tx` decodes hex and does `try_send` into `tx_submit` channel.

### Health/readiness today

- `/health` checks storage height only and always returns `healthy: true`.
- `/ready` is true if `latest_height > 0`.

---

## 14) Feature: Metrics and Logging

### Files
- `src/metrics/registry.rs`
- `src/metrics/server.rs`
- `src/metrics/mod.rs`

### Metrics registry

Defines counters/gauges/histograms for:
- consensus
- p2p
- state execution
- storage
- channel drops
- rpc requests

Implementation detail:
- Registry wrapped in `Arc<Mutex<Registry>>`.

### Metrics server

- Axum server exposing `GET /metrics`.
- Uses `prometheus_client` text encoder.

### Logging

- Logging initialization in `main.rs` (`init_logging`).
- Supports JSON or text format via config.
- Uses `tracing` + `tracing-subscriber`.

---

## 15) Feature: Configuration System

### File
- `src/config/mod.rs`

### Current structure

Top-level `NodeConfig` sections:
- `node`
- `consensus`
- `p2p`
- `rpc`
- `mempool`
- `storage`
- `observability`
- `logging`

### Behavior

- Defaults are defined per field/section.
- `load(path)` parses TOML.
- `load_or_default(path)` falls back to full defaults on **any** load/parse error.

### Practical implications

- Easy for development bootstrap.
- Risky for operations because bad config can silently run with defaults.

---

## 16) Feature: Test Suite and Current Coverage Shape

### Files
- `tests/consensus_core_tests.rs`
- `tests/integration_tests.rs`
- `tests/proposer_tests.rs`
- `tests/vote_set_tests.rs`
- `tests/validator_set_tests.rs`
- `tests/wal_tests.rs`

### What tests currently cover

- Consensus FSM basic scenarios (single-node and event-level behavior)
- Proposer determinism and weighted selection behavior
- VoteSet duplicate/equivocation/quorum behavior
- Validator set update policy basics
- WAL write/read/truncate/encode-decode behavior
- Broad integration checks for block store, executor pipeline, RPC dispatch

### What test style to expect

- Mostly direct unit/integration tests with handcrafted fixtures.
- No property/fuzz harness and no dedicated multi-node in-process harness abstraction yet.

---

## 17) Feature: DevOps and Local Cluster Tooling

### Files
- `devops/Dockerfile`
- `devops/docker-compose.yml`
- `devops/prometheus.yml`
- `devops/config/node*/node.toml`
- `devops/scripts/init-cluster.sh`
- `devops/scripts/submit-tx.sh`
- `devops/scripts/simulate-failure.sh`
- `devops/grafana/provisioning/*`

### What exists now

Docker compose stack includes:
- 4 RustBFT nodes
- Prometheus
- Grafana

Scripts support:
- starting cluster
- submitting test tx via RPC
- simulating failures (kill/restart/partition/latency)

### Current ops shape

- Good for local experimentation.
- Grafana dashboards folder exists but dashboard JSON files are currently empty/missing.

---

## 18) End-to-end flows to understand before editing

## Flow A: Consensus command lifecycle

1. Consensus emits `ConsensusCommand` into crossbeam channel.
2. Router receives command and routes:
   - P2P broadcast
   - timer schedule/cancel
   - execute block
   - persist block
   - WAL write
3. Router may send events back to consensus (`BlockExecuted`, `TxsAvailable`).

Key files:
- `consensus/state.rs`
- `consensus/events.rs`
- `router/mod.rs`

## Flow B: Network message to consensus

1. Peer reader decrypts frame.
2. Decodes `MsgType` + payload to `NetworkMessage`.
3. Verifies signature via callback.
4. Sends `ConsensusEvent` via `to_consensus.try_send`.
5. Manager gossips message to peers.

Key files:
- `p2p/manager.rs`
- `p2p/msg.rs`
- `p2p/peer.rs`

## Flow C: Block execution and persistence

1. Consensus decides commit and sends `ExecuteBlock`.
2. Router executes block with `StateExecutor`.
3. Router emits `BlockExecuted` back to consensus.
4. Consensus sends `PersistBlock`.
5. Router writes block/state to RocksDB and truncates WAL.

Key files:
- `consensus/state.rs`
- `router/mod.rs`
- `state/executor.rs`
- `storage/block_store.rs`
- `storage/state_store.rs`
- `storage/wal.rs`

## Flow D: RPC transaction submit

1. Client calls `broadcast_tx` with hex bytes.
2. RPC handler decodes and `try_send`s to tx_submit channel.
3. In current startup wiring, receiver is dropped, so txs are not processed further.

Key files:
- `rpc/handlers.rs`
- `main.rs`

---

## 19) Safe review checklist before you start fixing

1. Confirm your local run path:
   - `cargo test`
   - start node with explicit config
2. Read and trace one full path in debugger/logs:
   - proposal -> vote -> execute -> persist
3. Identify boundary assumptions before each refactor:
   - canonical encoding boundary
   - signature verification boundary
   - sync/async channel boundary
   - crash consistency boundary (WAL + fsync)
4. Make changes in narrow slices:
   - one feature boundary at a time
   - add tests before broad rewires

---

## 20) Suggested reading order by maintenance goal

### If you want to fix consensus safety first
- `consensus/state.rs`
- `storage/wal.rs`
- `router/mod.rs`
- `main.rs` consensus deps setup

### If you want to fix networking and message authenticity
- `p2p/msg.rs`
- `p2p/peer.rs`
- `p2p/manager.rs`
- `crypto/ed25519.rs`

### If you want to fix state correctness
- `state/executor.rs`
- `state/accounts.rs`
- `state/merkle.rs`
- `storage/state_store.rs`

### If you want to fix operability
- `config/mod.rs`
- `main.rs` startup/shutdown
- `rpc/server.rs`, `rpc/handlers.rs`
- `metrics/registry.rs`

---

## 21) Reality check: current codebase maturity level

Current code has many solid scaffolds but still has placeholder behavior in critical paths. Treat it as:

- Good structural prototype
- Partially integrated feature set
- Not yet production-safe without substantial issue resolution

This is normal for AI-generated first-pass systems. The fastest path is to keep module boundaries and replace placeholder internals incrementally with tests.

