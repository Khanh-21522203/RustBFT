# RustBFT — Requirements

**Purpose:** Define all functional and non-functional requirements for the RustBFT MVP.
**Audience:** Core engineers, reviewers, and anyone assessing scope.

---

## 1. Functional Requirements

### FR-1: Block Production

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-1.1 | The system MUST produce blocks at a configurable interval (default: 1 second). | P0 |
| FR-1.2 | A designated proposer MUST construct a block from pending transactions. | P0 |
| FR-1.3 | Blocks MUST contain: header (height, timestamp, previous block hash, proposer ID, validator set hash, state root, tx merkle root), body (ordered list of transactions). | P0 |
| FR-1.4 | Empty blocks MAY be produced if no transactions are pending (configurable). | P1 |

### FR-2: Consensus

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-2.1 | The system MUST implement a multi-round BFT consensus protocol with deterministic finality. | P0 |
| FR-2.2 | A block is committed if and only if it receives prevotes and precommits from >2/3 of voting power. | P0 |
| FR-2.3 | No two different blocks MAY be committed at the same height (safety). | P0 |
| FR-2.4 | The system MUST progress to new rounds via configurable timeouts if a round fails to commit. | P0 |
| FR-2.5 | Proposer selection MUST be deterministic and a function of (height, round, validator_set). | P0 |
| FR-2.6 | The consensus core MUST be single-threaded and synchronous (no `.await`). | P0 |
| FR-2.7 | The system MUST tolerate up to f < n/3 Byzantine validators without violating safety. | P0 |
| FR-2.8 | The system MUST maintain liveness when >2/3 of voting power is honest and connected. | P0 |

### FR-3: Networking

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-3.1 | Nodes MUST communicate via authenticated, encrypted TCP connections. | P0 |
| FR-3.2 | The networking layer MUST support peer discovery from a static seed list. | P0 |
| FR-3.3 | Messages MUST be length-prefixed and deserialized before delivery to consensus. | P0 |
| FR-3.4 | The networking layer MUST validate message signatures before forwarding to consensus. | P0 |
| FR-3.5 | The networking layer MUST implement backpressure to prevent memory exhaustion. | P0 |
| FR-3.6 | Gossip: consensus votes MUST be broadcast to all connected peers. | P0 |
| FR-3.7 | Proposals MUST be sent point-to-point from proposer, then gossiped. | P1 |

### FR-4: Mempool

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-4.1 | The mempool MUST accept transactions via RPC and P2P gossip. | P0 |
| FR-4.2 | The mempool MUST perform stateless validation (signature, format, gas limit). | P0 |
| FR-4.3 | The mempool MUST enforce a configurable maximum size (count and bytes). | P0 |
| FR-4.4 | Transactions included in a committed block MUST be evicted from the mempool. | P0 |
| FR-4.5 | The mempool MUST support reaping: returning an ordered subset for block proposal. | P0 |

### FR-5: State Machine

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-5.1 | Block execution MUST follow the sequence: BeginBlock → DeliverTx(×N) → EndBlock → Commit. | P0 |
| FR-5.2 | Execution MUST be deterministic: identical inputs produce identical outputs on all nodes. | P0 |
| FR-5.3 | Each committed block MUST produce a new state root (Merkle hash of all state). | P0 |
| FR-5.4 | Failed transactions MUST revert their local state changes but remain in the block (for gas accounting). | P0 |
| FR-5.5 | The state machine MUST support account balances, nonces, and contract storage. | P0 |

### FR-6: Smart Contract Execution

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-6.1 | The system MUST support deploying and calling smart contracts. | P0 |
| FR-6.2 | Contract execution MUST be synchronous and deterministic. | P0 |
| FR-6.3 | Contracts MUST NOT access wall-clock time, randomness, network, or filesystem. | P0 |
| FR-6.4 | Gas metering MUST be enforced; execution MUST halt when gas is exhausted. | P0 |
| FR-6.5 | Contract bytecode format MUST be defined (MVP: WASM via wasmtime or equivalent). | P0 |
| FR-6.6 | A host API MUST be provided for state read/write, event emission, and caller context. | P0 |

### FR-7: Dynamic Validator Sets

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-7.1 | The validator set MUST be part of the replicated state. | P0 |
| FR-7.2 | Validator set changes decided at height H MUST take effect at height H+1. | P0 |
| FR-7.3 | The validator set MUST be fixed for the entire duration of a height. | P0 |
| FR-7.4 | Add, remove, and voting power update operations MUST be supported. | P0 |
| FR-7.5 | No validator set update MAY reduce the total voting power below the minimum quorum threshold. | P0 |
| FR-7.6 | Validator set updates MUST be submitted as special transactions and executed during EndBlock. | P0 |

### FR-8: Storage

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-8.1 | The system MUST persist committed blocks, state, and validator sets. | P0 |
| FR-8.2 | A Write-Ahead Log (WAL) MUST be maintained for crash recovery of in-progress consensus rounds. | P0 |
| FR-8.3 | After crash and restart, the node MUST resume from the last committed height. | P0 |
| FR-8.4 | Storage operations MUST NOT block the consensus core directly (offloaded to dedicated thread). | P0 |
| FR-8.5 | State MUST be queryable by height (at minimum: latest and latest-1). | P1 |

