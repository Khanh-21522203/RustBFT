# RustBFT Documentation

**Purpose:** Top-level entry point for all RustBFT project documentation.
**Audience:** Engineers, operators, and contributors evaluating or working on RustBFT.

---

## What is RustBFT?

RustBFT is a minimal, production-quality Byzantine Fault Tolerant (BFT) blockchain with **deterministic finality**. It is an original design — not a fork or reimplementation of Tendermint, HotStuff, Cosmos SDK, or any existing system.

### Design Goals

- **Deterministic finality:** Once a block is committed, it is final. No forks, no reorgs.
- **Strict layered separation:** Each subsystem (networking, consensus, execution, storage) has a single responsibility with explicit boundaries.
- **Hybrid threading model:** Async Tokio outer shell for I/O; single-threaded deterministic consensus core.
- **Deterministic smart contracts:** Synchronous, gas-metered contract execution with no external side effects.
- **Dynamic validator sets:** Safe validator transitions as part of replicated state.
- **Infra-first:** No token economics, no governance UX. Focus on safety, debuggability, and operability.

### Non-Goals (MVP)

- Token economics or staking rewards
- Permissionless validator joining
- EVM compatibility
- Async or non-deterministic contracts
- Cross-chain communication

---

## Docs Index

| # | Document | Path | Description |
|---|----------|------|-------------|
| 1 | **README** | `docs/README.md` | This file. Project overview and docs index. |
| 2 | **Requirements** | `docs/requirements.md` | Functional and non-functional requirements. |
| 3 | **Architecture Overview** | `docs/architecture/overview.md` | System-level architecture, module layout, data flow. |
| 4 | **Consensus** | `docs/architecture/consensus.md` | BFT consensus protocol design. |
| 5 | **Networking** | `docs/architecture/networking.md` | P2P networking layer. |
| 6 | **State Machine** | `docs/architecture/state-machine.md` | Replicated state machine and block execution. |
| 7 | **Smart Contracts** | `docs/architecture/contracts.md` | Contract execution engine and gas metering. |
| 8 | **Validator Sets** | `docs/architecture/validator-sets.md` | Dynamic validator set management. |
| 9 | **Storage** | `docs/architecture/storage.md` | Persistence, crash recovery, state snapshots. |
| 10 | **RPC / API** | `docs/architecture/rpc.md` | External API surface. |
| 11 | **Event Loop & Threading** | `docs/architecture/event-loop-and-threading.md` | Hybrid async/sync execution model. |
| 12 | **State Transition Table** | `docs/architecture/state-transition-table.md` | Consensus FSM transitions. |
| 13 | **Testing Strategy** | `docs/testing/testing-strategy.md` | Unit, integration, byzantine, and replay tests. |
| 14 | **Observability** | `docs/observability/observability.md` | Metrics, logging, health checks. |
| 15 | **Docker** | `docs/devops/docker.md` | Docker Compose cluster setup. |
| 16 | **Operations** | `docs/devops/operations.md` | Operator runbook and procedures. |
| 17 | **Debugging Consensus** | `docs/runbooks/debugging-consensus.md` | Consensus debugging workflows. |
| 18 | **Failure Models** | `docs/failure-models.md` | Failure taxonomy and mitigations. |
| 19 | **Roadmap** | `docs/roadmap.md` | MVP phases and future work. |

---

## Quick Start (Development)

```
# Clone
git clone <repo-url> && cd rustBFT

# Build
cargo build

# Run unit tests
cargo test

# Run 4-node local cluster
docker-compose -f devops/docker-compose.yml up

# Submit a test transaction
curl -X POST http://localhost:26657/broadcast_tx \
  -H 'Content-Type: application/json' \
  -d '{"tx": "<hex-encoded-tx>"}'
```

---

## Architecture at a Glance

```
┌─────────────────────────────────────────────────────────┐
│                    Operator / Client                     │
│                  (RPC, CLI, Dashboard)                    │
└────────────────────────┬────────────────────────────────┘
                         │ HTTP / WebSocket
┌────────────────────────▼────────────────────────────────┐
│                     RPC Layer                            │
│              (Async Tokio, read-only queries)            │
└────────────────────────┬────────────────────────────────┘
                         │ channel
┌────────────────────────▼────────────────────────────────┐
│               Async Outer Shell (Tokio)                  │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌────────┐  │
│  │Networking│  │ Mempool  │  │  Timers  │  │Metrics │  │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────────┘  │
│       │              │              │                     │
│       └──────────────┴──────────────┘                    │
│                      │ events (mpsc)                     │
└──────────────────────┼──────────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────────┐
│          Consensus Core (Single-Threaded, Sync)          │
│                                                          │
│   ┌─────────────────────────────────────────────────┐   │
│   │  Event Loop: recv() → process() → emit()       │   │
│   │  State: (height, round, step)                   │   │
│   │  No .await, no shared mutable state             │   │
│   └─────────────────────────────────────────────────┘   │
│                      │ commands                          │
└──────────────────────┼──────────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────────┐
│            State Machine + Contract Execution            │
│                                                          │
│   BeginBlock → DeliverTx (×N) → EndBlock → Commit       │
│   Synchronous, deterministic, gas-metered                │
│                      │                                   │
└──────────────────────┼──────────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────────┐
│                    Storage Layer                         │
│          (Block store, State store, WAL)                 │
└─────────────────────────────────────────────────────────┘
```

---

## Definition of Done — README

- [x] Project purpose stated
- [x] Design goals and non-goals listed
- [x] Full docs index with paths
- [x] Quick start commands
- [x] High-level architecture diagram
- [x] No Rust source code included
