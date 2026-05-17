# Node Crate Architecture

This file focuses on `crates/node`, which is the runtime integration crate for storage, execution, networking, RPC, mempool, metrics, contracts, and validator/staking state.

## Module Map

```mermaid
flowchart TB
    CLI["crates/node-cli<br>main.rs<br>Runtime bootstrap"]

    subgraph NODE["crates/node"]
        CONFIG["config<br>NodeConfig"]
        BRIDGE["bridge<br>CommandRouter"]
        RPC["rpc<br>JSON-RPC server<br>handlers/types"]
        P2P["p2p<br>TCP manager<br>peer handshake<br>gossip<br>codec/messages"]
        MEMPOOL["mempool<br>local pending txs<br>tx hash/status"]
        EXEC["execution<br>StateExecutor<br>accounts<br>tx decoder<br>Merkle root"]
        CONTRACTS["contracts<br>Wasm runtime<br>host API<br>validation"]
        STORAGE["storage<br>BlockStore<br>StateStore<br>WAL"]
        METRICS["metrics<br>registry<br>exporter"]
        STAKING["staking<br>scaffold<br>governance<br>lifecycle<br>rewards<br>slashing<br>metadata"]
    end

    subgraph EXTERNAL["Other crates"]
        CORE["rustbft-core<br>Block/Tx/ValidatorSet<br>crypto/codecs"]
        CONSENSUS["rustbft-consensus<br>ConsensusCore<br>TimerService<br>ConsensusEvent/Command"]
    end

    CLI --> CONFIG
    CLI --> RPC
    CLI --> P2P
    CLI --> BRIDGE
    CLI --> STORAGE
    CLI --> EXEC
    CLI --> CONTRACTS
    CLI --> MEMPOOL
    CLI --> METRICS
    CLI --> CONSENSUS

    RPC --> STORAGE
    RPC --> MEMPOOL
    RPC --> EXEC
    P2P --> CORE
    P2P --> CONSENSUS
    BRIDGE --> P2P
    BRIDGE --> STORAGE
    BRIDGE --> EXEC
    BRIDGE --> CONTRACTS
    BRIDGE --> MEMPOOL
    BRIDGE --> METRICS
    BRIDGE --> CONSENSUS
    EXEC --> CONTRACTS
    EXEC --> STAKING
    EXEC --> CORE
    STORAGE --> CORE
    STAKING --> CORE
```

## Runtime Ownership

```mermaid
flowchart LR
    subgraph SYNC["Synchronous consensus side"]
        CORE_LOOP["ConsensusCore thread<br>owns consensus state machine"]
        CONS_RX["rx_consensus_ev<br>crossbeam receiver"]
        CONS_TX["tx_consensus_cmd<br>crossbeam sender"]
        CORE_LOOP --> CONS_RX
        CORE_LOOP --> CONS_TX
    end

    subgraph ASYNC["Tokio runtime side"]
        ROUTER["CommandRouter loop<br>bridges consensus commands"]
        TIMER["TimerService loop<br>schedules consensus timeouts"]
        P2PM["P2pManager loop<br>accept/connect/fanout"]
        RPCS["RpcServer<br>JSON-RPC HTTP"]
        METS["MetricsServer<br>Prometheus scrape endpoint"]
    end

    subgraph SHARED["Shared node state"]
        APP["AppState<br>RwLock"]
        VS["ValidatorSet<br>RwLock"]
        POOL["Mempool<br>Arc"]
        BLOCKS["BlockStore<br>committed blocks<br>tx index"]
        STATE["StateStore<br>latest app snapshots"]
        WAL["WAL<br>consensus recovery log"]
        WASM["ContractRuntime<br>Mutex"]
    end

    CONS_TX --> ROUTER
    ROUTER --> CONS_RX
    TIMER --> CONS_RX
    P2PM --> CONS_RX
    ROUTER --> TIMER
    ROUTER --> P2PM

    RPCS --> POOL
    RPCS --> BLOCKS
    RPCS --> APP
    RPCS --> VS
    ROUTER --> POOL
    ROUTER --> BLOCKS
    ROUTER --> STATE
    ROUTER --> WAL
    ROUTER --> APP
    ROUTER --> VS
    ROUTER --> WASM
```

