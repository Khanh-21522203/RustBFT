# Feature: Node Binary

## 1. Purpose

The Node Binary (`rustbft-node`) is the executable that wires all modules together into a running blockchain validator node. It owns the startup sequence, reads and validates configuration files, spawns every thread and async task, routes commands between modules, handles OS signals for graceful shutdown, and provides a `keygen` subcommand for generating validator key pairs. It is the integration seam — no business logic lives here; it only composes.

## 2. Responsibilities

- Parse the config file (`node.toml`) and genesis file (`genesis.json`) using `serde` and `toml`
- Validate configuration: required fields present, addresses parseable, key file readable
- Run WAL recovery before starting consensus; reconstitute `ConsensusState` from WAL output
- Perform startup integrity checks on the storage layer; HALT if corruption is detected
- Initialise and start all modules in dependency order:
  1. Storage (threads + reader)
  2. Logging and metrics
  3. State machine
  4. Mempool
  5. Consensus core
  6. P2P networking
  7. Timer service
  8. Command router
  9. RPC server
- Wire all channels and shared handles created by each module into the modules that consume them
- Route `ConsensusCommand`s from the consensus outbound channel to the correct consumer
- Install `SIGTERM`/`SIGINT` signal handlers and execute the graceful shutdown sequence
- Provide `keygen` subcommand: generate an Ed25519 key pair, write to JSON file
- Exit with a non-zero status code on any unrecoverable error

## 3. Non-Responsibilities

- Does not implement any consensus, execution, networking, or storage logic
- Does not serve as a library — it is a binary-only crate
- Does not provide a daemon manager or process supervisor (use systemd/Docker)
- Does not implement hot config reload (restart required for config changes)
- Does not manage OS-level privileges or sandboxing

## 4. Architecture Design

```
$ rustbft-node --config node.toml

main()
  |
  ├── parse CLI args (clap)
  ├── load_config(node.toml)
  ├── load_genesis(genesis.json)
  |
  ├── recover_wal() → WALRecoveryResult
  ├── storage::start()        → (StorageHandle, StorageReader)
  ├── logging::init()
  ├── metrics::init()         → Arc<Metrics>
  ├── StateExecutor::new()
  ├── Mempool::start()        → MempoolHandle
  ├── build ConsensusState from genesis + WAL recovery
  ├── create_channels()       → ChannelSet
  ├── spawn_consensus_thread()
  ├── spawn_state_machine_thread()
  ├── p2p::start()            → (P2PHandle, consensus_event_sender)
  ├── timer::start()
  ├── rpc::start()            → RpcHandle
  ├── metrics::start_server()
  |
  └── run_command_router()    // blocks; routes ConsensusCommands
        |
        ├── on SIGTERM/SIGINT: begin shutdown()
        └── loop: recv ConsensusCommand → route to consumer
```

## 5. Core Data Structures (Rust)

```rust
// src/main.rs / src/node/config.rs

#[derive(Deserialize)]
pub struct NodeConfig {
    pub node: NodeSection,
    pub consensus: ConsensusConfig,
    pub p2p: P2PConfig,
    pub rpc: RpcConfig,
    pub mempool: MempoolConfig,
    pub storage: StorageConfig,
    pub contracts: ContractConfig,
    pub observability: ObservabilityConfig,
    pub logging: LoggingConfig,
}

#[derive(Deserialize)]
pub struct NodeSection {
    pub chain_id: String,
    pub node_id: String,
    pub data_dir: PathBuf,
    pub key_file: PathBuf,
}

#[derive(Deserialize)]
pub struct GenesisConfig {
    pub chain_id: String,
    pub genesis_time: u64,
    pub initial_validators: Vec<ValidatorConfig>,
    pub initial_accounts: Vec<AccountConfig>,
    pub admin_addresses: Vec<String>,
    pub chain_params: ChainParams,
}

// src/node/keygen.rs

pub struct NodeKeyFile {
    pub node_id: String,
    pub public_key: String,    // "ed25519:<hex>"
    pub private_key: String,   // "ed25519:<hex>"
}
```

## 6. Public Interfaces

