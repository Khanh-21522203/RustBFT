# Feature: Testing Strategy

## 1. Purpose

The Testing Strategy defines the complete test suite for RustBFT: what is tested, at what level, with what tools, and in what CI pipeline stages. The goal is to prove the four fundamental properties of the system: safety (no two different blocks committed at the same height), liveness (the system makes progress), determinism (same inputs always produce same outputs), and correctness (every module behaves according to its specification). Because the consensus core is safety-critical, it receives the most thorough treatment.

## 2. Responsibilities

- Define the testing pyramid: unit, property/fuzz, integration, E2E Docker, crash recovery, deterministic replay
- Specify per-module unit test cases with function names
- Define the safety and liveness test scenarios for the BFT protocol
- Define Byzantine behavior test scenarios (equivocation, conflicting proposals, replayed messages)
- Define deterministic replay tests and cross-platform verification
- Define crash-recovery test scenarios for each WAL checkpoint
- Define the CI pipeline with time budgets per stage
- Specify test infrastructure: mock implementations, in-process multi-node harness, Docker test harness
- Define coverage targets

## 3. Non-Responsibilities

- Does not implement load testing or benchmarking (that is a performance engineering task)
- Does not define alert thresholds or operational runbooks
- Does not cover end-user documentation testing
- Does not cover security penetration testing (a separate engagement)

## 4. Architecture Design

```
Testing Pyramid

                        ┌───────────────┐
                        │  E2E Docker   │  4 nodes + Prometheus
                        │  (bollard)    │  Failure injection
                        │  ~15 min      │
                        ├───────────────┤
                        │  Integration  │  In-process multi-node
                        │  (in-memory   │  Mock storage/network
                        │   channels)   │  ~5 min
                        ├───────────────┤
                        │  Crash /      │  Panic hooks + WAL replay
                        │  Recovery     │  ~3 min
                        ├───────────────┤
                        │  Property /   │  proptest / arbitrary
                        │  Fuzz         │  ~5 min
                    ┌───┴───────────────┴───┐
                    │     Unit Tests        │  Per module, mocks
                    │     ~2 min            │
                    └───────────────────────┘

Test Infrastructure:
    MockStorage     → in-memory StorageEngine trait impl
    MockMempool     → in-memory MempoolCommand handler
    MockNetwork     → in-memory channel-based message routing
    ConsensusHarness → spawn N consensus threads with mock deps
    DockerHarness   → bollard-based container lifecycle manager
```

## 5. Core Data Structures (Rust)

```rust
// tests/harness/consensus.rs

pub struct ConsensusHarness {
    nodes: Vec<HarnessNode>,
    message_bus: InMemoryMessageBus,
}

pub struct HarnessNode {
    pub id: ValidatorId,
    pub inbound_tx: crossbeam::channel::Sender<ConsensusEvent>,
    pub outbound_rx: crossbeam::channel::Receiver<ConsensusCommand>,
    pub thread: std::thread::JoinHandle<()>,
    pub committed: Arc<Mutex<BTreeMap<u64, Hash>>>,  // height → committed block hash
}

pub struct InMemoryMessageBus {
    // Routes BroadcastProposal and BroadcastVote to all other nodes
    // Can inject: delays, drops, duplicates, reorders
    pub drop_rate: f64,
    pub delay_ms: u64,
}

// tests/harness/docker.rs

pub struct DockerHarness {
    docker: bollard::Docker,
    container_ids: Vec<String>,
    node_urls: Vec<String>,   // "http://127.0.0.1:26657" etc.
}

impl DockerHarness {
    pub async fn start_cluster(n: usize) -> Self;
    pub async fn kill_node(&self, index: usize);
    pub async fn stop_node(&self, index: usize);
    pub async fn start_node(&self, index: usize);
    pub async fn partition(&self, group_a: &[usize], group_b: &[usize]);
    pub async fn heal_partition(&self);
    pub async fn wait_for_height(&self, node: usize, height: u64, timeout: Duration);
    pub async fn submit_tx(&self, node: usize, tx: &SignedTransaction) -> Receipt;
}
```

## 6. Public Interfaces

