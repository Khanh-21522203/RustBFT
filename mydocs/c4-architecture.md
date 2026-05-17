# RustBFT C4 Architecture

This document shows RustBFT using C4-style Mermaid diagrams.

## Level 1: System Context

```mermaid
flowchart TB
    User[Client / Operator]
    Peer[Other Validator Nodes]
    Prom[Prometheus / Monitoring]

    subgraph RustBFTBoundary[RustBFT System]
        Node[RustBFT Node]
    end

    User -->|HTTP JSON-RPC: query blocks, accounts, validators, submit tx| Node
    Node -->|HTTP metrics endpoint| Prom
    Node <-->|Encrypted raw TCP P2P: proposals, votes, gossip| Peer
```

## Level 2: Container View

```mermaid
flowchart TB
    Client[Client / Operator]
    Peer[Validator Peer]
    Prom[Prometheus]

    subgraph NodeProcess[RustBFT Node Process]
        Cli[node-cli<br>Tokio process entrypoint]
        Rpc[RPC Server<br>Axum JSON-RPC]
        P2P[P2P Manager<br>Raw TCP + encrypted frames]
        Consensus[Consensus Core<br>Tendermint-style BFT FSM]
        Router[Command Router<br>sync-to-async bridge]
        Timer[Timer Service<br>Tokio timeouts]
        Mempool[Mempool Scaffold<br>pending txs + tx hash lookup]
        Executor[State Executor<br>accounts + contracts]
        Contracts[Contract Runtime<br>Wasmtime]
        Metrics[Metrics Registry<br>Prometheus client]
    end

    subgraph DataStores[Local Data Stores]
        BlockStore[(Block Store<br>redb)]
        TxIndex[(Committed Tx Index<br>tx_hash -> height/index)]
        StateStore[(State Store<br>redb)]
        WAL[(Consensus WAL<br>file)]
    end

    Client -->|HTTP JSON-RPC| Rpc
    Rpc -->|read queries| BlockStore
    Rpc -->|get_tx pending| Mempool
    Rpc -->|get_tx committed| TxIndex
    Rpc -->|broadcast_tx insert| Mempool
    Rpc -->|read account state| Executor

    Peer <-->|encrypted TCP gossip| P2P
    P2P -->|proposal/vote events| Consensus
    Consensus -->|commands| Router
    Router -->|timer commands| Timer
    Timer -->|timeout events| Consensus
    Router -->|broadcast proposal/vote| P2P
    Router -->|ReapTxs / EvictTxs| Mempool
    Router -->|execute committed block| Executor
    Executor -->|contract deploy/call| Contracts
    Router -->|persist committed block| BlockStore
    BlockStore -->|indexes tx hashes| TxIndex
    Router -->|persist app state| StateStore
    Router -->|write/truncate consensus entries| WAL
    Router -->|update counters/gauges/histograms| Metrics
    Prom -->|scrape /metrics| Metrics

    Cli --> Rpc
    Cli --> P2P
    Cli --> Consensus
    Cli --> Router
    Cli --> Timer
```

## Level 3: Component View

```mermaid
flowchart TB
    subgraph CoreCrate[crates/core]
        CoreTypes[Core Types<br>Block, Vote, Proposal, Transaction]
        ValidatorTypes[Validator Types<br>ValidatorSet, ValidatorId, voting_power]
        Crypto[Crypto + Hashing<br>Ed25519, SHA-256]
        Codec[Canonical Codec]
    end

    subgraph ConsensusCrate[crates/consensus]
        ConsensusState[ConsensusCore<br>height, round, step, locks]
        VoteSet[VoteSet<br>quorum aggregation + equivocation evidence]
        Proposer[Proposer Selection<br>weighted round-robin]
        ConsensusEvents[Consensus Events/Commands]
        ConsensusTimer[Timer API]
    end

    subgraph NodeCrate[crates/node]
        Config[Config Loader<br>TOML defaults]
        Bridge[CommandRouter]
        P2PModules[P2P<br>manager, peer, codec, gossip]
        RPCModules[RPC<br>server, handlers, JSON-RPC types]
        Execution[Execution<br>accounts, tx decode, executor, merkle]
        Staking[Staking Scaffold<br>governance, lifecycle, delegation, slashing, rewards]
        ContractModules[Contracts<br>runtime, host API, validation]
        Storage[Storage<br>block store, state store, WAL]
        Observability[Metrics<br>registry + server]
    end

    subgraph CliCrate[crates/node-cli]
        Main[main.rs<br>wires runtime]
    end

    Main --> Config
    Main --> ConsensusState
    Main --> Bridge
    Main --> P2PModules
    Main --> RPCModules
    Main --> Observability
    Main --> Storage
    Main --> Execution
    Main --> Staking
    Main --> ContractModules

    ConsensusState --> ConsensusEvents
    ConsensusState --> VoteSet
    ConsensusState --> Proposer
    ConsensusState --> ConsensusTimer
    VoteSet --> ValidatorTypes
    Proposer --> ValidatorTypes
    ConsensusEvents --> CoreTypes

    Bridge --> ConsensusEvents
    Bridge --> P2PModules
    Bridge --> Execution
    Bridge --> Storage
    Bridge --> Observability
    Execution --> CoreTypes
    Execution --> Staking
    Execution --> ContractModules
    Staking --> ValidatorTypes
    Storage --> CoreTypes
    P2PModules --> CoreTypes
    RPCModules --> Storage
    RPCModules --> Execution

    CoreTypes --> Crypto
    CoreTypes --> Codec
```

