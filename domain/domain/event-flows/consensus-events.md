# Consensus Event Flow

Source: `src/consensus/events.rs`, `src/router/mod.rs`, `src/p2p/manager.rs`, `src/consensus/timer.rs`

## Command / Event Bus Overview

```mermaid
flowchart LR
    subgraph External["External"]
        NET["Remote Peers\nTCP 26656"]
        DISK["RocksDB\nBlockStore + StateStore + WAL"]
    end

    subgraph P2P["P2P Layer Tokio"]
        MGR["P2pManager\nfanout and dedup"]
        PEER["PeerTask\nreader and writer"]
    end

    subgraph TimerSvc["Timer Service Tokio"]
        TMR["TimerService\nSchedule and Cancel"]
    end

    subgraph RouterSvc["CommandRouter Tokio"]
        ROUTER["CommandRouter\nbridges sync and async"]
        EXEC["StateExecutor\nBeginBlock DeliverTx EndBlock"]
    end

    subgraph ConsensusSvc["ConsensusCore OS thread crossbeam"]
        CORE["ConsensusCore\nTendermint BFT"]
    end

    subgraph RPCSvc["RPC Server Tokio 26657"]
        RPC["RPC\nread-only AppState"]
    end

    NET --> PEER
    PEER -->|ProposalReceived VoteReceived| CORE

    TMR -.->|TimeoutPropose TimeoutPrevote TimeoutPrecommit| CORE

    ROUTER -.->|TxsAvailable empty in MVP| CORE
    ROUTER -.->|BlockExecuted state_root validator_updates| CORE

    CORE -->|BroadcastProposal BroadcastVote| ROUTER
    CORE -->|ReapTxs EvictTxs| ROUTER
    CORE -->|ExecuteBlock| ROUTER
    CORE -->|PersistBlock WriteWAL| ROUTER
    CORE -->|ScheduleTimeout CancelTimeout| ROUTER

    ROUTER -->|forward BroadcastProposal BroadcastVote| MGR
    MGR -->|fanout to all peers| PEER
    PEER -->|encrypted frame| NET

    ROUTER -->|Schedule Cancel| TMR

    ROUTER -->|execute_block| EXEC
    EXEC -->|state_root validator_updates| ROUTER

    ROUTER -->|save_block| DISK
    ROUTER -->|save_state| DISK
    ROUTER -->|write_entry truncate| DISK

    RPC -->|read AppState| ROUTER
```

## Event Classification

### ConsensusEvent (inbound to ConsensusCore)

| Event | Source | Trigger |
|---|---|---|
| `ProposalReceived` | P2P reader task | Remote peer sent proposal, after sig verify |
| `VoteReceived` | P2P reader task | Remote peer sent prevote or precommit |
| `TimeoutPropose` | TimerService | Propose timer expired for height, round |
| `TimeoutPrevote` | TimerService | Prevote timer expired |
| `TimeoutPrecommit` | TimerService | Precommit timer expired |
| `TxsAvailable` | CommandRouter | Response to ReapTxs, empty in MVP |
| `BlockExecuted` | CommandRouter | StateExecutor finished execute_block |

### ConsensusCommand (outbound from ConsensusCore)

| Command | Destination | Purpose |
|---|---|---|
| `BroadcastProposal` | P2P Manager | Proposer publishing block proposal |
| `BroadcastVote` | P2P Manager | Node broadcasting prevote or precommit |
| `ReapTxs` | CommandRouter mempool | Proposer requesting transactions |
| `EvictTxs` | CommandRouter mempool | Post-commit eviction, placeholder |
| `ExecuteBlock` | CommandRouter StateExecutor | Trigger deterministic block execution |
| `PersistBlock` | CommandRouter RocksDB | Persist committed block and state |
| `WriteWAL` | CommandRouter WAL | Crash-recovery journal entry |
| `ScheduleTimeout` | TimerService | Arm a one-shot timeout |
| `CancelTimeout` | TimerService | Disarm timeout after commit |

## Async Boundary

`ConsensusCore` runs on a dedicated OS thread using blocking crossbeam recv. Everything else is Tokio async. `CommandRouter` bridges the boundary — this is intentional to avoid async overhead in the hot BFT path.

```mermaid
flowchart TD
    subgraph SyncThread["OS Thread blocking"]
        CC["ConsensusCore\ncrossbeam Receiver"]
    end
    subgraph TokioRuntime["Tokio Runtime"]
        CR["CommandRouter\ncrossbeam recv in async context"]
        P2P["P2pManager"]
        TM["TimerService"]
        ST["StateExecutor and Storage"]
        RPC2["RPC Server"]
    end
    CC -->|ConsensusCommand crossbeam| CR
    CR -->|ConsensusEvent crossbeam| CC
    CR -->|mpsc| P2P
    CR -->|mpsc| TM
    CR --> ST
    CR --> RPC2
```

> **Verified against:** `src/consensus/events.rs` — all enum variants; `src/router/mod.rs` — full match on `ConsensusCommand`; `src/p2p/manager.rs` — fanout logic; `src/main.rs` — thread/task topology.