```rust
// Test trait for mock storage engine
pub struct MemoryStorageEngine {
    data: BTreeMap<String, BTreeMap<Vec<u8>, Vec<u8>>>,
}

impl StorageEngine for MemoryStorageEngine {
    fn get(&self, cf: &str, key: &[u8]) -> Result<Option<Vec<u8>>, StorageError> { ... }
    fn write_batch(&self, ops: Vec<BatchOp>) -> Result<(), StorageError> { ... }
    // ...
}

// WAL crash simulation
pub struct CrashableWAL {
    inner: WAL,
    crash_after: Option<usize>,   // panic after N writes
    write_count: AtomicUsize,
}

// Deterministic replay recorder / player
pub struct EventRecorder {
    events: Vec<(u64 /* timestamp_ns */, ValidatorId, ConsensusEvent)>,
}

impl EventRecorder {
    pub fn record(&mut self, node_id: ValidatorId, event: ConsensusEvent);
    pub fn replay_into(&self, node_id: ValidatorId, state: ConsensusState)
        -> Vec<ConsensusCommand>;
}
```

## 7. Internal Algorithms

### In-Process Consensus Harness
```
fn run_integration_test(n_nodes, scenario):
    // 1. Build validator set
    validator_set = ValidatorSet::new(n_nodes validators, equal power)

    // 2. Spawn consensus threads
    harness = ConsensusHarness::new(n_nodes)
    for i in 0..n_nodes:
        harness.spawn_node(i, validator_set.clone())

    // 3. Connect message bus
    // BroadcastProposal/Vote commands are intercepted and routed to all peers
    // via the InMemoryMessageBus (with optional drop/delay)

    // 4. Run scenario (inject events: proposals, votes, timeouts)
    scenario(&mut harness)

    // 5. Assert invariants
    assert_no_double_commit(&harness)    // all committed heights have same hash
    assert_monotonic_heights(&harness)   // height only increases
```

### Crash Recovery Test Template
```
fn test_crash_at_point(crash_point: CrashPoint):
    // 1. Run consensus until crash_point is about to be hit
    // 2. Install crash hook: panic on next WAL write of type X
    // 3. Catch panic (std::panic::catch_unwind)
    // 4. Reconstruct ConsensusState via WAL recovery
    // 5. Resume consensus
    // 6. Assert: no safety violation (no conflicting commits)
    // 7. Assert: no duplicate vote sent (WAL restored lock state)
```

### Byzantine Simulation
```
fn byzantine_equivocation_test():
    // Node 3 sends different prevotes to different peers
    harness.set_byzantine(3, ByzantineStrategy::SplitVote {
        vote_a: prevote_for_block_a,
        send_to: [0, 1],
        vote_b: prevote_for_block_b,
        send_to: [2],
    })
    harness.run(50 heights)
    assert_no_double_commit(&harness)
    assert harness.evidence_recorded() > 0
```

### Deterministic Replay
```
fn test_deterministic_replay():
    // 1. Run 4-node cluster for 50 heights, record all events per node
    recorder = EventRecorder::new()
    harness = ConsensusHarness::with_recorder(recorder)
    harness.run(50 heights)

    // 2. For each node, replay events into fresh state
    for node_id in 0..4:
        fresh_state = ConsensusState::new_at_height(1, validator_set)
        commands_recorded = harness.recorded_commands(node_id)
        commands_replayed = recorder.replay_into(node_id, fresh_state)
        assert_eq!(commands_recorded, commands_replayed,
            "determinism violated for node {node_id}")
```

## 8. Persistence Model

Test databases use the `MemoryStorageEngine` (in-memory `BTreeMap`). E2E Docker tests use the actual RocksDB storage inside containers, with volumes reset between test runs via `docker-compose down -v`.

## 9. Concurrency Model

Unit and integration tests are synchronous (no Tokio runtime). Each test function spawns its own threads and channels and tears them down on exit. This keeps tests isolated and avoids global state. E2E Docker tests use a single `tokio::runtime::Runtime` per test binary for the `bollard` Docker API client.

## 10. Configuration

```toml
# .cargo/config.toml

[test]
# Cargo test configuration

# Run tests with full output on failure
RUST_BACKTRACE = "1"

# Integration tests require more stack space for nested call depth tests
[target.'cfg(target_os = "linux")']
rustflags = ["-C", "default-linker-libraries=yes"]
```

