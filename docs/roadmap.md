# RustBFT — Roadmap

**Purpose:** Define the phased implementation plan for the RustBFT MVP and outline future work beyond MVP scope.
**Audience:** Engineers planning implementation work and stakeholders tracking progress.

---

## 1. Guiding Principles

- **Safety before liveness, liveness before performance.**
- **Each phase produces a testable, runnable artifact.**
- **No phase may violate the core invariants:** deterministic finality, hybrid threading, layered separation.
- **Documentation is updated alongside implementation, not after.**

---

## 2. Phase Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                        ROADMAP PHASES                            │
│                                                                  │
│  Phase 0    Phase 1     Phase 2      Phase 3      Phase 4       │
│  ────────   ────────    ────────     ────────     ────────      │
│  Foundation Consensus   Execution    Contracts    Production    │
│  & Types    Core        & Storage    & Validator  Hardening     │
│                                      Dynamics                   │
│                                                                  │
│  ◀──── 2 weeks ────▶◀── 3 weeks ──▶◀── 3 weeks ─▶◀── 2 weeks ▶│
│                                                                  │
│  Estimated total: 10–12 weeks                                   │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## 3. Phase 0 — Foundation & Types (Weeks 1–2)

### Goal

Establish the project, define all shared types, implement cryptographic primitives, and set up the build/test infrastructure.

### Deliverables

| # | Deliverable | Module | Description |
|---|-------------|--------|-------------|
| 0.1 | Project setup | `Cargo.toml` | Single crate with module stubs |
| 0.2 | Shared types | `src/types/` | Block, Transaction, Vote, Proposal, ValidatorSet, Address, Hash |
| 0.3 | Canonical serialization | `src/types/` | Deterministic binary encoding for all types |
| 0.4 | Cryptographic primitives | `src/crypto/` | Ed25519 sign/verify, SHA-256, key generation |
| 0.5 | Genesis parsing | `src/types/` | Load and validate genesis.json |
| 0.6 | CI pipeline | `.github/workflows` | Lint, format, test, clippy |
| 0.7 | Documentation sync | `docs/` | Update docs with any design changes |

### Exit Criteria

- [ ] All types compile and have serialization round-trip tests
- [ ] Canonical serialization produces identical bytes across platforms
- [ ] Ed25519 sign/verify works with test vectors
- [ ] Genesis file can be parsed and validated
- [ ] CI pipeline runs on every PR
- [ ] `#![deny(clippy::disallowed_types)]` configured for `HashMap`/`HashSet` in the crate

---

## 4. Phase 1 — Consensus Core (Weeks 3–5)

### Goal

Implement the BFT consensus state machine as a single-threaded, synchronous event processor. This is the most critical component.

### Deliverables

| # | Deliverable | Module | Description |
|---|-------------|--------|-------------|
| 1.1 | Consensus state machine | `src/consensus/` | Full FSM: Propose → Prevote → Precommit → Commit |
| 1.2 | Vote aggregation | `src/consensus/` | VoteSet with BTreeMap, quorum calculation (integer math) |
| 1.3 | Locking mechanism | `src/consensus/` | Lock/unlock logic with safety invariants |
| 1.4 | Proposer selection | `src/consensus/` | Weighted round-robin, deterministic |
| 1.5 | Event loop | `src/consensus/` | Blocking recv() loop, batch processing |
| 1.6 | Timer integration | `src/consensus/` | ScheduleTimeout / CancelTimeout commands |
| 1.7 | Equivocation detection | `src/consensus/` | Detect and record conflicting votes |
| 1.8 | Unit tests | `src/consensus/` | All FSM transitions, safety tests, locking tests |
| 1.9 | In-process multi-node test | `tests/integration` | 4 consensus cores connected via channels |

### Exit Criteria

- [ ] Consensus module contains no `tokio` imports or `.await`
- [ ] No `.await` in `src/consensus/`
- [ ] All state transitions from `state-transition-table.md` are implemented and tested
- [ ] Safety test: no double-commit across 1000 heights with 4 nodes
- [ ] Liveness test: commits proceed with 3/4 nodes
- [ ] Locking test: locked node refuses to prevote conflicting block
- [ ] Deterministic replay test: same events → same commands

---

## 5. Phase 2 — Execution & Storage (Weeks 6–8)

### Goal

Implement block execution (state machine), persistent storage, WAL, and the P2P networking layer. Wire everything together into a running node.

### Deliverables