## Transaction To Commit Flow

```mermaid
sequenceDiagram
    participant C as Client
    participant RPC as node::rpc
    participant M as node::mempool
    participant CONS as rustbft-consensus
    participant R as node::bridge::CommandRouter
    participant E as node::execution
    participant S as node::storage
    participant P as node::p2p

    C->>RPC: broadcast_tx(raw tx)
    RPC->>M: insert tx by hash
    RPC-->>C: accepted + tx hash
    CONS->>R: ReapTxs(max_bytes)
    R->>M: reap pending txs
    R-->>CONS: TxsAvailable(txs)
    CONS->>R: BroadcastProposal / BroadcastVote
    R->>P: fanout consensus message
    CONS->>R: ExecuteBlock(block)
    R->>E: execute_block(AppState, ContractRuntime, block)
    E-->>R: state_root + validator_updates
    R-->>CONS: BlockExecuted(height, state_root, validator_updates)
    CONS->>R: PersistBlock(block, state_root)
    R->>S: save block + tx index
    R->>S: save latest app state
    R->>S: truncate WAL
    R->>M: remove committed tx hashes
    C->>RPC: get_tx(hash)
    RPC->>S: load_tx_commit(hash)
    RPC-->>C: committed / pending / unknown
```

## Consensus Command Routing

```mermaid
flowchart TB
    CMD{"ConsensusCommand"}

    CMD -->|"BroadcastProposal<br>BroadcastVote"| P2P_OUT["p2p::manager<br>fanout to peers"]
    CMD -->|"ScheduleTimeout<br>CancelTimeout"| TIMER_OUT["TimerService<br>schedule/cancel timeout"]
    CMD -->|"ExecuteBlock"| EXEC_OUT["execution::StateExecutor<br>mutate AppState<br>run contracts<br>collect validator updates"]
    CMD -->|"PersistBlock"| STORE_OUT["storage<br>save block<br>save state<br>truncate WAL<br>evict committed mempool txs"]
    CMD -->|"WriteWAL"| WAL_OUT["storage::WAL<br>append recovery entry"]
    CMD -->|"ReapTxs"| MEM_REAP["mempool<br>reap txs then send TxsAvailable"]
    CMD -->|"EvictTxs"| MEM_EVICT["mempool<br>remove bad or committed txs"]

    EXEC_OUT -->|"BlockExecuted event"| CONS_IN["ConsensusEvent<br>back to consensus core"]
    MEM_REAP -->|"TxsAvailable event"| CONS_IN
```

## Node Crate Boundary

```mermaid
flowchart LR
    subgraph INSIDE["Inside crates/node"]
        RPC2["RPC facade"]
        P2P2["P2P transport"]
        BRIDGE2["CommandRouter"]
        EXEC2["Execution"]
        STORE2["Storage"]
        POOL2["Mempool"]
        STAKE2["Staking scaffold"]
        CONTRACT2["Contract runtime"]
    end

    subgraph OUTSIDE["Outside crates/node"]
        CLIENT["External clients"]
        PEERS["Other validators"]
        CONS2["Consensus crate"]
        CORE2["Core types/crypto"]
    end

    CLIENT -->|"JSON-RPC"| RPC2
    PEERS <-->|"encrypted TCP gossip"| P2P2
    CONS2 <-->|"events/commands"| BRIDGE2
    RPC2 --> POOL2
    RPC2 --> STORE2
    BRIDGE2 --> P2P2
    BRIDGE2 --> EXEC2
    BRIDGE2 --> STORE2
    BRIDGE2 --> POOL2
    EXEC2 --> CONTRACT2
    EXEC2 --> STAKE2
    RPC2 --> CORE2
    P2P2 --> CORE2
    EXEC2 --> CORE2
    STORE2 --> CORE2
```

## Reading Notes

- `node-cli` owns process bootstrap and creates the shared runtime objects.
- `crates/node` is not only networking; it is the node integration layer.
- `CommandRouter` is the main internal hub because consensus cannot directly import storage, execution, contracts, or P2P.
- The current network protocol is custom TCP framing with typed consensus messages, not gRPC.
- The current staking module is scaffold-level and only partially participates through validator update transactions.
