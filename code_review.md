# RustBFT Code Review

Review of the codebase against `plans/` and `docs/`. Each issue is classified by severity:
- **[CRITICAL]** — Safety or correctness violation; must be fixed before production
- **[MAJOR]** — Important feature gap that breaks a key contract in the plan
- **[MINOR]** — Deviation from the plan that reduces correctness or operability
- **[MVP-OK]** — Acknowledged in code as MVP limitation; acceptable for now

---

## 1. Foundation & Types

### [MAJOR] Duplicate transaction type systems
- `src/types/transaction.rs` defines `Transaction`, `TransferTx`, `ContractDeployTx`, etc.
- `src/state/tx.rs` defines a parallel `DecodedTx`, `TransferTx`, `ContractDeployTx`, etc. with different field names (`wasm`/`init_input` vs `code`/`init_args`)
- The executor uses `state/tx.rs`; the types module's `Transaction` is never used anywhere
- **Plan**: single canonical tx type in `types/transaction.rs`

### [MINOR] `tx_hash()` is a stub
- `src/types/transaction.rs:112` — `tx_hash(_tx) -> Hash([0u8; 32])` always returns zero
- **Plan**: deterministic canonical encoding + sha256

### [MINOR] No canonical serialization of Transaction for signing
- `Transaction` in `types/transaction.rs` is not serializable (no `Serialize`/`Deserialize`), so it cannot be signed or hashed canonically. Only the wire-decoded `DecodedTx` in `state/tx.rs` exists as bytes.

---

## 2. Consensus Core

### [CRITICAL] WAL writes are not synchronous before action
- `consensus/state.rs:706–730` — `wal_write()` sends a `ConsensusCommand::WriteWAL` through the crossbeam channel to the router, which processes it **asynchronously**.
- The BFT safety requirement (plan `plan-storage.md §7`) is: write WAL and `fsync` **before** broadcasting a vote/proposal. With async routing, the node can broadcast a vote before it is durably committed to the WAL.
- **Fix**: WAL writes must be synchronous and must happen before the broadcast command is sent.

### [CRITICAL] No actual vote/proposal signatures
- `consensus/state.rs:454,703` — proposals and votes are created with `signature: [0u8; 64]` (placeholder zeros)
- `main.rs:147–151` — `verify_proposal` and `verify_vote` closures always return `true`
- **Plan**: Ed25519 signing in the consensus thread before broadcasting; P2P verifies before forwarding
- No validator can distinguish authentic from forged messages in the current state

### [CRITICAL] State root mismatch not detected
- `router/mod.rs:128–153` — `execute_block()` returns `state_root`, but there is no check that `state_root == block.header.state_root`
- **Plan (`plan-failure-models.md §7`)**: state root mismatch must trigger `halt()` immediately (S0 critical failure)
- In the current code, `block.header.state_root` is always `Hash::ZERO` in proposals, so even if the check were added, it would require the proposer to fill in the correct pre-execution root

### [MAJOR] Missing `CommitStarted`/`CommitCompleted` WAL entries
- `storage/wal.rs` — `WalEntryKind` has `Proposal`, `Prevote`, `Precommit`, `Timeout`, `RoundStep`
- **Plan (`plan-storage.md §7`, `plan-failure-models.md §7`)**: `CommitStarted` and `CommitCompleted` entries are essential for crash recovery — specifically to detect "crash during block execution" (S1 failure)
- Without these, WAL recovery cannot distinguish a clean commit from a mid-execution crash

### [MAJOR] Missing `Locked`/`Unlocked` WAL entries
- When `locked_block`/`locked_round` changes in `state.rs`, no WAL entry is written
- **Plan**: recording `Locked` and `Unlocked` events is required to restore locking state after a crash, preventing double-voting

### [MAJOR] Equivocation not reported as a command
- `consensus/state.rs:350–358` — equivocation is detected and printed with `eprintln!()` but no `RecordEvidence` command is emitted to the router
- **Plan**: consensus emits `ConsensusCommand::RecordEvidence { evidence }` so evidence can be stored in the next block proposal and gossiped to peers