| # | Deliverable | Module | Description |
|---|-------------|--------|-------------|
| 2.1 | State machine | `src/state/` | BeginBlock → DeliverTx → EndBlock → Commit pipeline |
| 2.2 | Account model | `src/state/` | Accounts with balance, nonce |
| 2.3 | Transfer execution | `src/state/` | Simple value transfers with gas |
| 2.4 | Merkle state tree | `src/state/` | Sparse Merkle trie for state root computation |
| 2.5 | Block store | `src/storage/` | RocksDB-backed block persistence |
| 2.6 | State store | `src/storage/` | RocksDB-backed state persistence with versioning |
| 2.7 | WAL | `src/storage/` | File-based WAL with CRC32, fsync, recovery |
| 2.8 | P2P networking | `src/p2p/` | TCP connections, handshake, message framing, gossip |
| 2.9 | Mempool | `src/mempool/` | Transaction pool with validation, reap, eviction |
| 2.10 | Node binary | `src/main.rs` | Wire all modules, startup/shutdown sequence |
| 2.11 | RPC server | `src/rpc/` | Basic endpoints: broadcast_tx, get_block, status, health |
| 2.12 | Metrics | `src/metrics/` | Prometheus exporter with core metrics |
| 2.13 | Docker setup | `devops/` | Dockerfile, docker-compose, 4-node cluster |

### Exit Criteria

- [ ] 4-node Docker cluster starts and commits blocks
- [ ] Transactions submitted via RPC are included in blocks
- [ ] All nodes agree on state root at every height
- [ ] Node survives crash and restart (WAL recovery)
- [ ] Prometheus metrics are exported and scrapeable
- [ ] Health check endpoint returns correct status
- [ ] Block queries via RPC return correct data

---

## 6. Phase 3 — Contracts & Validator Dynamics (Weeks 9–11)

### Goal

Add WASM smart contract execution and dynamic validator set management.

### Deliverables

| # | Deliverable | Module | Description |
|---|-------------|--------|-------------|
| 3.1 | WASM runtime | `src/contracts/` | wasmtime integration with fuel metering |
| 3.2 | Module validation | `src/contracts/` | Deploy-time checks (no floats, no SIMD, approved imports) |
| 3.3 | Host API | `src/contracts/` | Storage read/write, context, events, crypto |
| 3.4 | Contract deploy | `src/state/` | ContractDeploy transaction type |
| 3.5 | Contract call | `src/state/` | ContractCall transaction type with gas metering |
| 3.6 | Cross-contract calls | `src/contracts/` | Nested calls with snapshot/revert |
| 3.7 | Contract queries | `src/rpc/` | Read-only contract execution via RPC |
| 3.8 | Validator updates | `src/state/` | ValidatorUpdate transaction processing in EndBlock |
| 3.9 | Safety validation | `src/state/` | Quorum preservation, max power change per height |
| 3.10 | Validator set transition | `src/consensus/` | Load new validator set at height boundary |
| 3.11 | Example contracts | `examples/` | Counter contract, token contract |
| 3.12 | CLI tool | `src/cli.rs` | Transaction construction, signing, submission |

### Exit Criteria

- [ ] Contract deploy and call work end-to-end via RPC
- [ ] Gas metering halts execution at limit
- [ ] Contract failure reverts state without corrupting global state
- [ ] Float instructions in WASM are rejected at deploy time
- [ ] Cross-contract calls work with proper snapshot/revert
- [ ] Validator add/remove/update-power works via transaction
- [ ] Validator set change takes effect at next height
- [ ] Quorum safety check rejects dangerous updates
- [ ] All nodes agree on state root after contract execution

---

## 7. Phase 4 — Production Hardening (Weeks 12–13)

### Goal

Harden the system for reliability: comprehensive testing, failure simulation, observability polish, and operational tooling.

### Deliverables

| # | Deliverable | Description |
|---|-------------|-------------|
| 4.1 | E2E test suite | Docker-based tests for all scenarios in testing-strategy.md |
| 4.2 | Byzantine tests | Simulated equivocation, invalid proposals, selective delivery |
| 4.3 | Crash recovery tests | Crash at every critical point, verify WAL recovery |
| 4.4 | Network partition tests | Docker network disconnect/reconnect |
| 4.5 | Deterministic replay | Record and replay event sequences, verify identical output |
| 4.6 | Property-based tests | proptest for consensus invariants and serialization |
| 4.7 | Grafana dashboards | Pre-built dashboards for consensus, network, state, infra |
| 4.8 | Alerting rules | Prometheus alerting rules for critical and warning conditions |
| 4.9 | Operational runbooks | Finalize debugging and operations documentation |
| 4.10 | Performance baseline | Measure and document commit latency, throughput, resource usage |
| 4.11 | Security audit prep | Review all unsafe code (should be none), review crypto usage |
| 4.12 | README and quickstart | Polish top-level README with getting-started guide |