```
# CI environment variables
RUST_LOG = "rustbft=debug"
RUSTBFT_TEST_TIMEOUT_MS = "60000"
RUSTBFT_E2E_CLUSTER_SIZE = "4"
```

## 11. Observability

- CI pipeline reports test results and coverage via standard JUnit XML output
- `cargo-tarpaulin` measures line coverage; CI enforces > 80% overall
- Failed E2E tests collect container logs automatically via `DockerHarness::collect_logs_on_failure()`
- Integration test failures print the full event sequence that led to the failure for reproduction

## 12. Testing Strategy

### Unit Tests (per module)

**Foundation & Types**:
- **`test_serialize_round_trip_all_types`**: encode/decode all message types → byte-for-byte identical
- **`test_sha256_known_vector`**: sha256("abc") → known hash constant
- **`test_ed25519_sign_verify`**: sign a message, verify with public key → valid
- **`test_ed25519_wrong_key_fails`**: verify with wrong public key → invalid
- **`test_genesis_parse_valid`**: parse valid genesis JSON → all fields populated

**Consensus Core**:
- **`test_happy_path_4_validators`**: inject Proposal + 4 Prevotes + 4 Precommits → ExecuteBlock emitted, step=Commit
- **`test_proposer_timeout`**: no Proposal, TimeoutPropose fires → BroadcastVote(Prevote(nil))
- **`test_locking_refuses_conflicting_block`**: polka for A in round 0, lock; round 1 proposes B → prevote(nil) for B
- **`test_unlock_on_nil_polka`**: locked on A, nil polka arrives → WriteWAL(Unlocked), B accepted
- **`test_equivocation_detection`**: two different prevotes from same validator → RecordEvidence, vote counted once
- **`test_round_skip`**: > 1/3 power from round 5 → EnterRound(height, 5)
- **`test_commit_from_propose_step`**: > 2/3 precommits while in Propose → commit proceeds
- **`test_duplicate_vote_not_double_counted`**: same vote twice → quorum not spuriously reached
- **`test_stale_event_dropped`**: event for old height → state unchanged
- **`test_proposer_selection_deterministic`**: same (height, round, ValidatorSet) → same proposer
- **`test_quorum_integer_math`**: (267*2/3)+1 = 179; assert no float used

**VoteSet**:
- **`test_has_polka_returns_true_at_threshold`**: add votes summing to exactly quorum → has_polka true
- **`test_has_polka_returns_false_below_threshold`**: one vote short → has_polka false

**Mempool**:
- **`test_add_valid_tx`**: add tx → stats shows count=1
- **`test_duplicate_rejected`**: same tx twice → Err(Duplicate)
- **`test_invalid_signature_rejected`**: wrong sig → Err(InvalidSignature)
- **`test_max_tx_count_enforced`**: fill to max_txs → next returns Err(MempoolFull)
- **`test_reap_respects_max_bytes`**: reap 1000 bytes → total ≤ 1000
- **`test_evict_removes_txs`**: add 5, evict 3 → stats shows count=2

**State Machine**:
- **`test_transfer_success`**: valid transfer → balances updated, nonce incremented
- **`test_transfer_insufficient_balance`**: balance < cost → receipt success=false, gas charged
- **`test_failed_tx_still_in_block`**: failed tx → included, gas charged, state reverted
- **`test_state_root_deterministic`**: same block twice → same root
- **`test_snapshot_revert`**: modify state, revert → state equals pre-modification snapshot
- **`test_validator_update_quorum_violated`**: update removes > 1/3 power → batch rejected

**Smart Contracts**:
- **`test_validate_rejects_floats`**: module with f32.add → Err(FloatInstruction)
- **`test_validate_rejects_simd`**: module with i8x16.add → Err(SimdInstruction)
- **`test_validate_rejects_unknown_import`**: env.malloc import → Err(UnknownImport)
- **`test_deploy_and_call_counter`**: deploy counter WASM, increment, get_count → 1
- **`test_gas_exhaustion`**: call with too-low gas → OutOfFuel, state reverted, full gas charged
- **`test_cross_contract_call_callee_fails`**: A calls B, B aborts → B reverted, A continues
- **`test_call_depth_limit`**: 65 nested calls → 65th returns error
- **`test_deterministic_execution`**: same call twice → same output, same gas_used

