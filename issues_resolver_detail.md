# RustBFT Issues Resolver Detail

This document is the implementation playbook for resolving all issues listed in `code_review.md` against the current codebase (`src/`), plans (`plans/`), and architecture/ops docs (`docs/`).

## 0) How to execute this document

1. Follow issue IDs in order of dependency waves (Wave 1 -> Wave 6).
2. For each issue:
   - Apply the code changes in `Code Touchpoints`.
   - Complete `Implementation Steps`.
   - Run `Verification` before moving on.
3. Do not start multi-node or Docker validation until all Wave 1 and Wave 2 criticals are done.
4. Keep all consensus-path serialization deterministic and canonical.

## 1) Dependency waves (recommended order)

- Wave 1 (safety-critical blockers): CNS-01, STG-01, CNS-02, FLR-01, FLR-02
- Wave 2 (consensus correctness): CNS-03..CNS-07, STG-02, STG-03, STG-06, EVT-01
- Wave 3 (operability baseline): MMP-01, MMP-02, OPS-01..OPS-07, RPC-01..RPC-06
- Wave 4 (network hardening): P2P-01..P2P-05, OBS-01..OBS-05
- Wave 5 (testing and tooling): TST-01..TST-07, DBG-01..DBG-03, CLI-01
- Wave 6 (sync/rejoin and production polish): BSY-01, DKR-01, DKR-02

## 2) Cross-cutting implementation rules

- Canonical signing bytes must use `src/types/serialization.rs` (not `serde_json`).
- Consensus must remain single-threaded and synchronous (`src/consensus/*` no Tokio awaits).
- Any S0 failure path must call a central `halt()`.
- WAL durability means `write + flush + sync_all/sync_data` before returning success.
- Use deterministic containers (`BTreeMap`, sorted vectors) on consensus/execution path.

---

## 3) Issue-by-issue implementation instructions

## Foundation & Types

### FND-01 [MAJOR] Duplicate transaction type systems
Current gap:
- `src/types/transaction.rs` defines canonical tx types but executor path uses `src/state/tx.rs` (`DecodedTx` variants).

Implementation steps:
1. Make `types::transaction` the single source of truth:
   - Introduce `SignedTransaction` + `Transaction` variants as canonical runtime type.
   - Align field names with plans (`code`, `init_args`, `input`, `amount`, `new_power`, etc.).
2. Move wire decoding to `types/serialization.rs` (or dedicated `types/transaction_codec.rs`) returning canonical `SignedTransaction`.
3. Remove or deprecate parallel structs in `state/tx.rs`; keep only parser helpers if needed.
4. Update `state/executor.rs` to consume canonical transaction type instead of `DecodedTx`.
5. Update mempool/RPC/P2P interfaces to pass canonical signed tx bytes and decoded canonical type.

Code touchpoints:
- `src/types/transaction.rs`
- `src/state/tx.rs`
- `src/state/executor.rs`
- `src/rpc/handlers.rs`
- `src/consensus/events.rs` (tx container types)

Verification:
- Transaction decode round-trip tests pass for all tx variants.
- Executor tests run without referencing `state::tx::DecodedTx`.

### FND-02 [MINOR] `tx_hash()` is stubbed
Implementation steps:
1. Implement `tx_hash(tx)` as `sha256(canonical_encode(tx))`.
2. Ensure canonical encoding includes all signed fields and variant discriminant.
3. Add deterministic vectors in tests for each tx variant.

Code touchpoints:
- `src/types/transaction.rs`
- `src/types/serialization.rs`

Verification:
- Same tx bytes produce same hash across repeated runs.
- Distinct nonce/signature/field changes alter hash.

### FND-03 [MINOR] No canonical serialization for signing
Implementation steps:
1. Derive/sign over explicit signing payload (exclude signature field).
2. Add methods:
   - `SignedTransaction::signing_bytes()`
   - `SignedVote::signing_bytes()`
   - `SignedProposal::signing_bytes()`
3. Ensure verify functions in crypto and P2P use these bytes.
4. Remove any signing/verification using JSON serialization.

Code touchpoints:
- `src/types/serialization.rs`
- `src/types/transaction.rs`
- `src/types/vote.rs`
- `src/types/proposal.rs`
- `src/crypto/ed25519.rs`

Verification:
- Signature verification fails if any signed field mutates.

---

## Consensus Core

