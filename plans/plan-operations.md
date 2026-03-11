# Feature: Operations

## 1. Purpose

The Operations module defines the operator-facing procedures, CLI subcommands, and configuration behaviours that make RustBFT runnable in practice: first-time node setup, graceful and forced shutdown, configuration precedence (flags > env vars > file > defaults), rolling restarts, binary upgrades, data backup and restore, and capacity planning. It also defines the `rustbft-node init` and `rustbft-node backup`/`restore` subcommands that operators use during lifecycle events.

## 2. Responsibilities

- Implement `rustbft-node init --config <file>`: create the data directory layout (`/data/blocks`, `/data/state`, `/data/wal`) and validate the genesis file
- Implement `rustbft-node backup --output <path>`: create an atomic RocksDB checkpoint and copy it to the output path
- Implement `rustbft-node restore --input <path>`: stop the node, replace the data directory with the backup, clear the WAL
- Implement configuration precedence: CLI flags override environment variables (prefix `RUSTBFT_`), which override `node.toml` values, which override compiled-in defaults
- Validate all configuration values at startup: address parseability, positive timeouts, valid paths, cross-field constraints
- Implement the 10-step graceful shutdown sequence triggered by SIGTERM/SIGINT with per-step timeouts
- Log the 10-step startup sequence at INFO level so operators can verify correct initialization
- Provide a security checklist as a startup log (DEBUG level) checking key file permissions, RPC binding, admin key presence

## 3. Non-Responsibilities

- Does not implement block sync (catching up from peers after long outage)
- Does not implement automatic failover or leader election at the infrastructure level
- Does not manage OS-level process supervision (use systemd or Docker restart policies)
- Does not implement rolling upgrades with zero-downtime (BFT requires at most f < n/3 nodes down simultaneously)
- Does not manage TLS certificates for RPC or P2P

## 4. Architecture Design

```
$ rustbft-node --config /config/node.toml

Config resolution:
  CLI flags
    └── RUSTBFT_ env vars
          └── node.toml
                └── compiled defaults

Startup sequence (logged at INFO):
  1.  "Loading configuration"           source, path
  2.  "Configuration validated"         all_ok=true
  3.  "Initializing storage"            engine, data_dir
  4.  "Storage integrity check passed"  latest_height
  5.  "Recovering from WAL"             entries_replayed
  6.  "Loading validator set"           validators_count, total_power
  7.  "Starting P2P listener"           addr
  8.  "Connecting to seed peers"        seeds_count
  9.  "Starting RPC server"             addr
  10. "Starting metrics exporter"       addr
  11. "Node started"                    height, node_id, chain_id

Shutdown sequence (on SIGTERM):
  1. Stop accepting RPC requests
  2. Drain in-flight requests (5s)
  3. Close peer connections (5s)
  4. Cancel all timers
  5. Close inbound consensus channel → consensus exits
  6. Join consensus thread (1s)
  7. Wait for in-progress block execution (10s)
  8. Flush storage (10s)
  9. Shutdown Tokio runtime (5s)
  10. Exit 0
```

## 5. Core Data Structures (Rust)

```rust
// src/node/config.rs

pub struct ResolvedConfig {
    pub node: NodeSection,
    pub consensus: ConsensusConfig,
    pub p2p: P2PConfig,
    pub rpc: RpcConfig,
    pub mempool: MempoolConfig,
    pub storage: StorageConfig,
    pub contracts: ContractConfig,
    pub observability: ObservabilityConfig,
    pub logging: LoggingConfig,
    pub source: ConfigSource,
}

pub enum ConfigSource {
    File(PathBuf),
    EnvOnly,
    Defaults,
}

pub struct ConfigValidationError {
    pub field: String,
    pub value: String,
    pub message: String,
}

// src/node/ops.rs — operational subcommand results

pub struct InitResult {
    pub data_dir: PathBuf,
    pub dirs_created: Vec<PathBuf>,
}

pub struct BackupResult {
    pub output_path: PathBuf,
    pub height: u64,
    pub size_bytes: u64,
}
```

## 6. Public Interfaces

