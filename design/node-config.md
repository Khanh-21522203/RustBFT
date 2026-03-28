# Node Configuration

## Purpose

Loads node runtime parameters from a TOML file (or uses defaults) and exposes them to all subsystems at startup. Provides a single typed struct covering networking, consensus timeouts, storage, observability, and logging.

## Scope

**In scope:**
- TOML deserialization into `NodeConfig` with per-section structs
- Defaults for every field — node runs without any config file
- Config path from CLI argument (first argument to binary, default `"config/node.toml"`)

**Out of scope:**
- Hot reload / config watch
- Environment variable overrides (except `RUST_LOG` for log level)
- Config validation (no range checks on timeout values or port numbers)
- Config generation CLI command

## Primary User Flow

1. Binary starts; reads first CLI argument as config path (default `"config/node.toml"`).
2. `NodeConfig::load_or_default(path)` — reads TOML if file exists, otherwise returns `NodeConfig::default()`.
3. Each subsystem reads its section: `cfg.node`, `cfg.consensus`, `cfg.p2p`, `cfg.rpc`, `cfg.observability`, `cfg.logging`.
4. Logging initialized from `cfg.logging` before any other subsystem starts.

## Data Model

`NodeConfig` (`src/config/mod.rs`): top-level struct with 8 sections, all with `#[serde(default)]`.

**`NodeSection`** (key `[node]`):
- `chain_id: String` — default `"localnet"`. Used in P2P handshake chain ID check.
- `node_id: String` — default `"node0"`. Informational only in MVP (not derived from key).
- `data_dir: String` — default `"data"`. Base path for `{data_dir}/blocks/`, `{data_dir}/state/`, `{data_dir}/consensus.wal`, `{data_dir}/node_key.json`.

**`ConsensusSection`** (key `[consensus]`):
- `timeout_propose_ms: u64` — default 1000ms
- `timeout_prevote_ms: u64` — default 1000ms
- `timeout_precommit_ms: u64` — default 1000ms
- `timeout_delta_ms: u64` — default 500ms. Added per round to timeouts.
- `create_empty_blocks: bool` — default `true`. Maps to `ConsensusConfig.allow_empty_blocks`.

**`P2pSection`** (key `[p2p]`):
- `listen_addr: String` — default `"0.0.0.0:26656"`
- `seeds: Vec<String>` — default empty. Format: `"<hex_id>@<ip>:<port>"`.
- `max_peers: usize` — default 10 (not enforced in MVP).

**`RpcSection`** (key `[rpc]`):
- `listen_addr: String` — default `"0.0.0.0:26657"`
- `rate_limit_per_second: u32` — default 100 (not enforced in MVP).

**`MempoolSection`** (key `[mempool]`):
- `max_txs: usize` — default 5000 (no mempool implementation; field unused).
- `max_bytes: usize` — default 10MB (unused).

**`StorageSection`** (key `[storage]`):
- `engine: String` — default `"rocksdb"` (only supported engine; field informational).
- `pruning_window: u64` — default 1000 (pruning functions exist but are never called).

**`ObservabilitySection`** (key `[observability]`):
- `metrics_enabled: bool` — default `true`. Controls whether `MetricsServer` is started.
- `metrics_listen_addr: String` — default `"0.0.0.0:26660"`.

**`LoggingSection`** (key `[logging]`):
- `format: String` — `"json"` (default) or `"text"`.
- `level: String` — default `"info"`. Used as fallback if `RUST_LOG` unset.
- `module_levels: Option<String>` — e.g. `"rustbft::p2p=debug"`. Passed to `EnvFilter::try_new`.
- `output: String` — default `"stdout"` (file output not implemented).
- `file_path: Option<String>` — defined but not used.

## Interfaces and Contracts

**`NodeConfig::load(path) -> anyhow::Result<Self>`** — reads file and parses TOML. Returns error if file missing or invalid TOML.

**`NodeConfig::load_or_default(path) -> Self`** — silently falls back to all defaults on any error.

**`NodeConfig::to_toml(&self) -> anyhow::Result<String>`** — serialize to TOML string (useful for generating template config files).

## Dependencies

**External crates:**
- `toml v0.8` — `toml::from_str` for deserialization, `toml::to_string_pretty` for serialization
- `serde` with `derive` — `Deserialize` / `Serialize` on all config structs
- `std::fs::read_to_string` — file I/O

## Failure Modes and Edge Cases

- **Missing config file:** `load_or_default` silently uses defaults. No warning logged.
- **Invalid TOML / unknown field:** Serde will fail parsing — `load_or_default` falls back to defaults without indication.
- **Invalid `listen_addr`:** Not validated at load time — parsing fails at bind time in P2P or RPC server startup.
- **Relative `data_dir`:** Interpreted relative to the process working directory; no resolution to absolute path.
- **`RUST_LOG` environment variable:** Takes precedence over `logging.level` when `module_levels` is not set in config. If `module_levels` is set, `RUST_LOG` is ignored.
- **`node_id` vs actual key:** `node_id` in config is informational; the actual node identity is derived from the ed25519 keypair loaded from `{data_dir}/node_key.json`.

## Observability and Debugging

- `info!(config_path, chain_id, node_id, "Loading configuration")` logged at startup after config load.
- To generate a template config: call `NodeConfig::default().to_toml()`.

## Risks and Notes

- `load_or_default` gives no feedback when it falls back to defaults — a typo in TOML silently produces a node running with defaults.
- All timeout fields are `u64` milliseconds with no minimum/maximum validation — setting `timeout_propose_ms = 0` is accepted.
- `MempoolSection` fields are parsed but never read — the mempool is not implemented.
- `StorageSection.engine` is always `"rocksdb"` — alternative storage backends are not supported.

Changes:

