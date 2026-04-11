# C4 Level 2: Container Diagram

Shows the major deployable/runnable units inside a single RustBFT node process.

```mermaid
flowchart TB
    Client(["👤 Client"])
    Peer(["🌐 Peer Nodes"])
    Prom(["📊 Prometheus"])

    subgraph node["RustBFT Process (single binary)"]
        direction TB

        subgraph async_layer["Async Layer — Tokio Runtime"]
            RPC[("🌐 RPC Server\nAxum HTTP\nsrc/rpc/")]
            P2P[("📡 P2P Manager\nTCP gossip\nsrc/p2p/")]
            Timer[("⏱ Timer Service\ntokio::spawn\nsrc/consensus/timer.rs")]
            Metrics[("📊 Metrics Server\nPrometheus HTTP\nsrc/metrics/")]
        end

        Router[("🔀 Command Router\ncrossbeam bridge\nsrc/router/")]

        subgraph sync_layer["Sync Layer — Dedicated OS Thread"]
            Core[("🧠 Consensus Core\nBFT state machine\nsrc/consensus/state.rs")]
        end

        subgraph storage_layer["Shared State — Arc + RwLock"]
            BS[("🗄 BlockStore\nRocksDB\nsrc/storage/block_store.rs")]
            SS[("📸 StateStore\nsnapshots\nsrc/storage/state_store.rs")]
            WAL[("📝 WAL\ncrash recovery\nsrc/storage/wal.rs")]
            AppState[("💾 AppState\naccounts + contracts\nsrc/state/accounts.rs")]
        end

        SE[("⚙️ State Executor\ntx execution\nsrc/state/executor.rs")]
        CR[("📦 Contract Runtime\nWasmtime WASM\nsrc/contracts/")]
    end

    Client -->|"JSON-RPC 2.0"| RPC
    Peer <-->|"TCP frames"| P2P
    Prom -->|"HTTP scrape"| Metrics

    RPC -->|"reads"| BS
    RPC -->|"reads"| AppState

    P2P -->|"ConsensusEvent\nmpsc"| Router
    Timer -->|"ConsensusEvent\nmpsc"| Router
    Router -->|"ConsensusEvent\ncrossbeam"| Core

    Core -->|"ConsensusCommand\ncrossbeam"| Router
    Router -->|"NetworkMessage\nmpsc"| P2P
    Router -->|"TimerCommand\nmpsc"| Timer
    Router -->|"execute_block()"| SE
    Router -->|"writes"| BS
    Router -->|"writes"| SS
    Router -->|"writes"| WAL

    SE -->|"runs txs"| AppState
    SE -->|"executes WASM"| CR
    SE -->|"BlockExecuted event"| Router
```

## Container Responsibilities

| Container | Thread | Responsibility |
|-----------|--------|----------------|
| **RPC Server** | Tokio | JSON-RPC 2.0 HTTP — query & submit |
| **P2P Manager** | Tokio | TCP accept, handshake, encrypted gossip |
| **Timer Service** | Tokio | Schedule/cancel timeouts per (height, round) |
| **Metrics Server** | Tokio | Expose Prometheus metrics endpoint |
| **Command Router** | Tokio | Bridge: sync consensus ↔ async subsystems |
| **Consensus Core** | OS thread | BFT state machine — pure, no I/O |
| **State Executor** | Tokio (via Router) | Execute transactions, compute state_root |
| **Contract Runtime** | Tokio (via Executor) | Wasmtime WASM sandbox execution |
| **BlockStore** | Shared (Arc) | RocksDB: blocks, hashes, validator sets |
| **StateStore** | Shared (Arc) | AppState snapshots per height |
| **WAL** | Shared (Arc) | Append-only crash-recovery log |
| **AppState** | Shared (Arc+RwLock) | Live account balances, contract storage |