### [MINOR] `check_any_round_commit` scans hardcoded range
- `consensus/state.rs:534` — `for round in 0..=self.round.saturating_add(10)` is a heuristic; if votes arrive for a round > current+10 this commit check will miss them
- **Plan**: scan all rounds for which precommits have been received

---

## 3. Storage

### [CRITICAL] WAL `flush()` does not guarantee durability (missing fsync)
- `storage/wal.rs:126` — `self.file.flush()?` only flushes Rust's BufWriter to the OS buffer
- **Plan**: WAL must call `sync_all()` or `sync_data()` after each write to ensure the entry survives a power failure
- Without this, the WAL provides no crash-recovery guarantee

### [MAJOR] WAL checksum is SHA-256, not CRC32
- `storage/wal.rs:52,79` — uses `sha256()` as the checksum
- **Plan (`plan-storage.md §7`)**: specifies CRC32 (`[CRC32(4)][Len(4 BE)][Payload][Pad(0-3)]`)
- SHA-256 is stronger but the format is different from what `wal-inspect` and recovery algorithms expect. Any future tooling built to the spec will be incompatible.

### [MAJOR] WAL is text-based (hex lines), not binary
- `storage/wal.rs` — encodes each entry as a hex-encoded line (`writeln!(file, "{}", hex_line)`)
- **Plan**: binary format with fixed-width fields and 4-byte alignment padding
- Text WAL is ≈2× larger, slower, and incompatible with the binary spec in `plan-storage.md §7` and `plan-debugging-tools.md §7`

### [MAJOR] No `StorageEngine` trait / no MemoryStorageEngine for tests
- All storage code directly instantiates `rocksdb::DB`
- **Plan (`plan-storage.md §6`, `plan-testing.md §6`)**: `StorageEngine` trait with `MemoryStorageEngine` for deterministic unit tests
- Without the abstraction, unit tests that need storage must use real RocksDB and temporary directories

### [MAJOR] State stored as full JSON snapshot, not Sparse Merkle Trie
- `storage/state_store.rs:15` — stores entire `AppState` as JSON per height
- `state/merkle.rs` — `compute_state_root()` concatenates all account/code/storage bytes and hashes the result; this is not a Merkle trie
- **Plan**: versioned Sparse Merkle Trie where individual keys can be proven by Merkle proof
- Current design cannot produce inclusion/exclusion proofs; state root is not incremental

### [MAJOR] No dedicated storage thread or `StorageCommand` enum
- **Plan (`plan-storage.md §4`)**: storage runs on its own thread receiving `StorageCommand` via crossbeam channel
- **Actual**: storage operations are performed inline in the router's async `run()` loop
- This means slow disk I/O blocks the router loop, delaying P2P and timer commands

### [MINOR] Data directory layout deviates from plan
- `main.rs:96–101` — WAL opened at `{data_dir}/consensus.wal`
- **Plan**: WAL path is `{data_dir}/wal/consensus.wal`
- `main.rs` creates `{data_dir}/blocks` and `{data_dir}/state` but the plan specifies `{data_dir}/rocksdb` and `{data_dir}/state`

### [MINOR] No automatic pruning thread
- `block_store.rs` and `state_store.rs` have `prune_below()` methods, but there is no background thread calling them
- **Plan**: a pruning thread wakes every `pruning_interval_seconds` and deletes blocks/state below `latest_height - pruning_window`

### [MINOR] No transaction receipts stored
- **Plan**: `save_block` should also persist transaction receipts per block
- No `receipts` column family exists; `get_receipt` RPC is unimplementable

---

## 4. P2P Networking

### [MAJOR] No peer scoring or banning
- `p2p/manager.rs` — invalid proposals/votes are silently dropped with `continue`
- **Plan (`plan-p2p-networking.md §7`)**: each invalid message penalizes peer score by -50; peers reaching 0 are banned for `peer_ban_duration_ms`
- Without scoring, a Byzantine peer can spam invalid messages indefinitely

### [MAJOR] No reconnect with backoff
- `p2p/manager.rs:116–141` — seeds are connected once at startup; if the connection fails or drops, no reconnection is attempted
- **Plan**: exponential backoff reconnection up to `reconnect_backoff_max_ms = 60000`