```rust
// src/main.rs — all CLI subcommands

#[derive(Subcommand)]
pub enum Command {
    // Runtime
    Start { #[arg(long)] config: PathBuf },

    // First-time setup
    Init {
        #[arg(long)] config: PathBuf,
        #[arg(long, default_value = "false")] force: bool,  // overwrite existing data_dir
    },

    // Key management
    Keygen { #[arg(long)] output: PathBuf },

    // Data management
    Backup { #[arg(long)] config: PathBuf, #[arg(long)] output: PathBuf },
    Restore { #[arg(long)] config: PathBuf, #[arg(long)] input: PathBuf },

    // Debugging
    WalInspect { #[arg(long)] data_dir: PathBuf, #[arg(long, default_value = "false")] json: bool },
    StateDump { #[arg(long)] data_dir: PathBuf, #[arg(long)] height: u64 },
    RepairDb { #[arg(long)] data_dir: PathBuf },
    Replay { #[arg(long)] events: PathBuf, #[arg(long)] genesis: PathBuf, #[arg(long, default_value = "false")] verbose: bool },
}

// src/node/config.rs
pub fn resolve_config(config_path: Option<PathBuf>) -> Result<ResolvedConfig, ConfigValidationError>;
pub fn validate_config(config: &ResolvedConfig) -> Vec<ConfigValidationError>;

// src/node/ops.rs
pub fn init(config: &ResolvedConfig, force: bool) -> Result<InitResult, OperationError>;
pub fn backup(config: &ResolvedConfig, output: PathBuf) -> Result<BackupResult, OperationError>;
pub fn restore(config: &ResolvedConfig, input: PathBuf) -> Result<(), OperationError>;
```

## 7. Internal Algorithms

### Configuration Resolution
```
fn resolve_config(config_path):
    // 1. Start with compiled-in defaults
    config = DefaultConfig::new()

    // 2. Overlay with file values (if file exists)
    if let Some(path) = config_path:
        file_config = toml::from_str(read_file(path))?
        config.merge(file_config)

    // 3. Overlay with environment variables
    // RUSTBFT_P2P_LISTEN_ADDR → config.p2p.listen_addr
    // RUSTBFT_CONSENSUS_TIMEOUT_PROPOSE_MS → config.consensus.timeout_propose_ms
    // RUSTBFT_LOGGING_LEVEL → config.logging.level
    // (full mapping defined in env_var_table)
    for (env_key, env_val) in std::env::vars():
        if env_key.starts_with("RUSTBFT_"):
            apply_env_override(&mut config, env_key, env_val)

    // 4. CLI flags (already parsed by clap, passed in)
    // not applicable here — flags are applied before calling resolve_config

    validate_config(&config)?
    Ok(config)
```

### Configuration Validation
```
fn validate_config(config):
    errors = []
    // Address parseability
    if config.p2p.listen_addr.parse::<SocketAddr>().is_err():
        errors.push(ConfigValidationError { field: "p2p.listen_addr", ... })
    if config.rpc.listen_addr.parse::<SocketAddr>().is_err():
        errors.push(ConfigValidationError { field: "rpc.listen_addr", ... })

    // Positive timeouts
    if config.consensus.timeout_propose_ms == 0:
        errors.push(ConfigValidationError { field: "consensus.timeout_propose_ms", ... })

    // Path existence
    if !config.node.key_file.exists():
        errors.push(ConfigValidationError { field: "node.key_file", message: "key file not found" })

    // Cross-field constraints
    if config.mempool.max_gas_limit > config.chain_params.max_block_gas:
        errors.push(...)

    if !errors.is_empty():
        return Err(errors)
    Ok(())
```

### Init Subcommand
```
fn init(config, force):
    data_dir = config.node.data_dir
    if data_dir.exists() && !force:
        return Err("data_dir already exists; use --force to overwrite")

    dirs = [
        data_dir / "rocksdb",
        data_dir / "wal",
        data_dir / "debug",
    ]
    for dir in dirs:
        create_dir_all(dir)?
        log INFO "Created directory" path=dir

    // Validate genesis file exists and is parseable
    genesis_path = data_dir.parent() / "genesis.json"
    genesis: GenesisConfig = serde_json::from_str(read_file(genesis_path)?)?
    validate_genesis(&genesis)?
    log INFO "Genesis file validated" chain_id=genesis.chain_id validators=genesis.initial_validators.len()

    log INFO "Node initialized. Run: rustbft-node --config <config>"
    Ok(InitResult { data_dir, dirs_created: dirs })
```

