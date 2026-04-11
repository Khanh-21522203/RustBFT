Entry:
  [main] --starts--> [NodeRuntime]
  [RpcServer] <--http-- [JSON-RPC Clients]
  [MetricsServer] <--http-- [Prometheus]

Application:
  [NodeRuntime] --bootstraps--> [CommandRouter]
  [NodeRuntime] --bootstraps--> [ConsensusCore]
  [NodeRuntime] --bootstraps--> [TimerService]
  [NodeRuntime] --bootstraps--> [P2pManager]
  [NodeRuntime] --bootstraps--> [RpcServer]
  [NodeRuntime] --bootstraps--> [RpcState]
  [NodeRuntime] --bootstraps--> [MetricsServer]
  [NodeRuntime] --bootstraps--> [Metrics]
  [NodeRuntime] --bootstraps--> [StateExecutor]
  [NodeRuntime] --bootstraps--> [ContractRuntime]
  [NodeRuntime] --bootstraps--> [BlockStore]
  [NodeRuntime] --bootstraps--> [StateStore]
  [NodeRuntime] --bootstraps--> [WAL]
  [NodeRuntime] --bootstraps--> [AppState]
  [NodeRuntime] --bootstraps--> [ValidatorSet]
  [NodeRuntime] --fn--> [ConsensusCore]
  [NodeRuntime] --fn--> [P2pManager]
  [NodeRuntime] --fs--> [Filesystem:config/node.toml]
  [NodeRuntime] --fs--> [Filesystem:node_key.json]
  [CommandRouter] ~~p2p_cmd~~> [P2pManager]
  [CommandRouter] ~~timer_cmd~~> [TimerService]
  [CommandRouter] ~~consensus_ev~~> [ConsensusCore]
  [CommandRouter] --executes via--> [StateExecutor]
  [CommandRouter] --calls--> [ContractRuntime]
  [CommandRouter] --mutates--> [AppState]
  [CommandRouter] --updates--> [ValidatorSet]
  [CommandRouter] --persists--> [BlockStore]
  [CommandRouter] --persists--> [StateStore]
  [CommandRouter] --writes--> [WAL]
  [CommandRouter] --records--> [Metrics]

Domain:
  [ConsensusCore] ~~consensus_cmd~~> [CommandRouter]
  [StateExecutor] --calls--> [ContractRuntime]
  [StateExecutor] --mutates--> [AppState]
  [ContractRuntime] --mutates--> [AppState]
  [RpcState] --queries--> [BlockStore]
  [RpcState] --reads--> [AppState]
  [RpcState] --reads--> [ValidatorSet]
  [RpcState] ~~tx_submit~~> [Mempool] (? entry point unknown)
  [loop:peerReader] ~~tx_gossip~~> [P2pManager]
  [loop:peerReader] ~~consensus_ev~~> [ConsensusCore]
  [loop:peerWriter] --sends socket frames--> [P2P Peers]
  [loop:p2pAccept] --accepts peers from--> [P2P Peers]
  [AppState] (shared chain state for router, executor, contracts, and RPC reads)
  [ValidatorSet] (shared validator set for router updates and RPC reads)

Infrastructure:
  [P2pManager] ~~consensus_ev~~> [ConsensusCore]
  [P2pManager] --spawns--> [loop:p2pAccept]
  [P2pManager] --spawns--> [loop:peerReader]
  [P2pManager] --spawns--> [loop:peerWriter]
  [P2pManager] ~~tx_out~~> [loop:peerWriter]
  [TimerService] ~~consensus_ev~~> [ConsensusCore]
  [BlockStore] --db--> [RocksDB:/data/blocks]
  [StateStore] --db--> [RocksDB:/data/state]
  [WAL] --fs--> [Filesystem:consensus.wal]
  [Metrics] (shared Prometheus registry)

External:
  [JSON-RPC Clients]
  [Prometheus]
  [P2P Peers]
  [RocksDB:/data/blocks]
  [RocksDB:/data/state]
  [Filesystem:config/node.toml]
  [Filesystem:node_key.json]
  [Filesystem:consensus.wal]
  [Mempool] (? entry point unknown)

! violations: none