### [MAJOR] Seed node ID not verified during handshake
- `p2p/manager.rs:377` — `parse_seed_addr` extracts only `host:port`, discarding the `node_id` part of `node_id@host:port`
- **Plan**: the node ID prefix is used to verify the peer's identity during the handshake exchange

### [MINOR] `max_peers` not enforced
- `P2pConfig` has `max_peers` but the accept loop in `manager.rs` never checks the current peer count before calling `spawn_peer`

### [MINOR] No keepalive / ping-pong logic
- `msg.rs` defines `Ping`/`Pong` messages but the manager never sends pings or times out silent peers
- **Plan**: ping every `ping_interval_ms`, disconnect if pong not received within `pong_timeout_ms`

---

## 5. Mempool

### [MAJOR] No mempool implementation
- `router/mod.rs:196–205` — `ReapTxs` always returns `TxsAvailable { txs: vec![] }`; `EvictTxs` is a no-op
- **Plan (`plan-mempool.md`)**: in-memory mempool with deduplication, signature validation, capacity limits (`max_txs`, `max_bytes`, `max_tx_bytes`), and gas limit checks

### [MAJOR] Submitted transactions are discarded
- `main.rs:226` — `let (tx_submit, _rx_submit) = mpsc::channel::<Vec<u8>>(1024);` — the receiver is immediately dropped
- Transactions submitted via `broadcast_tx` RPC are accepted into the channel, then dropped when the receiver is garbage-collected

---

## 6. Smart Contracts

### [MAJOR] Call depth limit not enforced
- `state/executor.rs:251–260` — `CallContext { call_depth: 0, .. }` is always hardcoded to 0
- **Plan (`plan-smart-contracts.md §7`)**: `max_call_depth = 64`; each nested call increments depth and returns error at the limit

### [MAJOR] Admin authorization not checked for ValidatorUpdateTx
- `state/executor.rs:138–153` — `ValidatorUpdateTx` is processed without checking if `from` is in the chain's `admin_addresses`
- **Plan**: only admin-authorized addresses can submit validator updates

### [MINOR] Cross-contract calls not implemented
- `contracts/host_api.rs` defines the host functions but `call_contract` (cross-contract invocation) is likely not wired to the contract runtime
- **Plan**: contracts can call other contracts via `env.call(addr, input, value, gas)` host function

---

## 7. Configuration & Operations

### [MAJOR] No environment variable (RUSTBFT_) overlay
- `config/mod.rs` — only reads from a TOML file; no env var parsing
- **Plan (`plan-operations.md §7`)**: `RUSTBFT_P2P_LISTEN_ADDR`, `RUSTBFT_CONSENSUS_TIMEOUT_PROPOSE_MS`, etc. must override file values

### [MAJOR] No configuration validation
- `NodeConfig::load_or_default()` falls back to defaults on any parse error, silently
- **Plan**: `validate_config()` must check address parseability (`SocketAddr::parse`), positive timeouts (> 0), key file existence, cross-field constraints (e.g., `max_gas_limit <= max_block_gas`)

### [MAJOR] No CLI subcommands (clap not used)
- `main.rs:35–38` — uses `std::env::args().nth(1)` to read config path; no argument parser
- **Plan (`plan-operations.md §6`, `plan-node-binary.md §6`)**: full `clap` command tree with `Start`, `Init`, `Keygen`, `Backup`, `Restore`, `WalInspect`, `StateDump`, `RepairDb`, `Replay` subcommands

### [MAJOR] No graceful 10-step shutdown sequence
- `main.rs:257–271` — shutdown is: receive Ctrl-C, log "Shutdown initiated", log "Draining connections", exit
- **Plan (`plan-operations.md §7`)**: stop RPC → drain in-flight requests (5s) → close peers (5s) → cancel timers → close consensus channel → join consensus thread (1s) → wait for state machine (10s) → flush storage (10s) → shutdown Tokio (5s) → exit 0

### [MAJOR] No SIGTERM handling
- `main.rs` only handles `ctrl_c` (SIGINT); SIGTERM (used by `docker stop` and `systemd`) is not handled
- **Plan**: both SIGTERM and SIGINT trigger the graceful shutdown sequence