### Backup Subcommand
```
fn backup(config, output):
    // 1. Open RocksDB
    db = open_rocksdb(config.storage.data_dir)?

    // 2. Create atomic checkpoint (near-instantaneous COW, does not block writes)
    checkpoint_path = config.storage.data_dir / "checkpoint-tmp"
    db.create_checkpoint(checkpoint_path)?

    // 3. Copy checkpoint to output
    H = get_latest_height(db)
    copy_dir_recursive(checkpoint_path, output)?
    remove_dir_recursive(checkpoint_path)?

    // 4. Report
    size = dir_size_bytes(output)?
    log INFO "Backup complete" height=H output=output size_bytes=size
    Ok(BackupResult { output_path: output, height: H, size_bytes: size })
```

### Graceful Shutdown Sequence
```
fn shutdown(threads, channels, timeout_config):
    log INFO "Shutdown initiated"
    t0 = Instant::now()

    // Step 1-4: Tokio async layer
    tokio_runtime.block_on(async {
        rpc_server.stop_accepting()
        tokio::time::timeout(5s, drain_rpc_requests()).await
        p2p.disconnect_all_peers().await
        timer_service.cancel_all().await
    })

    // Step 5-6: Close consensus inbound → consensus thread exits
    drop(channels.consensus_inbound_tx)
    if let Err(_) = consensus_thread.join_timeout(1s):
        log WARN "Consensus thread did not exit cleanly"

    // Step 7: Wait for state machine
    drop(channels.execute_block_tx)
    if let Err(_) = state_machine_thread.join_timeout(10s):
        log WARN "State machine thread did not exit cleanly"

    // Step 8: Flush storage
    channels.storage_tx.send(StorageCommand::Flush)
    if let Err(_) = storage_thread.join_timeout(10s):
        log WARN "Storage thread did not flush cleanly"

    // Step 9: Shutdown Tokio
    tokio_runtime.shutdown_timeout(Duration::from_secs(5))

    log INFO "Node stopped" uptime_seconds=t0.elapsed().as_secs()
    // Step 10: process exits
```

## 8. Persistence Model

The operations module creates and manages the data directory layout:
```
/data/
  rocksdb/          ← RocksDB database files (block store + state store)
  wal/
    consensus.wal   ← Write-Ahead Log
  debug/
    events.log      ← Optional event recording (RUSTBFT_RECORD_EVENTS=true)
```

The `backup` command creates a RocksDB checkpoint (copy-on-write snapshot) and copies it to the output path. The `restore` command replaces the `rocksdb/` directory with the backup and deletes `wal/` (the WAL from a previous state is invalid after a restore).

## 9. Concurrency Model

All operational subcommands (`init`, `backup`, `restore`) are single-threaded and fully synchronous. The `backup` command relies on RocksDB's built-in checkpoint mechanism, which is thread-safe and non-blocking for concurrent writes. The `restore` command must only be run while the node is stopped.

## 10. Configuration

```toml
# node.toml — complete reference with defaults

[node]
chain_id = "rustbft-testnet-1"       # Required
node_id = "node0"                     # Required
data_dir = "./data"
key_file = "./config/node_key.json"  # Required

[consensus]
timeout_propose_ms = 1000
timeout_prevote_ms = 1000
timeout_precommit_ms = 1000
timeout_delta_ms = 500               # Added per round: round * delta
create_empty_blocks = true

[p2p]
listen_addr = "0.0.0.0:26656"
seeds = []                           # "node_id@host:port" format
max_peers = 50
handshake_timeout_ms = 5000
ping_interval_ms = 10000
pong_timeout_ms = 5000
max_message_size_bytes = 4194304     # 4 MB
reconnect_backoff_max_ms = 60000
peer_ban_duration_ms = 300000

[rpc]
enabled = true
listen_addr = "127.0.0.1:26657"
max_concurrent_requests = 1000
max_request_body_bytes = 1048576
request_timeout_ms = 30000
rate_limit_per_second = 100
rate_limit_burst = 200
broadcast_rate_limit_per_second = 10
cors_allowed_origins = ["*"]

[mempool]
max_txs = 5000
max_bytes = 10485760
max_tx_bytes = 65536
max_gas_limit = 10000000

[storage]
data_dir = "./data"
engine = "rocksdb"
wal_dir = "./data/wal"
pruning_window = 100
pruning_interval_seconds = 60
rocksdb_cache_size_mb = 256
rocksdb_max_open_files = 1000

[contracts]
max_code_size_bytes = 524288
max_memory_pages = 256
max_call_depth = 64

[observability]
metrics_enabled = true
metrics_listen_addr = "0.0.0.0:26660"

[logging]
format = "json"
level = "info"
module_levels = ""
output = "stdout"
```