### CNS-01 [CRITICAL] WAL writes are asynchronous before action
Implementation steps:
1. Replace fire-and-forget WAL command path with synchronous WAL ack path.
2. Extend `ConsensusCommand::WriteWAL` to carry responder (`oneshot`/crossbeam ack).
3. In consensus state methods (`wal_write` call sites), block until WAL write+fsync returns success.
4. Only then emit `BroadcastProposal`/`BroadcastVote`/`ExecuteBlock` side-effect commands.
5. On WAL write failure, trigger `halt("wal write failed")`.

Code touchpoints:
- `src/consensus/events.rs`
- `src/consensus/state.rs` (proposal/vote/commit paths)
- `src/router/mod.rs` (WAL write handling and ack)
- `src/storage/wal.rs`

Verification:
- Inject failing WAL writer: broadcast command must not be emitted.

### CNS-02 [CRITICAL] No real vote/proposal signatures
Implementation steps:
1. Add node signing key to consensus state dependencies.
2. Sign proposal in proposer path using canonical proposal bytes.
3. Sign votes in `broadcast_vote` using canonical vote bytes.
4. Wire real verification closures in `main.rs` and P2P manager using validator pubkeys.
5. Reject invalid signatures before forwarding to consensus.

Code touchpoints:
- `src/consensus/state.rs`
- `src/main.rs` (deps and verify closures)
- `src/p2p/manager.rs`
- `src/crypto/ed25519.rs`
- `src/types/validator.rs` (store public keys in validator struct)

Verification:
- Forged vote/proposal from non-validator or bad signature is dropped and penalized.

### CNS-03 [CRITICAL] State root mismatch not checked
Implementation steps:
1. In router post-execution flow, compare computed `state_root` with `block.header.state_root`.
2. If mismatch: log ERROR with expected/computed/height and call `halt()`.
3. Ensure proposer fills `block.header.state_root` correctly before broadcast:
   - either execute proposal block before proposing, or use deterministic pre-execution design from plan.
4. Update block build path so `state_root` is no longer `Hash::ZERO` placeholder.

Code touchpoints:
- `src/router/mod.rs` (`ExecuteBlock` branch)
- `src/consensus/state.rs` (`handle_txs_available` block construction)

Verification:
- Tampered header state root causes immediate process halt.

### CNS-04 [MAJOR] Missing `CommitStarted`/`CommitCompleted` WAL entries
Implementation steps:
1. Extend `WalEntryKind` with commit lifecycle entries.
2. Emit `CommitStarted` immediately before `ExecuteBlock`.
3. Emit `CommitCompleted` after block+state persist succeeds.
4. Update WAL recovery parser to detect crash-in-commit and set re-execution flag.

Code touchpoints:
- `src/storage/wal.rs`
- `src/consensus/state.rs`
- `src/router/mod.rs`

Verification:
- Crash between started/completed is detected and recoverable.

### CNS-05 [MAJOR] Missing `Locked`/`Unlocked` WAL entries
Implementation steps:
1. Emit `Locked` when `locked_block/locked_round` becomes `Some`.
2. Emit `Unlocked` when nil polka unlocks.
3. Replay these in WAL recovery to restore lock state before consensus resumes.

Code touchpoints:
- `src/consensus/state.rs` (`compute_precommit` and lock transitions)
- `src/storage/wal.rs` recovery layer

Verification:
- Recovery reproduces locked state and prevents conflicting prevote after restart.

### CNS-06 [MAJOR] Equivocation not emitted as command
Implementation steps:
1. Add `ConsensusCommand::RecordEvidence`.
2. When VoteSet returns equivocation, emit command with last evidence object.
3. Router persists evidence and forwards to P2P evidence gossip.

Code touchpoints:
- `src/consensus/events.rs`
- `src/consensus/state.rs`
- `src/router/mod.rs`
- `src/p2p/msg.rs` / `src/p2p/manager.rs`

Verification:
- Equivocation increments metric and generates stored/gossiped evidence event.

### CNS-07 [MINOR] `check_any_round_commit` scans hardcoded range
Implementation steps:
1. Add VoteSet API to iterate rounds observed for precommits.
2. Replace `0..=self.round+10` with iteration over real rounds present.
3. Keep deterministic order (ascending round).

Code touchpoints:
- `src/consensus/vote_set.rs`
- `src/consensus/state.rs`

Verification:
- Precommits at far future round still trigger commit detection.

---

## Storage

### STG-01 [CRITICAL] WAL flush without fsync
Implementation steps:
1. After writing an entry: `flush()` then `sync_data()` (or `sync_all()`).
2. Return error if either fails.
3. Measure and record fsync latency metric.

Code touchpoints:
- `src/storage/wal.rs`
- `src/router/mod.rs` (metric timing)