### [MINOR] `key_file` missing from NodeSection
- `config/mod.rs` — `NodeSection` has no `key_file` field; key path is hardcoded to `{data_dir}/node_key.json` in `main.rs`
- **Plan**: `node.key_file = "./config/node_key.json"` is a configurable field

### [MINOR] Genesis file not loaded or validated at startup
- `main.rs` — no genesis file is read; validator set is bootstrapped as a single-validator set from the node's own key
- **Plan**: `init` subcommand validates and loads `genesis.json`; at startup, the validator set is loaded from the genesis file if no committed blocks exist

---

## 8. RPC API

### [MAJOR] No rate limiting
- `rpc/server.rs` — no middleware for rate limiting
- **Plan (`plan-rpc-api.md §7`)**: token-bucket limiter, `rate_limit_per_second = 100`, burst of 200; `broadcast_tx` has a stricter limit of 10/s

### [MAJOR] No request body size limit enforced
- `RpcConfig.max_request_size` is defined but never applied to the axum router (no `RequestBodyLimitLayer`)

### [MAJOR] `broadcast_tx_commit` not implemented
- Only `broadcast_tx` (fire-and-forget) exists; `broadcast_tx_commit` that blocks until the tx appears in a committed block is missing
- **Plan (`plan-rpc-api.md §7`)**: subscribes to a `tokio::sync::broadcast` channel on the commit path and polls until the tx receipt is available

### [MINOR] Missing endpoints
- `get_receipt`, `get_state_root`, `query_contract` are not implemented
- **Plan**: these are among the 10 defined JSON-RPC endpoints

### [MINOR] CORS not configured
- `tower-http` is in `Cargo.toml` with `cors` feature, but no `CorsLayer` is applied in `rpc/server.rs`
- **Plan**: `cors_allowed_origins = ["*"]` by default

### [MINOR] Health check does not check consensus staleness or peer count
- `rpc/server.rs:74–86` — health only checks `latest_height` in storage
- **Plan (`plan-observability.md §5`)**: health is unhealthy if consensus hasn't advanced in >30s, or if peer count is 0

---

## 9. Observability / Metrics

### [MAJOR] Consensus metrics never updated by consensus core
- The consensus core (`consensus/state.rs`) never calls metrics; it has no reference to `Metrics`
- `consensus_height` is only updated in the router after `PersistBlock`
- `consensus_round`, `consensus_proposals_received`, `consensus_votes_received`, `consensus_timeouts`, `consensus_equivocations` are never updated anywhere in the codebase

### [MINOR] `storage_wal_entries` gauge never updated
- Gauge exists in `metrics/registry.rs` but no code calls `storage_wal_entries.set()`

### [MINOR] `channel_drops` counter never incremented
- When `to_consensus.try_send()` fails in P2P (channel full), the counter is not incremented
- **Plan**: track every dropped message to detect backpressure

### [MINOR] Missing Grafana dashboard JSON files
- `devops/grafana/provisioning/` exists with datasource and dashboard provisioning YAML, but `devops/grafana/dashboards/` is empty — no `consensus.json`, `network.json`, `state-machine.json`, `infrastructure.json`

### [MINOR] Security checklist startup log not implemented
- **Plan**: a DEBUG-level startup log checks key file permissions, RPC binding address, admin key presence

---

## 10. Event Loop & Threading

### [MAJOR] Router blocks Tokio thread on crossbeam recv
- `router/mod.rs:101` — `self.rx_cmd.recv()` is a **blocking** crossbeam call inside an `async fn`
- This starves other Tokio tasks on the same thread whenever the consensus channel is empty
- **Plan (`plan-event-loop-threading.md §9`)**: the router should run on a dedicated OS thread via `std::thread::spawn`, or use `tokio::task::spawn_blocking`

### [MINOR] Consensus command channel capacity mismatch
- `main.rs:74` — `tx_consensus_cmd` channel is `bounded(1024)`
- **Plan**: `CAP_CONSENSUS_CMD = 256` (commands are consumed quickly; 1024 allows unbounded buffering and can mask backpressure)

---

## 11. Testing