## Staking Scaffold View

```mermaid
flowchart TB
    subgraph StakingModule[crates/node/src/staking]
        StakingState[StakingState<br>validators, delegations, unbonding, pending slashes]
        Metadata[ValidatorMetadata<br>moniker, website, contacts, P2P/RPC addresses]
        Economics[ValidatorEconomics<br>min self bond, commission policy]
        Lifecycle[ValidatorLifecycle<br>Candidate, Bonded, Unbonding, Unbonded, Jailed]
        StakingTx[StakingTx<br>Bond, Unbond, Delegate, Undelegate]
        GovernanceTx[GovernanceValidatorTx<br>Add, Remove, UpdatePower, metadata, reason]
        Slashing[SlashEvent<br>future evidence handling]
        Rewards[RewardPolicy / RewardEvent<br>future reward accounting]
    end

    subgraph ConsensusBoundary[Consensus Boundary]
        ValidatorUpdate[ValidatorUpdate<br>id + new_power]
        ValidatorSet[ValidatorSet<br>active voting power]
    end

    GovernanceTx -->|currently implemented scaffold path| StakingState
    GovernanceTx --> Metadata
    GovernanceTx --> Economics
    Metadata -->|stored on ValidatorRecord| StakingState
    Economics -->|stored on ValidatorRecord| StakingState
    StakingTx -->|future staking economics| StakingState
    Slashing -->|future power/stake penalties| StakingState
    Rewards -->|future distribution accounting| StakingState
    Lifecycle --> StakingState
    StakingState -->|emits| ValidatorUpdate
    ValidatorUpdate -->|applied at H+1| ValidatorSet
```

## Consensus State Transitions

```mermaid
stateDiagram-v2
    [*] --> NewHeight : node starts / previous height committed
    NewHeight --> Propose : enter_new_height()

    Propose --> Prevote : valid proposal received
    Propose --> Prevote : timeout_propose / no proposal
    Propose --> Prevote : local proposer builds proposal

    Prevote --> Precommit : >2/3 prevotes for block
    Prevote --> Precommit : >2/3 prevotes for nil
    Prevote --> Precommit : timeout_prevote / no polka

    Precommit --> Commit : >2/3 precommits for same block
    Precommit --> Propose : timeout_precommit / next round

    Commit --> NewHeight : block executed and persisted

    note right of Prevote
        A validator prevotes a block when the proposal is valid
        and does not violate its lock. Otherwise it prevotes nil.
    end note

    note right of Precommit
        A validator locks on a block when a prevote polka exists
        and the full block is available. Nil polka unlocks.
    end note
```

## Block Commit Sequence

```mermaid
sequenceDiagram
    participant Peer as Validator Peer
    participant P2P as P2P Manager
    participant Core as Consensus Core
    participant Router as Command Router
    participant Timer as Timer Service
    participant Exec as State Executor
    participant Store as Block/State Store
    participant WAL as WAL

    Core->>Router: ScheduleTimeout(Propose)
    Router->>Timer: schedule propose timeout

    Peer->>P2P: proposal over encrypted TCP
    P2P->>Core: ProposalReceived
    Core->>Router: WriteWAL(Proposal)
    Router->>WAL: append proposal entry
    Core->>Router: BroadcastVote(Prevote)
    Router->>P2P: gossip prevote

    Peer->>P2P: prevotes
    P2P->>Core: VoteReceived
    Core->>Core: detect >2/3 prevote polka
    Core->>Router: BroadcastVote(Precommit)
    Router->>P2P: gossip precommit

    Peer->>P2P: precommits
    P2P->>Core: VoteReceived
    Core->>Core: detect >2/3 precommit quorum
    Core->>Router: ExecuteBlock
    Router->>Exec: execute committed block
    Exec-->>Router: state_root + validator_updates
    Router-->>Core: BlockExecuted
    Core->>Router: PersistBlock
    Router->>Store: save block and state
    Router->>WAL: truncate after commit
```

