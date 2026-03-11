# RustBFT — Testing Strategy

**Purpose:** Define the complete testing strategy: unit tests, integration tests, byzantine simulation, deterministic replay, and CI pipeline.
**Audience:** Engineers writing tests and maintaining the CI/CD pipeline.

---

## 1. Testing Pyramid

```
                    ┌───────────┐
                    │  E2E /    │   4-node Docker cluster
                    │  Docker   │   Failure injection
                    │           │   ~minutes per run
                    ├───────────┤
                    │Integration│   Multi-node in-process
                    │           │   Channel-connected cores
                    │           │   ~seconds per run
                    ├───────────┤
                    │  Property │   Randomized inputs
                    │  / Fuzz   │   Invariant checking
                    │           │   ~seconds per run
                ┌───┴───────────┴───┐
                │    Unit Tests     │   Per-function, per-module
                │                   │   Mock dependencies
                │                   │   ~milliseconds per run
                └───────────────────┘
```

---

## 2. Unit Tests

### 2.1 Scope

Every module has unit tests covering its public API and critical internal logic.

### 2.2 Per-Module Test Focus

| Module | Test Focus |
|--------|-----------|
| `types` | Serialization round-trip, canonical encoding, hash determinism |
| `crypto` | Signature generation/verification, key derivation, hash correctness |
| `consensus` | State transitions, vote counting, locking/unlocking, proposer selection, timeout handling |
| `mempool` | Add/remove/reap transactions, size limits, duplicate rejection, eviction |
| `state` | Block execution pipeline, gas metering, snapshot/revert, state root computation |
| `contracts` | WASM validation, host API correctness, gas exhaustion, cross-contract calls |
| `storage` | Block store CRUD, state store versioning, WAL write/read/recovery |
| `p2p` | Message framing, handshake, peer scoring, backpressure |
| `rpc` | Request parsing, response formatting, error codes, rate limiting |
| `metrics` | Metric registration, counter/gauge/histogram correctness |

### 2.3 Consensus Core Unit Tests (Critical)

These tests exercise the consensus FSM in isolation, without networking or async:

```
Test structure:
    1. Construct ConsensusState with a known validator set
    2. Create crossbeam channels (inbound, outbound)
    3. Send a sequence of ConsensusEvents
    4. Assert the sequence of ConsensusCommands emitted
    5. Assert the final ConsensusState

Example test cases:
    - Happy path: proposal → prevotes → precommits → commit
    - Proposer timeout: no proposal → prevote nil → precommit nil → next round
    - Locking: prevote block A, lock, next round, refuse to prevote block B
    - Unlocking: locked on A, see polka for nil, unlock, prevote B in next round
    - Equivocation detection: two different votes from same validator
    - Round skip: receive votes from round R+2, skip to R+2
    - Commit from behind: in Propose step, receive enough precommits to commit
    - Invalid proposal: bad signature → prevote nil
    - Duplicate vote: same vote received twice → no double-counting
```

### 2.4 Mock Traits

Each layer depends on traits, not concrete implementations:

```
trait BlockStore:       fn get_block(h) → Block, fn store_block(b) → ()
trait StateStore:       fn get_account(addr) → Account, fn apply_changeset(cs) → Hash
trait ContractEngine:   fn execute(module, input, gas) → Result
trait Mempool:          fn reap(max) → Vec<Tx>, fn add(tx) → Result
trait Network:          fn broadcast(msg) → (), fn send(peer, msg) → ()
```

Unit tests inject mock implementations that record calls and return predetermined values.

---

## 3. Safety Tests

Safety tests verify the core invariant: **no two different blocks are committed at the same height**.

### 3.1 Double-Commit Test

```
Test: No double commit under honest behavior
    Setup: 4 validators, all honest
    Action: Run 100 heights
    Assert: For each height, exactly one block is committed
    Assert: All nodes agree on the committed block hash
```

### 3.2 Equivocation Safety Test

```
Test: Safety holds even if 1 of 4 validators equivocates
    Setup: 4 validators, validator 3 sends conflicting prevotes
    Action: Run 50 heights
    Assert: No two different blocks committed at any height
    Assert: Equivocation evidence is recorded
```

### 3.3 Partition Safety Test

```
Test: Safety holds during network partition
    Setup: 4 validators, partition into {0,1} and {2,3}
    Action: Run for 30 seconds
    Assert: Neither partition commits (neither has >2/3)
    Action: Heal partition
    Assert: All nodes converge and commit
    Assert: No conflicting commits occurred
```

### 3.4 Lock Safety Test

```
Test: Locking prevents conflicting commits across rounds
    Setup: 4 validators
    Round 0: Validators 0,1,2 prevote block A (polka). Validator 3 is slow.
    Round 0: Only validators 0,1 precommit A (no commit, <2/3).
    Round 1: Validator 3 proposes block B.
    Assert: Validators 0,1 (locked on A) refuse to prevote B.
    Assert: Block B cannot be committed.
```

---

## 4. Liveness Tests

Liveness tests verify that the system makes progress under expected conditions.