### Exit Criteria

- [ ] All E2E test scenarios pass
- [ ] System survives 24-hour continuous operation with failure injection
- [ ] No safety violations detected across all test runs
- [ ] Commit latency < 500ms (4 nodes, LAN)
- [ ] Throughput > 100 tx/s (simple transfers)
- [ ] Memory usage < 256 MB (idle)
- [ ] All Grafana dashboards functional
- [ ] All runbooks tested by a second engineer
- [ ] No `unsafe` code in consensus or execution paths

---

## 8. Post-MVP — Future Work

These items are explicitly out of scope for the MVP but are natural extensions:

### 8.1 Near-Term (Post-MVP)

| Item | Description | Complexity |
|------|-------------|------------|
| Block sync protocol | Fast-sync for new nodes (download blocks, not replay consensus) | Medium |
| State sync | Snapshot-based sync (download state at height H, verify Merkle proof) | High |
| Light client | Verify commits without full state, using validator set tracking | Medium |
| WebSocket subscriptions | Real-time event streaming for new blocks, tx confirmations | Low |
| Peer exchange (PEX) | Dynamic peer discovery beyond static seeds | Medium |
| Transaction indexing | Index transactions by sender, recipient, contract for efficient queries | Medium |

### 8.2 Medium-Term

| Item | Description | Complexity |
|------|-------------|------------|
| Slashing | Penalize equivocating validators (requires token economics) | High |
| On-chain governance | Validator set changes via voting instead of admin keys | High |
| Contract standards | Standard interfaces (token, NFT, etc.) | Medium |
| Gas price market | Dynamic gas pricing based on demand | Medium |
| Parallel execution | Execute independent transactions in parallel (with deterministic ordering) | High |
| Snapshot pruning | Aggressive state pruning with periodic snapshots | Medium |

### 8.3 Long-Term

| Item | Description | Complexity |
|------|-------------|------------|
| Cross-chain communication | IBC-style or bridge protocols | Very High |
| Permissionless validation | Stake-based validator joining | High |
| EVM compatibility | EVM execution engine alongside WASM | Very High |
| Formal verification | TLA+ or similar specification of consensus protocol | High |
| HSM integration | Hardware security module for key management | Medium |
| Geographic distribution | Optimizations for WAN latency (pipelining, view change) | High |

---

## 9. Risk Register

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Determinism bug in Merkle tree | Medium | S0 | Extensive cross-platform testing, property-based tests |
| wasmtime fuel metering inaccuracy | Low | S1 | Validate against reference implementation, fuzz testing |
| RocksDB version incompatibility | Low | S2 | Pin RocksDB version, test upgrades |
| Performance below targets | Medium | S2 | Profile early (Phase 2), optimize hot paths |
| Scope creep beyond MVP | High | Schedule | Strict adherence to non-goals list |
| Single-developer bottleneck | Medium | Schedule | Document everything, keep modules independently testable |

---

## 10. Milestone Summary

```
Week  1-2:   Phase 0 — Types compile, crypto works, CI green
Week  3-5:   Phase 1 — Consensus core passes all safety tests
Week  6-8:   Phase 2 — 4-node cluster commits blocks via Docker
Week  9-11:  Phase 3 — Contracts deploy and execute, validators update
Week  12-13: Phase 4 — All tests pass, 24h soak test clean, docs complete

MVP COMPLETE: A minimal, rigorous BFT blockchain with:
    ✓ Deterministic finality
    ✓ WASM smart contracts
    ✓ Dynamic validator sets
    ✓ Full observability
    ✓ Docker-based testing
    ✓ Crash recovery
    ✓ Comprehensive documentation
```

---

## Definition of Done — Roadmap

- [x] Phased implementation plan with clear deliverables per phase
- [x] Exit criteria for each phase (testable, specific)
- [x] Time estimates per phase
- [x] Post-MVP future work categorized by time horizon
- [x] Risk register with mitigations
- [x] Milestone summary
- [x] No Rust source code included