Verification:
- Fault-injection test confirms write is not acknowledged if sync fails.

### STG-02 [MAJOR] WAL checksum format mismatch (SHA-256 vs CRC32)
Implementation steps:
1. Redefine WAL frame as `[crc32][len_be][payload][pad]`.
2. Replace SHA-256 entry checksum with CRC32 implementation.
3. Update reader to stop on first CRC mismatch/truncated payload.

Code touchpoints:
- `src/storage/wal.rs`

Verification:
- Corrupt entry test detects CRC mismatch and truncates replay.

### STG-03 [MAJOR] WAL text format vs binary spec
Implementation steps:
1. Remove hex line encoding and `BufRead` text replay.
2. Switch to binary append/read with alignment padding.
3. Add WAL inspect helper compatibility with binary format.

Code touchpoints:
- `src/storage/wal.rs`
- future `src/tools/wal_inspect.rs`

Verification:
- WAL size and write throughput improve; recovery works with binary frames.

### STG-04 [MAJOR] Missing `StorageEngine` trait and memory engine
Implementation steps:
1. Add `storage/engine.rs` trait + batch ops abstraction.
2. Wrap RocksDB implementation behind trait.
3. Add in-memory deterministic engine for unit/property tests.
4. Refactor block/state stores to depend on trait object/generic.

Code touchpoints:
- `src/storage/engine.rs` (new)
- `src/storage/block_store.rs`
- `src/storage/state_store.rs`
- tests/harness modules

Verification:
- Storage test suite runs on both RocksDB and memory engines.

### STG-05 [MAJOR] State storage is snapshot hashing, not sparse Merkle
Implementation steps:
1. Add trie module (`src/storage/trie.rs`) with versioned sparse Merkle structure.
2. Convert state updates to key-value changesets, recompute incremental root.
3. Persist trie nodes and height->root mapping.
4. Support inclusion/exclusion proof generation API.

Code touchpoints:
- `src/storage/state_store.rs`
- `src/state/merkle.rs` (replace/bridge)
- `src/state/accounts.rs` (changeset extraction)

Verification:
- Same changeset on fresh state produces identical root; proof verification tests pass.

### STG-06 [MAJOR] No storage thread / `StorageCommand`
Implementation steps:
1. Introduce storage thread loop owning WAL + RocksDB writer.
2. Add `StorageCommand` enum (`WriteWAL`, `PersistBlock`, `ApplyChangeset`, `Flush`, etc.).
3. Router and other modules send commands via bounded channel.
4. Keep read path via `StorageReader` snapshot APIs.

Code touchpoints:
- `src/storage/mod.rs`
- `src/main.rs`
- `src/router/mod.rs`

Verification:
- Slow disk write no longer blocks router/Tokio event loop.

### STG-07 [MINOR] Data directory layout mismatch
Implementation steps:
1. Standardize paths to `{data_dir}/rocksdb`, `{data_dir}/state`, `{data_dir}/wal/consensus.wal`.
2. Update startup directory creation and config defaults.
3. Update scripts/docs and backward-compatible migration check.

Code touchpoints:
- `src/main.rs`
- `src/config/mod.rs`
- `devops/config/*`

Verification:
- Fresh init creates expected structure; existing old path emits migration warning.

### STG-08 [MINOR] Missing pruning background thread
Implementation steps:
1. Add pruning thread waking every `pruning_interval_seconds`.
2. Compute `min_height = latest - pruning_window` and prune state snapshots/trie nodes.
3. Keep blocks and WAL untouched.

Code touchpoints:
- `src/storage/mod.rs`
- `src/storage/state_store.rs`
- config section extensions

Verification:
- With small window in test, old state heights become unavailable as expected.

### STG-09 [MINOR] Missing transaction receipts persistence
Implementation steps:
1. Extend block persistence schema with receipts CF/keys.
2. Persist `(height, tx_index) -> receipt` and `tx_hash -> (height, index)` index.
3. Add `get_receipt`/`get_tx` reader APIs for RPC.

Code touchpoints:
- `src/storage/block_store.rs`
- `src/state/executor.rs` (receipt generation)
- `src/rpc/handlers.rs`

Verification:
- Receipt query returns deterministic execution result for committed tx.

---

## P2P Networking

### P2P-01 [MAJOR] No peer scoring/banning
Implementation steps:
1. Add `PeerState { score, banned_until, failures }` map in manager.
2. Penalize invalid format/signature/equivocation/timeouts using plan penalties.
3. Disconnect and ban when score <= 0; reject reconnect until ban expires.