### 4.1 Basic Liveness

```
Test: System commits blocks when all validators are online
    Setup: 4 validators, all connected
    Action: Submit 100 transactions
    Assert: All transactions are committed within 60 seconds
    Assert: Height advances monotonically
```

### 4.2 Liveness After Crash

```
Test: System recovers liveness after one validator crashes and restarts
    Setup: 4 validators
    Action: Crash validator 2 at height 10
    Assert: System continues committing (3/4 > 2/3)
    Action: Restart validator 2 at height 20
    Assert: Validator 2 catches up and participates
```

### 4.3 Liveness After Partition

```
Test: System recovers liveness after partition heals
    Setup: 4 validators
    Action: Partition {0,1} from {2,3} for 30 seconds
    Assert: No commits during partition (neither side has >2/3)
    Action: Heal partition
    Assert: Commits resume within 10 seconds
```

### 4.4 Round Advancement

```
Test: System advances rounds when proposer is offline
    Setup: 4 validators, validator 0 (proposer for round 0) is offline
    Assert: Timeout fires, nodes advance to round 1
    Assert: New proposer (validator 1) proposes and block is committed
```

---

## 5. Byzantine Behavior Tests

### 5.1 Byzantine Proposer

```
Test: Byzantine proposer sends different blocks to different validators
    Setup: 4 validators, validator 0 is byzantine proposer
    Action: Validator 0 sends block A to {1} and block B to {2,3}
    Assert: No commit occurs (no block gets >2/3 prevotes)
    Assert: System advances to next round with honest proposer
    Assert: Equivocation evidence is generated
```

### 5.2 Byzantine Voter

```
Test: Byzantine validator sends conflicting votes
    Setup: 4 validators, validator 3 sends prevote(A) to {0,1} and prevote(B) to {2}
    Assert: No safety violation
    Assert: Equivocation evidence is detected and recorded
```

### 5.3 Message Replay Attack

```
Test: Replayed messages do not cause double-counting
    Setup: 4 validators
    Action: Replay validator 1's prevote 100 times
    Assert: Vote is counted exactly once
    Assert: No state corruption
```

### 5.4 Future Message Attack

```
Test: Messages from future heights/rounds are handled safely
    Setup: 4 validators at height 5
    Action: Inject votes claiming height 100
    Assert: Messages are buffered or dropped, not processed
    Assert: No state corruption
```

---

## 6. Deterministic Replay Tests

### 6.1 Event Recording

During test runs, all `ConsensusEvent`s received by each node are recorded to a file:

```
replay_log format:
    [timestamp_ns] [node_id] [event_type] [serialized_event]
```

### 6.2 Replay Verification

```
Test: Deterministic replay produces identical output
    1. Run a 4-node cluster for 50 heights, recording all events
    2. For each node:
       a. Replay the recorded events into a fresh ConsensusState
       b. Assert the same ConsensusCommands are emitted
       c. Assert the same final state (height, locked_block, etc.)
    3. This proves: same inputs → same outputs (determinism)
```

### 6.3 Cross-Platform Replay

```
Test: Replay produces identical results on different platforms
    1. Record events on Linux x86_64
    2. Replay on Linux aarch64 (or in CI with different runners)
    3. Assert identical outputs
    4. This catches platform-dependent behavior (endianness, float, etc.)
```

---

## 7. Integration Tests

### 7.1 In-Process Multi-Node

```
Test setup:
    1. Spawn 4 consensus core threads in the same process
    2. Connect them via in-memory channels (no TCP)
    3. Use mock mempool, mock storage
    4. Inject transactions and timeouts programmatically

Advantages:
    - Fast (no network overhead)
    - Deterministic (controlled event delivery)
    - Debuggable (single process, single debugger)
```

### 7.2 Test Scenarios

| Scenario | Description |
|----------|-------------|
| Happy path (4/4) | All nodes online, commit 100 blocks |
| One crash (3/4) | Kill one node, verify continued progress |
| Crash and restart | Kill and restart a node, verify catch-up |
| Slow node | One node processes events with artificial delay |
| Message loss | Randomly drop 10% of messages |
| Message reorder | Deliver messages in random order |
| Validator set change | Add a 5th validator at height 10, verify it participates at height 11 |
| Contract execution | Deploy and call a contract, verify state across all nodes |

---

## 8. End-to-End Docker Tests

### 8.1 Setup

```
docker-compose up -d   # 4 validators + Prometheus + Grafana

Test runner (outside Docker):
    1. Wait for all nodes to be healthy (poll /health)
    2. Submit transactions via RPC
    3. Query state via RPC
    4. Inject failures via Docker commands (pause, stop, network disconnect)
    5. Assert expected behavior
```

### 8.2 E2E Test Scenarios