### [MAJOR] No property/fuzz tests
- No `proptest` or `cargo-fuzz` integration
- **Plan (`plan-testing.md §12`)**: `prop_no_double_commit`, `prop_commit_requires_quorum`, `prop_serialize_round_trip`, `prop_contract_no_host_panic`

### [MAJOR] No Docker E2E tests
- `bollard` is not in `Cargo.toml`; `DockerHarness` is not implemented
- **Plan**: 12 Docker E2E test scenarios covering cluster health, crash recovery, network partition, contract deploy, metrics scraping

### [MAJOR] No deterministic replay tests
- `EventRecorder` is not implemented; `RUSTBFT_RECORD_EVENTS` env var is not supported
- **Plan**: `test_replay_same_output`, `test_replay_cross_architecture`

### [MAJOR] No crash recovery test infrastructure
- `CrashableWAL` is not implemented
- **Plan**: `test_crash_after_prevote_before_precommit`, `test_crash_after_lock`, `test_crash_during_block_execution`

### [MAJOR] No `ConsensusHarness` for multi-node in-process tests
- Tests in `tests/consensus_core_tests.rs` test a single node; no harness exists for routing messages between N consensus cores
- **Plan**: `test_no_double_commit_4_honest`, `test_partition_no_commits`, liveness tests require this

### [MINOR] Missing named test functions from plan
- `test_happy_path_4_validators` (only 1-validator exists), `test_locking_refuses_conflicting_block`, `test_unlock_on_nil_polka`, `test_commit_from_propose_step`, `test_proposer_selection_deterministic`, `test_quorum_integer_math`
- **Plan (`plan-testing.md §12`)**: all consensus unit tests are named specifically

### [MINOR] No coverage enforcement
- `cargo-tarpaulin` is not configured; no CI target for >80% line coverage

---

## 12. Failure Models

### [CRITICAL] No `halt()` function
- `src/node/failure.rs` does not exist
- **Plan (`plan-failure-models.md §6`)**: `pub fn halt(reason: &str) -> !` calls `std::process::exit(1)` and is invoked on every S0 failure
- Current code uses `eprintln!()` for equivocation and silently swallows validator set rejection errors; no S0 path triggers process exit

### [CRITICAL] No startup integrity check
- `main.rs` loads `last_height` and `app_state` but never verifies:
  1. Block bytes at `last_height` hash to the stored block hash
  2. Stored state root at `last_height` matches `block.header.state_root`
- **Plan (`plan-failure-models.md §7`)**: `check_startup_integrity()` runs before entering consensus; mismatch → halt

### [MAJOR] No panic hook installed
- **Plan (`plan-failure-models.md §6`)**: `install_panic_hook()` sets a `std::panic::set_hook` that logs the panic at ERROR level and exits cleanly
- A thread panic currently just kills that thread silently

---

## 13. Debugging Tools

### [MAJOR] No debugging subcommands implemented
- `wal-inspect`, `state-dump`, `repair-db`, `replay` are described in `plan-debugging-tools.md` but none exist in the codebase

### [MAJOR] No event recording
- `RUSTBFT_RECORD_EVENTS=true` and `/data/debug/events.log` are not implemented
- **Plan**: the consensus event loop writes every `ConsensusEvent` to a JSON-lines file when enabled

### [MINOR] No `collect-debug-bundle.sh` script
- **Plan**: `devops/scripts/collect-debug-bundle.sh` collects logs, WAL state, metrics, and node status into a tarball for bug reports. Only `init-cluster.sh`, `submit-tx.sh`, `simulate-failure.sh` exist.

---

## 14. CLI Tool

### [MAJOR] No `rustbft-cli` binary
- **Plan (`plan-cli-tool.md`)**: a separate `rustbft-cli` binary for submitting transactions, querying state, and managing keys
- Only `rustbft-node` exists (and even that lacks subcommands)

---

## 15. Docker & DevOps

### [MINOR] No Grafana dashboard JSON files
- `devops/grafana/provisioning/` exists but `devops/grafana/dashboards/` has no JSON files
- **Plan**: 4 pre-built dashboards: `consensus.json`, `network.json`, `state-machine.json`, `infrastructure.json`