Code touchpoints:
- `src/p2p/manager.rs`
- `src/p2p/config.rs`

Verification:
- Invalid-message spam peer is eventually banned and ignored.

### P2P-02 [MAJOR] No reconnect with backoff
Implementation steps:
1. Track seed peers and persistent reconnect attempts.
2. Implement exponential backoff capped by `reconnect_backoff_max_ms`.
3. Reset backoff on successful reconnect.

Code touchpoints:
- `src/p2p/manager.rs`

Verification:
- Dropped seed reconnect attempts follow 1s,2s,4s...60s pattern.

### P2P-03 [MAJOR] Seed node ID not verified
Implementation steps:
1. Parse seeds into `(expected_node_id, addr)` from `node_id@host:port`.
2. Pass expected ID into outbound handshake context.
3. Validate handshake peer node_id equals expected node_id.

Code touchpoints:
- `src/p2p/manager.rs` (`parse_seed_addr` -> structured parser)
- `src/p2p/peer.rs` (expected peer id check)

Verification:
- Outbound connection with mismatched peer identity is rejected.

### P2P-04 [MINOR] `max_peers` not enforced
Implementation steps:
1. Before accepting/spawning inbound peer, check current peer count.
2. Refuse or immediately disconnect when at capacity.
3. Expose metric/log for refused peers due to max cap.

Code touchpoints:
- `src/p2p/manager.rs`

Verification:
- When max reached, additional inbound peers are rejected.

### P2P-05 [MINOR] Ping/pong keepalive missing
Implementation steps:
1. Add periodic ping task per peer using `ping_interval_ms`.
2. Track last pong and disconnect if overdue by `pong_timeout_ms`.
3. Penalize no-pong peers.

Code touchpoints:
- `src/p2p/manager.rs`
- `src/p2p/msg.rs`

Verification:
- Silent peer is disconnected after timeout.

---

## Mempool

### MMP-01 [MAJOR] No mempool implementation
Implementation steps:
1. Create `src/mempool` module with single-thread loop + command enum.
2. Implement add/reap/evict/stats with dedup and byte/count caps.
3. Add stateless tx validation (size, gas limit, signature).
4. Wire consensus `ReapTxs`/`EvictTxs` router path to mempool thread.

Code touchpoints:
- `src/mempool/mod.rs` (new)
- `src/main.rs`
- `src/router/mod.rs`
- `src/rpc/handlers.rs`
- `src/p2p/manager.rs` (tx gossip path)

Verification:
- `ReapTxs` returns deterministic non-empty set after submissions.

### MMP-02 [MAJOR] RPC submitted txs are dropped
Implementation steps:
1. Replace dropped receiver `_rx_submit` with real mempool submission consumer.
2. Bridge `RpcState.tx_submit` channel into mempool add command path.
3. Return correct accepted/rejected response based on mempool result.

Code touchpoints:
- `src/main.rs` line around `tx_submit`
- `src/rpc/handlers.rs`

Verification:
- `broadcast_tx` increases mempool size and tx can be proposed in block.

---

## Smart Contracts

### SCT-01 [MAJOR] Call depth limit not enforced end-to-end
Implementation steps:
1. Add `contracts.max_call_depth` to config.
2. Propagate call depth through executor -> runtime -> host cross-call.
3. Reject nested call when depth >= max.
4. Ensure error semantics match plan (callee failure does not corrupt caller state).

Code touchpoints:
- `src/config/mod.rs`
- `src/state/executor.rs`
- `src/contracts/runtime.rs`
- `src/contracts/host_api.rs`

Verification:
- 65th nested call fails with deterministic error when max=64.

### SCT-02 [MAJOR] No admin authorization for ValidatorUpdateTx
Implementation steps:
1. Add chain/admin addresses to state/genesis config model.
2. On validator-update tx execution, verify `from` in admin list.
3. Reject unauthorized updates while preserving block validity semantics.

Code touchpoints:
- `src/state/accounts.rs` (chain params/admin list)
- `src/state/executor.rs`
- genesis loading path in `main.rs`

Verification:
- Unauthorized update tx fails and does not mutate validator set.

### SCT-03 [MINOR] Cross-contract call wiring incomplete
Implementation steps:
1. Ensure `host_call_contract` invokes runtime `call` with child context.
2. Preserve nested gas accounting and event propagation.
3. Ensure callee failure reverts callee-only changes and returns failure to caller.

Code touchpoints:
- `src/contracts/host_api.rs`
- `src/contracts/runtime.rs`

Verification:
- Contract A calling B succeeds/fails according to plan scenarios.