## Runtime Loops Per Node

Application-level persistent loops in one running node:

```text
Base loops:
  1. main shutdown/select wait
  2. consensus core event loop
  3. command router loop
  4. timer service command loop
  5. P2P manager select loop
  6. P2P accept loop
  7. RPC HTTP server loop

Conditional loop:
  8. metrics HTTP server loop, only when metrics_enabled = true

Per connected peer:
  +1 peer reader loop
  +1 peer writer loop

One-shot tasks, not persistent loops:
  seed dial tasks
  individual timeout sleep tasks
```

With metrics enabled, the steady-state loop count is:

```text
7 base loops + 1 metrics loop + (2 * connected_peers)
```

```mermaid
flowchart TB
    subgraph Node[One RustBFT Node]
        MainLoop[main select loop<br>crates/node-cli/src/main.rs<br>waits for P2P exit or Ctrl-C]

        subgraph ConsensusThread[Dedicated OS Thread]
            ConsensusLoop[ConsensusCore loop<br>crates/consensus/src/state.rs<br>rx.recv + drain events]
        end

        subgraph TokioRuntime[Tokio Runtime]
            RouterLoop[CommandRouter loop<br>crates/node/src/bridge.rs<br>receives consensus commands]
            TimerLoop[TimerService loop<br>crates/consensus/src/timer.rs<br>receives timer commands]
            P2PLoop[P2P manager loop<br>crates/node/src/p2p/manager.rs<br>control + commands + gossip]
            AcceptLoop[P2P accept loop<br>crates/node/src/p2p/manager.rs<br>accept inbound TCP peers]
            RpcLoop[RPC server loop<br>crates/node/src/rpc/server.rs<br>Axum HTTP JSON-RPC]
            MetricsLoop[Metrics server loop<br>crates/node/src/metrics/server.rs<br>Axum /metrics, conditional]

            subgraph PeerTasks[Per Connected Peer]
                PeerReader[Peer reader loop<br>crates/node/src/p2p/manager.rs<br>read encrypted frames]
                PeerWriter[Peer writer loop<br>crates/node/src/p2p/manager.rs<br>write encrypted frames]
            end

            TimeoutTask[Timeout sleep task<br>crates/consensus/src/timer.rs<br>one-shot per scheduled timeout]
            SeedTask[Seed dial task<br>crates/node/src/p2p/manager.rs<br>one-shot connect attempt]
        end
    end

    MainLoop --> P2PLoop
    P2PLoop --> AcceptLoop
    P2PLoop --> PeerReader
    P2PLoop --> PeerWriter
    ConsensusLoop --> RouterLoop
    RouterLoop --> TimerLoop
    TimerLoop --> TimeoutTask
    RouterLoop --> P2PLoop
    RouterLoop --> RpcLoop
    RouterLoop --> MetricsLoop
    P2PLoop --> SeedTask
```

## Loop Interaction Flow

```mermaid
sequenceDiagram
    participant PeerReader as Peer Reader Loop
    participant P2P as P2P Manager Loop
    participant Core as Consensus Loop
    participant Router as Router Loop
    participant Timer as Timer Loop
    participant PeerWriter as Peer Writer Loop
    participant RPC as RPC Server Loop

    PeerReader->>Core: ProposalReceived / VoteReceived
    Core->>Router: BroadcastVote / ExecuteBlock / WriteWAL
    Router->>P2P: forward broadcast command
    P2P->>PeerWriter: outbound proposal/vote message
    Core->>Router: ScheduleTimeout
    Router->>Timer: schedule timeout
    Timer-->>Core: TimeoutPropose / TimeoutPrevote / TimeoutPrecommit
    RPC->>Router: currently no real mempool path for broadcast_tx
```

## Current Architecture Notes

```text
Consensus algorithm:
  Tendermint-style BFT: Propose -> Prevote -> Precommit -> Commit

Validator networking:
  Custom raw TCP P2P with encrypted frames, not gRPC

Client API:
  HTTP JSON-RPC

Persistence:
  redb block store, redb state store, file-backed consensus WAL

Execution:
  Account state executor with Wasmtime-based contract runtime

Staking scaffold:
  crates/node/src/staking defines validator lifecycle, staking txs, delegation records,
  governance updates, slashing events, and reward events.
  The only minimal behavior currently implemented is GovernanceValidatorTx -> ValidatorUpdate.
  Rich validator metadata/economics stay in StakingState; consensus still consumes only id + power.

Important current MVP gaps:
  Mempool is in-memory only; tx gossip, persistence, recheck, and retry helpers are future work.
  Proposal/vote signature verification is placeholder-wired in node-cli.
  WAL is written but not replayed on startup yet.
  Genesis validator configuration is not fully wired; node currently bootstraps self.
```