**Storage**:
- **`test_wal_write_and_recover`**: write entries, simulate crash, recover → all entries present
- **`test_wal_truncated_entry_discarded`**: partial write → truncated entry ignored
- **`test_persist_block_atomic`**: write block + receipts → read back all fields
- **`test_state_root_deterministic_trie`**: same changeset → same root
- **`test_corruption_detection_halts`**: tamper block data → HALT triggered

**P2P**:
- **`test_message_framing_round_trip`**: encode message → decode → same bytes
- **`test_handshake_valid`**: two nodes connect, handshake → both Connected
- **`test_invalid_signature_penalizes_peer`**: corrupted Proposal → peer score -50
- **`test_gossip_deduplication`**: same vote twice → forwarded once to consensus

**Validator Sets**:
- **`test_add_validator_success`**, **`test_remove_last_validator_rejected`**, **`test_power_change_exceeds_limit`**, **`test_quorum_threshold_integer_math`**

### Safety Tests (in-process multi-node)

- **`test_no_double_commit_4_honest`**: 4 validators, 100 heights → exactly one committed block per height, all nodes agree
- **`test_equivocation_does_not_break_safety`**: node 3 sends conflicting prevotes → no double commit, evidence recorded
- **`test_partition_no_commits`**: {0,1} vs {2,3} → neither side commits
- **`test_partition_heals_and_agrees`**: after healing, all nodes commit same blocks
- **`test_locking_prevents_conflicting_commit`**: polka then only 2/4 precommits, next round different proposer → locked nodes refuse

### Liveness Tests

- **`test_4_of_4_commits_100_blocks`**: all online → 100 blocks committed
- **`test_3_of_4_continues_after_crash`**: crash node 3 → remaining 3 continue committing
- **`test_restart_catches_up`**: crash and restart node 2 → catches up to current height
- **`test_round_advances_on_timeout`**: offline proposer → timeout fires, round 1 proposer commits

### Byzantine Tests

- **`test_byzantine_proposer_split_block`**: proposer sends different blocks to different peers → no commit, next round succeeds
- **`test_byzantine_voter_split_vote`**: node sends conflicting prevotes → safety preserved, evidence recorded
- **`test_replay_attack_no_double_count`**: replay same vote 100 times → counted once
- **`test_future_height_message_ignored`**: message for height+100 → state unchanged

### Crash Recovery Tests

- **`test_crash_after_prevote_before_precommit`**: WAL restores vote; no re-vote
- **`test_crash_after_lock`**: WAL restores locked_block; node precommits correctly
- **`test_crash_during_block_execution`**: CommitStarted in WAL; re-execute on restart
- **`test_crash_after_commit_completed`**: WAL truncated; clean startup

### Deterministic Replay Tests

- **`test_replay_same_output`**: recorded events replayed into fresh state → identical commands
- **`test_replay_cross_architecture`**: events recorded on x86, replayed on arm64 → identical outputs

### E2E Docker Tests

- **`test_e2e_smoke_cluster_healthy`**, **`test_e2e_commit_100_blocks`**, **`test_e2e_transfer_committed`**, **`test_e2e_node_crash_recovers`**, **`test_e2e_network_partition_and_heal`**, **`test_e2e_contract_deploy_and_call`**, **`test_e2e_validator_add`**, **`test_e2e_metrics_scraped`**, **`test_e2e_crash_recovery_wal`**

### Property Tests

- **`prop_no_double_commit`**: for any sequence of valid ConsensusEvents → never two CommitBlock for same height
- **`prop_commit_requires_quorum`**: every CommitBlock is preceded by > 2/3 precommits
- **`prop_serialize_round_trip`**: for any valid message → decode(encode(msg)) == msg
- **`prop_contract_no_host_panic`**: for any WASM bytes and input → no host process panic

## 13. Open Questions

- **Fuzzing infrastructure**: The contract execution fuzzer (`execute_contract(arbitrary_wasm, arbitrary_input)`) benefits from a persistent fuzzing corpus (e.g., `cargo-fuzz` with AFL or libFuzzer). Deferred to security hardening phase.
- **Cross-architecture determinism tests**: Testing on `linux/aarch64` requires either a second CI runner type (e.g., GitHub Actions `ubuntu-22.04-arm`) or QEMU emulation. Deferred post-MVP.