### FR-9: RPC / API

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-9.1 | The system MUST expose an HTTP JSON-RPC API. | P0 |
| FR-9.2 | Supported queries: block by height, block by hash, latest height, transaction by hash, account state, validator set. | P0 |
| FR-9.3 | The system MUST accept transaction submissions via RPC. | P0 |
| FR-9.4 | RPC MUST be read-only with respect to consensus state. | P0 |
| FR-9.5 | RPC MUST support health check and status endpoints. | P0 |

### FR-10: Observability

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-10.1 | The system MUST export Prometheus metrics. | P0 |
| FR-10.2 | The system MUST emit structured logs (JSON format). | P0 |
| FR-10.3 | Key metrics: height, round, step, peer count, mempool size, block latency, consensus duration. | P0 |
| FR-10.4 | The system MUST support configurable log levels at runtime. | P1 |

---

## 2. Non-Functional Requirements

### NFR-1: Safety

| ID | Requirement |
|----|-------------|
| NFR-1.1 | The system MUST NOT commit two different blocks at the same height under any circumstance where f < n/3. |
| NFR-1.2 | The system MUST NOT allow state divergence between honest nodes given the same committed block sequence. |
| NFR-1.3 | All cryptographic operations MUST use well-audited libraries (ed25519-dalek or equivalent). |

### NFR-2: Liveness

| ID | Requirement |
|----|-------------|
| NFR-2.1 | The system MUST make progress (commit new blocks) when >2/3 of voting power is honest and connected. |
| NFR-2.2 | Round timeouts MUST increase exponentially to handle transient failures. |
| NFR-2.3 | The system MUST recover liveness after network partitions heal. |

### NFR-3: Performance (MVP Targets)

| ID | Requirement | Target |
|----|-------------|--------|
| NFR-3.1 | Block commit latency (4 validators, LAN) | < 500ms |
| NFR-3.2 | Transaction throughput (simple transfers) | > 100 tx/s |
| NFR-3.3 | Memory usage per node (idle) | < 256 MB |
| NFR-3.4 | Startup time (cold, from persisted state) | < 5s |

### NFR-4: Operability

| ID | Requirement |
|----|-------------|
| NFR-4.1 | Nodes MUST support graceful shutdown (drain in-flight work, flush WAL). |
| NFR-4.2 | Configuration MUST be file-based (TOML) with environment variable overrides. |
| NFR-4.3 | The system MUST run in Docker containers with no host dependencies beyond the container runtime. |
| NFR-4.4 | Log output MUST include node identity, height, round for every consensus-related message. |

### NFR-5: Determinism

| ID | Requirement |
|----|-------------|
| NFR-5.1 | All serialization MUST use a canonical, platform-independent format. |
| NFR-5.2 | Hash computation MUST use a single, specified algorithm (SHA-256). |
| NFR-5.3 | Transaction ordering within a block MUST be deterministic (proposer-defined, not sorted post-hoc). |
| NFR-5.4 | Floating-point arithmetic MUST NOT be used in any consensus or execution path. |
| NFR-5.5 | Map iteration MUST use ordered maps (BTreeMap) or explicit sorting. |

### NFR-6: Testability

| ID | Requirement |
|----|-------------|
| NFR-6.1 | The consensus core MUST be testable in isolation without networking. |
| NFR-6.2 | All layers MUST accept trait-based dependencies for mocking. |
| NFR-6.3 | A deterministic replay mode MUST be supported for debugging. |
| NFR-6.4 | Integration tests MUST run a 4-node cluster in Docker. |

---

## 3. Constraints

| Constraint | Rationale |
|------------|-----------|
| Rust (latest stable) | Memory safety, performance, type system enforcement of invariants. |
| No `unsafe` in consensus or execution paths | Determinism and auditability. |
| No floating point in consensus/execution | Platform-dependent rounding violates determinism. |
| No `HashMap` in consensus/execution | Iteration order is non-deterministic. |
| Single-threaded consensus core | Eliminates concurrency bugs in the most critical path. |
| Synchronous contract execution | Determinism requires no async side effects. |

---

## 4. Assumptions

1. The initial validator set is configured at genesis (static config file).
2. Network latency between validators is bounded (LAN or low-latency WAN for MVP).
3. Clocks are loosely synchronized (NTP); timestamps are advisory, not consensus-critical.
4. The MVP targets 4–7 validators; scaling beyond 100 is out of scope.
5. Key management is file-based (no HSM integration for MVP).

---

## Definition of Done — Requirements

- [x] All functional requirements enumerated with IDs and priorities
- [x] Non-functional requirements with measurable targets where applicable
- [x] Constraints and assumptions explicitly stated
- [x] No vague language ("should try to", "ideally")
- [x] Every requirement is testable or verifiable
- [x] No Rust source code included