---

## Configuration & Operations

### OPS-01 [MAJOR] No `RUSTBFT_` env overlay
Implementation steps:
1. Add `resolve_config` flow: defaults -> file -> env -> CLI.
2. Map env names to nested config fields.
3. Parse typed overrides with explicit error reporting.

Code touchpoints:
- `src/config/mod.rs`
- `src/main.rs` startup path

Verification:
- Env vars override TOML values in startup logs and runtime behavior.

### OPS-02 [MAJOR] No config validation
Implementation steps:
1. Implement `validate_config()` returning structured errors.
2. Validate socket addresses, positive timeouts, path existence, cross-field constraints.
3. Replace silent fallback in `load_or_default`; fail-fast on invalid explicit config.

Code touchpoints:
- `src/config/mod.rs`
- `src/main.rs`

Verification:
- Invalid config exits with clear validation diagnostics.

### OPS-03 [MAJOR] Missing CLI subcommands (clap)
Implementation steps:
1. Introduce `clap` command tree in `main.rs` or `src/node/cli.rs`.
2. Implement at least: `start`, `init`, `keygen`, `backup`, `restore`, `wal-inspect`, `state-dump`, `repair-db`, `replay`.
3. Keep current start behavior under `start` subcommand.

Code touchpoints:
- `Cargo.toml` (add `clap`)
- `src/main.rs`
- new `src/node/*` or `src/tools/*`

Verification:
- `--help` shows full subcommand tree and each path executes.

### OPS-04 [MAJOR] Missing graceful 10-step shutdown
Implementation steps:
1. Implement shutdown coordinator with explicit ordered steps and per-step timeout.
2. Stop RPC intake, drain requests, disconnect peers, cancel timers, close channels.
3. Join consensus/state/storage threads with bounded waits.
4. Flush storage/WAL and shutdown Tokio runtime.

Code touchpoints:
- `src/main.rs`
- `src/router/mod.rs`
- `src/p2p/manager.rs`
- storage thread module

Verification:
- SIGTERM/SIGINT logs all steps and exits code 0 when successful.

### OPS-05 [MAJOR] No SIGTERM handling
Implementation steps:
1. Add SIGTERM listener (`tokio::signal::unix::signal(SignalKind::terminate())`).
2. Merge with Ctrl-C path into same shutdown trigger.

Code touchpoints:
- `src/main.rs`

Verification:
- `docker stop` triggers graceful shutdown path, not abrupt exit.

### OPS-06 [MINOR] `node.key_file` missing in config
Implementation steps:
1. Add `key_file: String` to `NodeSection` with default.
2. Use configured path in key loading instead of hardcoded `{data_dir}/node_key.json`.

Code touchpoints:
- `src/config/mod.rs`
- `src/main.rs`

Verification:
- Custom key path works and is validated.

### OPS-07 [MINOR] Genesis not loaded/validated
Implementation steps:
1. Add genesis config type and loader/validator.
2. On startup:
   - if no committed blocks, load validator set + chain params from genesis.
   - verify genesis consistency and chain_id.
3. Replace single-validator bootstrap fallback.

Code touchpoints:
- new `src/types/genesis.rs`
- `src/main.rs`
- `src/types/validator.rs`
- `src/state/accounts.rs`

Verification:
- Multi-validator genesis boots correctly; invalid genesis halts startup.

---

## RPC API

### RPC-01 [MAJOR] Missing rate limiting
Implementation steps:
1. Add token-bucket limiter middleware keyed by client IP.
2. Add stricter bucket for `broadcast_tx`/`broadcast_tx_commit`.
3. Return JSON-RPC error code for rate-limited requests.

Code touchpoints:
- `src/rpc/server.rs`
- `src/rpc/handlers.rs`
- `src/config/mod.rs` (burst and per-method limits)

Verification:
- Burst above configured limit yields rate-limit errors.

### RPC-02 [MAJOR] Max request body size not enforced
Implementation steps:
1. Apply `RequestBodyLimitLayer` with `RpcConfig.max_request_size`.
2. Ensure 413 is mapped to JSON-RPC parse/request error payload where needed.

Code touchpoints:
- `src/rpc/server.rs`

Verification:
- Oversized body is rejected consistently.

### RPC-03 [MAJOR] `broadcast_tx_commit` missing
Implementation steps:
1. Add commit notification channel (`tokio::sync::broadcast`) in node state.
2. On block commit, publish committed tx hashes/receipts.
3. RPC handler submits tx then waits until commit notification or timeout.

