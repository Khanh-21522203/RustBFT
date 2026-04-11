# Data Flow & Channel Topology

Shows how data moves through the RustBFT process: channels, shared state reads/writes.

```mermaid
flowchart LR
    subgraph external["External"]
        Client(["👤 Client\nJSON-RPC"])
        Peers(["🌐 Peer Nodes\nTCP"])
    end

    subgraph async_["Async — Tokio"]
        P2P["📡 P2P Manager\nsrc/p2p/manager.rs"]
        Timer["⏱ TimerService\nsrc/consensus/timer.rs"]
        RPC["🌐 RPC Server\nsrc/rpc/server.rs"]
        Router["🔀 CommandRouter\nsrc/router/mod.rs"]
    end

    subgraph sync_["Sync — OS Thread"]
        Core["🧠 ConsensusCore\nsrc/consensus/state.rs"]
    end

    subgraph exec_["Execution (Router calls sync)"]
        SE["⚙️ StateExecutor\nsrc/state/executor.rs"]
        CR["📦 ContractRuntime\nsrc/contracts/"]
    end

    subgraph storage_["Shared Storage (Arc)"]
        BS[("🗄 BlockStore\nRocksDB")]
        SS[("📸 StateStore")]
        WAL[("📝 WAL")]
        AS[("💾 AppState\nRwLock")]
    end

    %% External → async
    Client -->|"HTTP POST"| RPC
    Peers <-->|"TCP frames"| P2P

    %% Async → Router → Consensus (events)
    P2P -->|"ConsensusEvent\n(mpsc)"| Router
    Timer -->|"ConsensusEvent\n(mpsc)"| Router
    Router -->|"ConsensusEvent\n(crossbeam bounded)"| Core

    %% Consensus → Router (commands)
    Core -->|"ConsensusCommand\n(crossbeam bounded)"| Router

    %% Router dispatches commands
    Router -->|"NetworkMessage\n(mpsc)"| P2P
    Router -->|"TimerCommand\n(mpsc)"| Timer
    Router -->|"execute_block()"| SE
    Router -->|"put_block()"| BS
    Router -->|"put_state()"| SS
    Router -->|"append()"| WAL

    %% Execution
    SE -->|"apply txs"| AS
    SE -->|"call_contract()"| CR
    SE -.->|"BlockExecuted\nConsensusEvent"| Router

    %% RPC reads shared state
    RPC -->|"get_block()"| BS
    RPC -->|"get_account()"| AS
```

## Channel Summary

| Channel | Type | From | To | Capacity |
|---------|------|------|----|---------|
| `P2P → Router` | `mpsc` | P2P Manager | Command Router | bounded |
| `Timer → Router` | `mpsc` | TimerService | Command Router | bounded |
| `Router → Core` | `crossbeam` | Command Router | ConsensusCore | bounded |
| `Core → Router` | `crossbeam` | ConsensusCore | Command Router | bounded |
| `Router → P2P` | `mpsc` | Command Router | P2P Manager | bounded |
| `Router → Timer` | `mpsc` | Command Router | TimerService | bounded |
| `Router → Core` (BlockExecuted) | `crossbeam` | Command Router (post-execute) | ConsensusCore | bounded |

## Shared State Access Patterns

| Component | BlockStore | AppState | StateStore | WAL |
|-----------|-----------|---------|------------|-----|
| RPC Server | read | read | — | — |
| Command Router | write | — | write | write |
| State Executor | — | write | — | — |
| P2P Manager | — | — | — | — |
| ConsensusCore | — | — | — | — |
