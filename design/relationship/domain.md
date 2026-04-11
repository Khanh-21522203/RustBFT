+----------------------------- Runtime ------------------------------+
| [NodeRuntime] --bootstraps--> [CommandRouter]                     |
| [CommandRouter] ~~timer_cmd~~> [TimerService]                     |
| [TimerService] (timeout scheduler for consensus events)           |
+--------------------------------------------------------------------+
+---------------------------- Consensus -----------------------------+
| [ConsensusCore] (BFT state machine and vote/quorum logic)         |
+--------------------------------------------------------------------+
+---------------------------- Networking ----------------------------+
| [P2pManager] --spawns--> [loop:p2pAccept]                         |
| [P2pManager] --spawns--> [loop:peerReader]                        |
| [P2pManager] --spawns--> [loop:peerWriter]                        |
| [P2pManager] ~~tx_out~~> [loop:peerWriter]                        |
| [loop:peerReader] ~~tx_gossip~~> [P2pManager]                     |
+--------------------------------------------------------------------+
+---------------------------- Execution -----------------------------+
| [StateExecutor] --calls--> [ContractRuntime]                      |
| [StateExecutor] --mutates--> [AppState]                           |
| [ContractRuntime] --mutates--> [AppState]                         |
| [AppState] (shared chain state for router, executor, contracts, RPC) |
| [ValidatorSet] (shared set for router updates and RPC reads)      |
+--------------------------------------------------------------------+
+------------------------------- API --------------------------------+
| [RpcServer] --serves via--> [RpcState]                            |
| [RpcState] ~~tx_submit~~> [Mempool] (? entry point unknown)       |
+--------------------------------------------------------------------+
+----------------------------- Storage ------------------------------+
| [BlockStore] (block/state-root/validator-set persistence)         |
| [StateStore] (versioned AppState snapshots by height)             |
| [WAL] (consensus crash-recovery write-ahead log)                  |
+--------------------------------------------------------------------+
+-------------------------- Observability ---------------------------+
| [MetricsServer] --exports from--> [Metrics]                       |
| [Metrics] (shared Prometheus registry)                            |
+--------------------------------------------------------------------+

[Runtime] ~~consensus_ev~~> [Consensus]
[Consensus] ~~consensus_cmd~~> [Runtime]
[Runtime] ~~p2p_cmd~~> [Networking]
[Networking] ~~consensus_ev~~> [Consensus]
[Runtime] --executes via--> [Execution]
[Runtime] --persists via--> [Storage]
[Runtime] --records--> [Observability]
[API] --queries--> [Storage]
[API] --reads--> [Execution]
[API] <--http-- [JSON-RPC Clients]
[Observability] <--http-- [Prometheus]
[Networking] --sends socket frames--> [P2P Peers]
[Networking] --accepts peers from--> [P2P Peers]
[Runtime] --fs--> [Filesystem:config/node.toml]
[Runtime] --fs--> [Filesystem:node_key.json]
[Storage] --db--> [RocksDB:/data/blocks]
[Storage] --db--> [RocksDB:/data/state]
[Storage] --fs--> [Filesystem:consensus.wal]