Code touchpoints:
- `src/rpc/handlers.rs`
- `src/main.rs` / router commit path
- storage receipt index APIs

Verification:
- `broadcast_tx_commit` returns receipt when tx commits, timeout otherwise.

### RPC-04 [MINOR] Missing endpoints (`get_receipt`, `get_state_root`, `query_contract`)
Implementation steps:
1. Add methods to RPC dispatcher and typed params/results.
2. Implement read paths via storage reader and read-only contract runtime call.

Code touchpoints:
- `src/rpc/handlers.rs`
- `src/rpc/types.rs`
- storage read APIs

Verification:
- Endpoints return deterministic data and correct not-found/pruned errors.

### RPC-05 [MINOR] CORS not configured
Implementation steps:
1. Add `CorsLayer` with configurable allowed origins.
2. Default to `*` in development config.

Code touchpoints:
- `src/rpc/server.rs`
- `src/config/mod.rs`

Verification:
- Preflight requests receive expected CORS headers.

### RPC-06 [MINOR] Health check lacks consensus staleness and peer checks
Implementation steps:
1. Extend shared health probe state with:
   - last commit timestamp
   - peer count
   - storage height
2. `/health` returns unhealthy when consensus stale (>30s) or peers==0.

Code touchpoints:
- `src/rpc/server.rs`
- metrics/health probe state module

Verification:
- Simulated stalled consensus marks health down.

---

## Observability / Metrics

### OBS-01 [MAJOR] Consensus metrics not updated by consensus core
Implementation steps:
1. Emit metric update commands from consensus (`UpdateConsensusRound`, `IncVotesReceived`, etc.) or inject thread-safe metrics handle.
2. Update round/height/timeouts/equivocations counters at event points.
3. Keep metrics updates non-blocking with no consensus await.

Code touchpoints:
- `src/consensus/events.rs`
- `src/consensus/state.rs`
- `src/router/mod.rs`
- `src/metrics/registry.rs`

Verification:
- Metrics endpoint reflects live round/vote/timeout/equivocation activity.

### OBS-02 [MINOR] `storage_wal_entries` never updated
Implementation steps:
1. Track active WAL entry count on write/truncate/recovery.
2. Set gauge after each mutation.

Code touchpoints:
- `src/storage/wal.rs`
- `src/router/mod.rs` or storage thread

Verification:
- Gauge increments/decrements as WAL evolves.

### OBS-03 [MINOR] `channel_drops` not incremented
Implementation steps:
1. Increment counter on every failed `try_send` to consensus or bounded channels.
2. Add channel label support if needed.

Code touchpoints:
- `src/p2p/manager.rs`
- timer and router bridge points

Verification:
- Forced full channel scenario increments drop counter.

### OBS-04 [MINOR] Missing Grafana dashboards JSON
Implementation steps:
1. Add four dashboard files in `devops/grafana/dashboards/`:
   - `consensus.json`
   - `network.json`
   - `state-machine.json`
   - `infrastructure.json`
2. Ensure provisioning path matches mounted directory.

Code touchpoints:
- `devops/grafana/dashboards/*.json`
- `devops/grafana/provisioning/dashboards/dashboard.yml`

Verification:
- Grafana auto-loads all dashboards at startup.

### OBS-05 [MINOR] Startup security checklist logs missing
Implementation steps:
1. Add DEBUG startup checks:
   - key file permissions
   - RPC bind exposure warning
   - admin key/address presence
2. Emit structured warning/error entries.

Code touchpoints:
- `src/main.rs`
- config/security helper module

Verification:
- Startup logs include checklist entries in debug mode.

---

## Event Loop & Threading

### EVT-01 [MAJOR] Router blocks Tokio thread with blocking recv
Implementation steps:
1. Run command router on dedicated OS thread or `spawn_blocking`.
2. Keep async operations bridged via bounded channels (no blocking call in async fn).
3. Ensure shutdown joins router thread cleanly.

Code touchpoints:
- `src/router/mod.rs`
- `src/main.rs`

Verification:
- Tokio tasks continue progressing when consensus channel idle.

### EVT-02 [MINOR] Consensus command channel capacity mismatch
Implementation steps:
1. Set consensus command channel cap to 256 per plan.
2. Externalize capacities into config constants.

Code touchpoints:
- `src/main.rs`
- config constants

Verification:
- Channel cap metrics/logs show expected configured values.

---

## Testing

### TST-01 [MAJOR] Missing property/fuzz tests
Implementation steps:
1. Add `proptest` suite for core invariants:
   - no double commit
   - commit requires quorum
   - serialization round-trip
   - contract host panic isolation