### [MINOR] Dockerfile base image note
- `devops/Dockerfile` uses `rust:1.75-bookworm` but `Cargo.toml` specifies `edition = "2024"` which requires Rust ≥ 1.85. The Dockerfile image version would need updating.

---

## 16. Block Sync

### [CRITICAL] No block sync implementation — node cannot rejoin after downtime

The most operationally significant gap in the codebase. The following scenarios all result in the node being permanently stuck at its last known height with no path to recovery:

| Scenario | Current behavior |
|---|---|
| Node restarts after maintenance | WAL recovers to crash height; cannot obtain blocks committed while offline |
| New node joins an existing network | Starts at height 1 with no blocks; cannot participate in consensus |
| Network partition heals | Minority-side nodes are behind by N heights; cannot rejoin |
| Backup restore | Node is restored to backup height; cannot catch up to current height |
| Rolling restart (one node at a time) | Each restarted node is permanently stranded |

**What is missing** (per `docs/architecture/block-sync.md` and `plans/plan-block-sync.md`):
- `SyncRequest` / `SyncResponse` P2P messages (`MsgType 0x08`, `0x09`) — not in `p2p/msg.rs`
- `StatusRequest` / `StatusResponse` P2P messages (`MsgType 0x0A`, `0x0B`) — not in `p2p/msg.rs`
- `SyncManager` struct and `discover()` / `run()` methods — `src/sync/` directory does not exist
- `verify_commit_info()` — CommitInfo signature verification for synced blocks
- Integration into `main.rs`: after WAL recovery, call `SyncManager::discover()` and `SyncManager::run()` before spawning the consensus thread
- Sync metrics: `rustbft_sync_mode`, `rustbft_sync_target_height`, `rustbft_sync_blocks_applied_total`, `rustbft_sync_peer_failures_total`, `rustbft_sync_duration_seconds`
- `[sync]` section in `NodeConfig` / `config/mod.rs`

**Secondary impact**: `CommitInfo` in `Block` (defined in `types/block.rs`) is populated with precommit signatures during live consensus, but these signatures are placeholder zeros (`[0u8; 64]`) in all proposals today. Block sync verification requires real signatures in `CommitInfo` — this issue is coupled to the [CRITICAL] missing vote signatures issue in §2.

---

## Summary Table

| Area | Critical | Major | Minor |
|------|----------|-------|-------|
| Foundation & Types | 0 | 1 | 2 |
| Consensus Core | 3 | 3 | 1 |
| Storage | 1 | 5 | 3 |
| P2P Networking | 0 | 3 | 2 |
| Mempool | 0 | 2 | 0 |
| Smart Contracts | 0 | 2 | 1 |
| Configuration & Operations | 0 | 5 | 3 |
| RPC API | 0 | 3 | 3 |
| Observability | 0 | 1 | 4 |
| Event Loop & Threading | 0 | 1 | 1 |
| Testing | 0 | 5 | 3 |
| Failure Models | 2 | 1 | 0 |
| Debugging Tools | 0 | 2 | 1 |
| CLI Tool | 0 | 1 | 0 |
| Docker & DevOps | 0 | 0 | 2 |
| **Total** | **6** | **35** | **26** |

---

## Priority Fix Order

### Must fix before any multi-node test:
1. WAL fsync (`sync_all()` after every write) — crash safety is illusory without it
2. WAL write before action (synchronous, not async via channel) — BFT liveness invariant
3. Ed25519 signing of proposals and votes — identity is unenforced otherwise
4. `halt()` function and startup integrity check — S0 failures silently continue

### Must fix for correct BFT protocol:
5. `CommitStarted`/`CommitCompleted` and `Locked`/`Unlocked` WAL entries
6. State root mismatch detection after block execution
7. Equivocation emitted as `RecordEvidence` command
8. Blocking crossbeam recv in async router → move to dedicated thread

### Must fix for operability:
9. Mempool implementation (even a minimal one with deduplication)
10. SIGTERM handler + graceful shutdown sequence
11. RUSTBFT_ environment variable config overlay
12. Config validation at startup
13. Admin authorization check for ValidatorUpdateTx