```rust
// src/main.rs

fn main() {
    let cli = Cli::parse();   // clap
    match cli.command {
        Command::Start { config } => run_node(config),
        Command::Keygen { output }  => run_keygen(output),
    }
}

// src/node/mod.rs

fn run_node(config_path: PathBuf) -> ! {
    let config  = load_config(config_path);
    let genesis = load_genesis(&config.node.data_dir);
    let key     = load_key_file(&config.node.key_file);

    let wal_result = recover_wal(&config.storage);
    let (storage_handle, storage_reader) = storage::start(&config.storage);

    // integrity check — halts on failure
    storage_handle.check_integrity();

    logging::init(&config.logging);
    let metrics = metrics::init(&config.node.node_id);

    let executor      = StateExecutor::new_from_storage(&storage_reader, &genesis);
    let mempool_tx    = mempool::start(config.mempool.clone());
    let consensus_state = build_consensus_state(&genesis, &key, &wal_result);
    let channels      = create_channels();

    spawn_state_machine_thread(channels.execute_block_rx, channels.consensus_inbound_tx.clone(), executor);
    spawn_storage_thread(channels.storage_rx, config.storage.clone());

    let p2p_handle = p2p::start(&config.p2p, mempool_tx.clone(), channels.consensus_inbound_tx.clone());
    let timer_handle = timer::start(channels.timer_cmd_rx, channels.consensus_inbound_tx.clone());

    spawn_consensus_thread(channels.consensus_inbound_rx, channels.consensus_outbound_tx, consensus_state);

    let rpc_handle = rpc::start(&config.rpc, mempool_tx, storage_reader.clone()).await;
    metrics::start_server(&config.observability, metrics.clone());

    // Register signal handlers
    install_signal_handlers(shutdown_tx);

    // Command router — blocks main thread
    run_command_router(channels.consensus_outbound_rx, &channels, p2p_handle.cmd_tx);
}

// src/node/keygen.rs
fn run_keygen(output_path: PathBuf) {
    let (public_key, private_key) = Ed25519KeyPair::generate();
    let node_id = ValidatorId::from_public_key(&public_key);
    write_key_file(output_path, NodeKeyFile { node_id, public_key, private_key });
    println!("Key pair written to {:?}", output_path);
    println!("Public key: ed25519:{}", hex::encode(public_key.as_bytes()));
}
```

## 7. Internal Algorithms

### Config Loading
```
fn load_config(path):
    contents = std::fs::read_to_string(path)?
    config: NodeConfig = toml::from_str(&contents)?
    validate_config(&config)?   // check required fields, parse addresses
    config

fn load_genesis(data_dir):
    path = data_dir.join("genesis.json")
    contents = std::fs::read_to_string(path)?
    genesis: GenesisConfig = serde_json::from_str(&contents)?
    if genesis.initial_validators.is_empty():
        panic!("genesis must have at least one validator")
    genesis
```

### Build ConsensusState from WAL Recovery
```
fn build_consensus_state(genesis, key, wal_result):
    // Load validator set from storage (or genesis if height 0)
    validator_set = if wal_result.height == 0:
        ValidatorSet::from_genesis(genesis)
    else:
        storage.get_validator_set(wal_result.height)?

    ConsensusState {
        height: wal_result.height,
        round:  wal_result.round,
        step:   Step::NewHeight,
        locked_round: wal_result.locked_round,
        locked_block: wal_result.locked_block,
        valid_round:  None,
        valid_block:  None,
        proposal:     None,
        votes:        VoteSet::new(wal_result.height, validator_set.total_power),
        validator_set,
        proposer_state: ProposerState::new(),
        node_id:     key.node_id,
        private_key: key.private_key,
    }
```

### Signal Handling
```
fn install_signal_handlers(shutdown_tx):
    // Tokio signal listener (in async context)
    tokio::spawn(async move {
        tokio::select!:
            _ = tokio::signal::ctrl_c()   => {}
            _ = sigterm_stream.recv()     => {}
        shutdown_tx.send(())
    })
```

### Startup Integrity Check
```
fn check_integrity(storage):
    H = storage.get_latest_height()
    if H == 0: return  // fresh node, no check needed

    block = storage.get_block_by_height(H)
        .ok_or_else(|| fatal("missing latest block at height {H}"))?
    computed_hash = sha256(canonical_encode(&block))
    if computed_hash != block.header.hash:
        fatal("block hash mismatch at height {H}: data corrupted")

    stored_root = storage.get_state_root(H)
        .ok_or_else(|| fatal("missing state root at height {H}"))?
    if stored_root != block.header.state_root:
        fatal("state root mismatch at height {H}: state diverged from block")
```