## 11. Observability

Startup sequence is logged at INFO with structured fields so operators can verify each step:
```
INFO "Node starting"                  version="0.1.0" chain_id="..." node_id="..."
INFO "Configuration loaded"           source="file" path="/config/node.toml"
INFO "Storage initialized"            engine="rocksdb" latest_height=42
INFO "WAL recovery complete"          entries=0 height=42 round=0
INFO "Validator set loaded"           validators=4 total_power=400
INFO "P2P listener started"           addr="0.0.0.0:26656"
INFO "Connecting to seed peers"       seeds=3
INFO "RPC server started"             addr="127.0.0.1:26657"
INFO "Metrics server started"         addr="0.0.0.0:26660"
INFO "Node started"                   height=42 node_id="node0"
```

Shutdown sequence:
```
INFO "Shutdown initiated"             signal="SIGTERM"
INFO "RPC drain complete"             pending_drained=2 elapsed_ms=120
INFO "Peer connections closed"        peers=3
INFO "Consensus thread stopped"       elapsed_ms=5
INFO "State machine stopped"
INFO "Storage flushed"
INFO "Node stopped"                   uptime_seconds=3600 final_height=142
```

## 12. Testing Strategy

- **`test_config_resolution_file_only`**: config file with known values, no env vars → all fields match file
- **`test_config_resolution_env_overrides_file`**: file sets `timeout_propose_ms=1000`, env sets `RUSTBFT_CONSENSUS_TIMEOUT_PROPOSE_MS=2000` → resolved value is 2000
- **`test_config_validation_bad_address`**: set `p2p.listen_addr = "not-an-address"` → validation returns ConfigValidationError
- **`test_config_validation_zero_timeout`**: set `timeout_propose_ms = 0` → validation error
- **`test_config_validation_key_file_missing`**: key_file path does not exist → validation error
- **`test_init_creates_dirs`**: run init on empty dir → rocksdb/, wal/, debug/ directories created
- **`test_init_existing_dir_no_force`**: run init on existing dir without --force → Err returned
- **`test_init_existing_dir_with_force`**: run init with --force → succeeds, dirs re-created
- **`test_backup_creates_checkpoint`**: run backup → output directory contains RocksDB files, height matches latest
- **`test_backup_does_not_block_writes`**: run backup while writes are in progress → no deadlock, backup completes
- **`test_restore_replaces_data_dir`**: create backup, corrupt DB, restore → DB reads correctly post-restore
- **`test_restore_clears_wal`**: after restore, WAL directory is empty → node starts clean from backup height
- **`test_shutdown_sequence_clean`**: SIGTERM → all INFO shutdown log lines appear in order, exit code 0
- **`test_shutdown_rpc_drain_timeout`**: in-flight request hangs past 5s → request dropped, shutdown continues
- **`test_rolling_restart_safety`**: 4-node cluster, restart one node → cluster continues committing blocks, restarted node catches up

## 13. Open Questions

- **Environment variable naming convention**: `RUSTBFT_P2P_LISTEN_ADDR` for nested TOML keys. The full mapping table needs to be codified in the implementation. A proc-macro or codegen approach could auto-generate env var names from the config struct fields.
- **Config hot reload**: The `logging.level` field supports runtime change via admin RPC (reload handle). All other fields require restart. A `SIGHUP` handler that reloads the config file (for safe fields like seeds and rate limits) would improve operational agility. Deferred post-MVP.