| Scenario | Steps | Assertions |
|----------|-------|------------|
| Smoke test | Start cluster, submit 10 txs | All txs committed, all nodes at same height |
| Node failure | Stop node 3, submit txs | Txs committed by remaining 3 nodes |
| Node recovery | Restart node 3 | Node 3 catches up to current height |
| Network partition | `docker network disconnect` node 2,3 | No commits during partition; commits resume after reconnect |
| Contract deploy | Deploy WASM contract via RPC | Contract address returned, state queryable |
| Contract call | Call deployed contract | Return value matches expected output |
| Validator add | Submit ValidatorUpdate tx | New validator participates at next height |
| Validator remove | Submit ValidatorUpdate tx | Removed validator's votes are ignored |
| Crash recovery | `docker kill` node 1, restart | Node 1 recovers from WAL, catches up |
| Metrics | Query Prometheus | Metrics for height, round, peer count are present and correct |

### 8.3 Test Framework

E2E tests use a Rust test harness that:
- Manages Docker containers via `bollard` crate (Docker API client).
- Submits transactions via HTTP client (`reqwest`).
- Asserts conditions with retries and timeouts.
- Collects logs on failure for debugging.

---

## 9. Crash and Restart Tests

### 9.1 Crash Points

Test crashes at each critical point in the consensus lifecycle:

| Crash Point | Expected Recovery |
|-------------|-------------------|
| After sending prevote, before precommit | WAL replays vote; node does not re-vote |
| After locking, before precommit | WAL restores lock; node precommits locked block |
| After precommit, before commit | WAL replays; node re-enters precommit phase |
| During block execution | WAL has CommitStarted; re-execute block on restart |
| During state persistence | WAL has CommitStarted; re-persist on restart |
| After commit, before WAL truncation | WAL has CommitCompleted; truncate on restart |

### 9.2 Implementation

```
Crash simulation:
    1. Run consensus core with a "crash hook" that panics at a specified point
    2. Catch the panic (in test harness)
    3. Reconstruct ConsensusState from WAL + storage
    4. Resume consensus
    5. Assert: no safety violations, no duplicate votes, correct state
```

---

## 10. Property-Based / Fuzz Testing

### 10.1 Consensus Properties

Using `proptest` or `quickcheck`:

```
Property: For any sequence of valid ConsensusEvents,
          the consensus core never emits two CommitBlock commands
          for different blocks at the same height.

Property: For any sequence of valid ConsensusEvents,
          if a CommitBlock is emitted, it was preceded by
          >2/3 precommits for that block.

Property: For any validator set and any (height, round),
          proposer_selection returns exactly one validator.

Property: For any two identical event sequences,
          the consensus core produces identical command sequences.
```

### 10.2 Serialization Fuzzing

```
Fuzz target: deserialize(arbitrary_bytes)
    Assert: either succeeds and round-trips correctly,
            or returns a well-formed error.
    Assert: no panics, no undefined behavior.
```

### 10.3 Contract Execution Fuzzing

```
Fuzz target: execute_contract(arbitrary_wasm, arbitrary_input)
    Assert: either succeeds or returns a well-formed error.
    Assert: no host crashes, no state corruption.
    Assert: gas is always charged.
```

---

## 11. CI Pipeline

```
┌─────────────────────────────────────────────────────────┐
│                     CI Pipeline                          │
│                                                          │
│  Stage 1: Lint & Format (< 1 min)                       │
│    cargo fmt --check                                     │
│    cargo clippy -- -D warnings                           │
│    cargo deny check                                      │
│                                                          │
│  Stage 2: Unit Tests (< 3 min)                          │
│    cargo test                                            │
│                                                          │
│  Stage 3: Property Tests (< 5 min)                      │
│    cargo test --features proptest                        │
│                                                          │
│  Stage 4: Integration Tests (< 5 min)                   │
│    cargo test --test integration                         │
│                                                          │
│  Stage 5: E2E Docker Tests (< 15 min)                   │
│    docker-compose up -d                                  │
│    cargo test --test e2e                                 │
│    docker-compose down                                   │
│                                                          │
│  Stage 6: Deterministic Replay (< 5 min)                │
│    cargo test --test replay                              │
│                                                          │
│  Total: < 35 minutes                                     │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

---

## 12. Test Coverage Targets

| Area | Target |
|------|--------|
| Consensus state transitions | 100% of FSM transitions |
| Consensus safety invariants | 100% (no exceptions) |
| Serialization round-trips | 100% of message types |
| Host API functions | 100% |
| Error paths | > 90% |
| Overall line coverage | > 80% |

---

## Definition of Done — Testing Strategy

- [x] Testing pyramid defined with all levels
- [x] Per-module unit test focus areas listed
- [x] Consensus core unit test cases enumerated
- [x] Safety tests (double-commit, equivocation, partition, locking)
- [x] Liveness tests (basic, crash recovery, partition recovery, round advancement)
- [x] Byzantine behavior tests (proposer, voter, replay, future messages)
- [x] Deterministic replay tests with cross-platform verification
- [x] Integration tests (in-process multi-node)
- [x] E2E Docker tests with scenarios
- [x] Crash and restart tests at each critical point
- [x] Property-based / fuzz testing
- [x] CI pipeline stages with time budgets
- [x] Coverage targets
- [x] No Rust source code included