## 8. Persistence Model

The node binary reads configuration from `node.toml` and `genesis.json` at startup and never writes to them. Persistent state is owned by the storage module. The node binary's only write at startup is potential WAL truncation after recovery (delegated to the storage module).

## 9. Concurrency Model

The `main` function runs on the main OS thread. It creates the Tokio runtime (`tokio::runtime::Builder::new_multi_thread`), spawns async tasks inside it, then spawns the sync OS threads (consensus, state machine, storage). The main thread then enters the synchronous `run_command_router` loop and parks on `crossbeam::channel::recv`. Shutdown is initiated by the signal handler (which sends on a oneshot channel), causing the command router to break its loop and proceed through the shutdown sequence.

## 10. Configuration

```toml
# node.toml

[node]
chain_id = "rustbft-testnet-1"
node_id = "node0"
data_dir = "./data"
key_file = "./config/node_key.json"

[consensus]
timeout_propose_ms = 1000
timeout_prevote_ms = 1000
timeout_precommit_ms = 1000
timeout_delta_ms = 500
create_empty_blocks = true

[p2p]
listen_addr = "0.0.0.0:26656"
seeds = ["node1@172.28.1.2:26656"]
max_peers = 50

[rpc]
listen_addr = "127.0.0.1:26657"
rate_limit_per_second = 100

[mempool]
max_txs = 5000
max_bytes = 10485760

[storage]
data_dir = "./data"
engine = "rocksdb"
pruning_window = 100

[contracts]
max_code_size_bytes = 524288
max_memory_pages = 256

[observability]
metrics_listen_addr = "0.0.0.0:26660"

[logging]
format = "json"
level = "info"
output = "stdout"
```

## 11. Observability

- Log INFO `"Node starting"` with `{version, chain_id, node_id, data_dir}` at startup
- Log INFO `"WAL recovery complete"` with `{height, round, locked_block}` after WAL recovery
- Log INFO `"Storage integrity check passed"` with `{latest_height}` or HALT on failure
- Log INFO `"All modules started"` with list of listening addresses after full startup
- Log INFO `"Shutdown initiated"` with `{signal}` on SIGTERM/SIGINT
- Log INFO at each shutdown step with elapsed time
- Log INFO `"Node stopped"` with `{uptime_seconds, final_height}` on clean exit
- Exit code 0 on clean shutdown, non-zero on any unrecoverable error

## 12. Testing Strategy

- **`test_config_load_valid`**: parse a well-formed `node.toml` → all fields deserialize correctly
- **`test_config_missing_required_field`**: omit `chain_id` → parse returns descriptive error
- **`test_genesis_load_valid`**: parse a well-formed `genesis.json` → validators and accounts loaded
- **`test_genesis_empty_validators`**: genesis with no validators → startup panics with clear message
- **`test_keygen_produces_valid_keypair`**: run keygen, load produced JSON, verify Ed25519 public/private pair is valid and public key matches node_id derivation
- **`test_keygen_output_file_written`**: run keygen with output path → file exists with correct JSON schema
- **`test_startup_integrity_check_passes_fresh_node`**: height=0 → check passes without reading any block
- **`test_startup_integrity_check_halts_on_bad_hash`**: tamper block hash in test DB → check triggers fatal
- **`test_build_consensus_state_from_genesis`**: WAL result height=0 → ConsensusState matches genesis validators
- **`test_build_consensus_state_from_wal`**: WAL result height=5, locked_block=Some(H) → ConsensusState restored correctly
- **`test_signal_handler_triggers_shutdown`**: send SIGTERM in test process → shutdown_tx receives message within 100ms

## 13. Open Questions

- **Hot config reload**: Some operators want to change log levels or peer seeds without restarting. This requires externalizing the reload handle from the logging module. Deferred post-MVP.
- **Init subcommand**: For a first run, the operator needs to generate keys and create a genesis file. A `rustbft-node init` subcommand that scaffolds the `data_dir` with template configs would improve UX. Deferred post-MVP.