2. Add optional `cargo-fuzz` targets for codec/parser paths.

Code touchpoints:
- `Cargo.toml` (dev-deps)
- `tests/property/*`
- `fuzz/` targets

Verification:
- Property tests pass under deterministic seeds and CI.

### TST-02 [MAJOR] Missing Docker E2E harness
Implementation steps:
1. Add `bollard`-based harness for cluster lifecycle.
2. Implement core scenarios from plan (health, crash, partition, metrics scrape, contract flow).

Code touchpoints:
- `Cargo.toml` (dev-deps)
- `tests/harness/docker.rs`
- `tests/e2e/*`

Verification:
- Automated 4-node E2E suite runs in CI profile.

### TST-03 [MAJOR] Missing deterministic replay tests
Implementation steps:
1. Implement event recorder JSONL format.
2. Add replay runner feeding events into fresh consensus instance.
3. Assert command sequence and state transitions are identical.

Code touchpoints:
- `src/tools/event_replay.rs`
- consensus recorder hooks
- `tests/replay/*`

Verification:
- Replay same events yields identical outputs across runs.

### TST-04 [MAJOR] Missing crash-recovery test infrastructure
Implementation steps:
1. Add `CrashableWAL` test adapter with controlled crash points.
2. Implement tests for crash after prevote, after lock, during execute.

Code touchpoints:
- `tests/harness/crashable_wal.rs`
- WAL recovery tests

Verification:
- Recovery path restores safe state and resumes without safety violation.

### TST-05 [MAJOR] Missing in-process multi-node `ConsensusHarness`
Implementation steps:
1. Build in-process message bus routing proposal/votes among N consensus cores.
2. Add fault injection (delay/drop/duplicate/partition).
3. Add no-double-commit and liveness tests on 4-node setup.

Code touchpoints:
- `tests/harness/consensus.rs`
- `tests/consensus_multi_node/*`

Verification:
- Invariant checks pass over long-run heights.

### TST-06 [MINOR] Missing named plan tests
Implementation steps:
1. Add missing unit tests listed in plan-testing section 12.
2. Align names exactly for traceability.

Code touchpoints:
- `tests/consensus_core_tests.rs`
- `tests/vote_set_tests.rs`
- `tests/proposer_tests.rs`

Verification:
- Test inventory matches required names.

### TST-07 [MINOR] No coverage enforcement
Implementation steps:
1. Add tarpaulin (or llvm-cov) job in CI.
2. Enforce minimum threshold (e.g., 80%) for critical modules.

Code touchpoints:
- CI workflow files
- `Cargo.toml` dev tooling

Verification:
- CI fails when coverage drops below threshold.

---

## Failure Models

### FLR-01 [CRITICAL] Missing `halt()` function
Implementation steps:
1. Add `src/node/failure.rs` with `halt(reason) -> !` and structured error logging.
2. Replace silent `eprintln!` safety violations with `halt()` for S0 conditions.

Code touchpoints:
- new `src/node/failure.rs`
- `src/consensus/state.rs`
- `src/router/mod.rs`
- `src/main.rs`

Verification:
- S0 injected mismatch/equivocation path exits process as defined.

### FLR-02 [CRITICAL] No startup integrity check
Implementation steps:
1. Implement startup integrity routine:
   - load latest block
   - recompute block hash from canonical encoding
   - compare stored state root vs block header state root
2. Run before starting consensus.
3. On any mismatch, `halt()`.

Code touchpoints:
- `src/main.rs`
- `src/storage/block_store.rs`
- `src/storage/state_store.rs`

Verification:
- Corrupted DB fixtures are detected and halted at startup.

### FLR-03 [MAJOR] No panic hook
Implementation steps:
1. Install `std::panic::set_hook` during startup.
2. Log panic details with module/thread context.
3. Trigger controlled shutdown or process exit policy.

Code touchpoints:
- `src/main.rs`
- `src/node/failure.rs`

Verification:
- Panic in worker thread is surfaced and process exits predictably.

---

## Debugging Tools

### DBG-01 [MAJOR] Missing debug subcommands
Implementation steps:
1. Implement subcommands: `wal-inspect`, `state-dump`, `repair-db`, `replay`.
2. Add output modes (`text` and `--json`).

Code touchpoints:
- `src/main.rs` clap tree
- new `src/tools/*`

Verification:
- Each command runs against local data dir and returns expected output.

### DBG-02 [MAJOR] Event recording not implemented
Implementation steps:
1. Add env flag `RUSTBFT_RECORD_EVENTS` support.
2. Open `/data/debug/events.log` and append JSONL consensus events.
3. Guard by feature/env to avoid default runtime overhead.

Code touchpoints:
- consensus event loop
- new recorder utility module

Verification:
- With env on, event log grows per processed event; off means no file.

### DBG-03 [MINOR] Missing `collect-debug-bundle.sh`
Implementation steps:
1. Add script to gather logs, WAL inspect output, status/metrics snapshots.
2. Package into timestamped tarball.
3. Document usage in runbook.

Code touchpoints:
- `devops/scripts/collect-debug-bundle.sh`
- docs runbook

Verification:
- Script produces tarball with expected artifacts.

---

## CLI Tool

### CLI-01 [MAJOR] No `rustbft-cli` binary
Implementation steps:
1. Add separate binary target in Cargo (`[[bin]]` or `src/bin/rustbft-cli.rs`).
2. Implement tx build/sign/submit and query subcommands per plan.
3. Reuse canonical transaction serialization and crypto modules.

Code touchpoints:
- `Cargo.toml`
- `src/bin/rustbft-cli.rs`
- new `src/cli/*`

Verification:
- CLI can generate keys, submit tx, and query status/account/block.

---

## Docker & DevOps

### DKR-01 [MINOR] Missing Grafana dashboard JSON files
Implementation steps:
1. Reuse OBS-04 deliverables; ensure Docker image mounts dashboard folder.
2. Validate dashboard provisioning in compose stack.

Code touchpoints:
- `devops/grafana/dashboards/*.json`
- `devops/docker-compose.yml`

Verification:
- Grafana shows all prebuilt dashboards after `docker-compose up`.

### DKR-02 [MINOR] Docker Rust version incompatible with edition 2024
Implementation steps:
1. Update builder image from `rust:1.75-bookworm` to rust >= 1.85.
2. Pin explicit stable version used by CI.

Code touchpoints:
- `devops/Dockerfile`

Verification:
- Docker build compiles crate without edition errors.

---

## Block Sync

### BSY-01 [CRITICAL] Block sync subsystem missing
Implementation steps:
1. Add `src/sync/` module with `SyncConfig`, `SyncManager`, `SyncError`, and verification helpers.
2. Extend P2P protocol:
   - add `StatusRequest/StatusResponse`
   - add `SyncRequest/SyncResponse`
   - assign message IDs per plan.
3. Implement sync discovery:
   - query peers for latest height
   - choose best peer
4. Implement sync loop:
   - download batches
   - verify commit info signatures with validator set active at that height
   - execute each block
   - enforce `state_root == header.state_root` (halt on mismatch)
   - persist block/state and apply validator updates incrementally.
5. Integrate startup sequence:
   - after WAL recovery and before consensus thread spawn, run sync if behind.
6. Add sync metrics and logs.
7. Add `[sync]` config section + env overrides.
8. Couple with CNS-02 completion (real signatures required for commit verification).

Code touchpoints:
- new `src/sync/mod.rs`, `src/sync/verify.rs`
- `src/p2p/msg.rs`
- `src/p2p/manager.rs`
- `src/main.rs`
- `src/config/mod.rs`
- `src/types/block.rs` (commit info usage)

Verification:
- Restarted/lagging node catches up and then enters consensus at correct height.
- Invalid sync block/commit proofs are rejected and peer is abandoned.

---

## 4) Final verification checklist (before closing resolver effort)

- All 6 critical issues resolved and covered by tests.
- Consensus signatures, WAL sync-before-action, and halt paths verified.
- Startup integrity check and block-sync path validated.
- Router no longer blocks Tokio async thread.
- Mempool, RPC limits, graceful shutdown, and SIGTERM all functional.
- Debugging commands and event recording usable in devops flow.
- Grafana dashboards load automatically in Docker stack.

## 5) Suggested execution strategy for implementation PRs

1. PR-1: Safety core (CNS-01, CNS-02, STG-01, FLR-01, FLR-02)
2. PR-2: Consensus/WAL completion (CNS-03..CNS-07, STG-02, STG-03)
3. PR-3: Storage architecture (STG-04..STG-09, EVT-01)
4. PR-4: Mempool + RPC + config/ops (MMP, RPC, OPS)
5. PR-5: P2P hardening + observability (P2P, OBS, EVT-02)
6. PR-6: Testing/tooling/CLI/devops (TST, DBG, CLI, DKR)
7. PR-7: Block sync end-to-end (BSY-01)

This split keeps high-risk safety changes isolated and reviewable.
