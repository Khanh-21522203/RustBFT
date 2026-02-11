use serde::{Deserialize, Serialize};
use std::path::Path;

/// Top-level node configuration (doc 16 section 2, doc 15 section 5).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeConfig {
    #[serde(default)]
    pub node: NodeSection,
    #[serde(default)]
    pub consensus: ConsensusSection,
    #[serde(default)]
    pub p2p: P2pSection,
    #[serde(default)]
    pub rpc: RpcSection,
    #[serde(default)]
    pub mempool: MempoolSection,
    #[serde(default)]
    pub storage: StorageSection,
    #[serde(default)]
    pub observability: ObservabilitySection,
    #[serde(default)]
    pub logging: LoggingSection,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeSection {
    #[serde(default = "default_chain_id")]
    pub chain_id: String,
    #[serde(default = "default_node_id")]
    pub node_id: String,
    #[serde(default = "default_data_dir")]
    pub data_dir: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConsensusSection {
    #[serde(default = "default_timeout_ms")]
    pub timeout_propose_ms: u64,
    #[serde(default = "default_timeout_ms")]
    pub timeout_prevote_ms: u64,
    #[serde(default = "default_timeout_ms")]
    pub timeout_precommit_ms: u64,
    #[serde(default = "default_delta_ms")]
    pub timeout_delta_ms: u64,
    #[serde(default = "default_true")]
    pub create_empty_blocks: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct P2pSection {
    #[serde(default = "default_p2p_listen")]
    pub listen_addr: String,
    #[serde(default)]
    pub seeds: Vec<String>,
    #[serde(default = "default_max_peers")]
    pub max_peers: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RpcSection {
    #[serde(default = "default_rpc_listen")]
    pub listen_addr: String,
    #[serde(default = "default_rate_limit")]
    pub rate_limit_per_second: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MempoolSection {
    #[serde(default = "default_max_txs")]
    pub max_txs: usize,
    #[serde(default = "default_max_bytes")]
    pub max_bytes: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StorageSection {
    #[serde(default = "default_engine")]
    pub engine: String,
    #[serde(default = "default_pruning_window")]
    pub pruning_window: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObservabilitySection {
    #[serde(default = "default_true")]
    pub metrics_enabled: bool,
    #[serde(default = "default_metrics_listen")]
    pub metrics_listen_addr: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoggingSection {
    #[serde(default = "default_log_format")]
    pub format: String,
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub module_levels: Option<String>,
    #[serde(default = "default_log_output")]
    pub output: String,
    #[serde(default)]
    pub file_path: Option<String>,
}

// Default value functions
fn default_chain_id() -> String { "localnet".to_string() }
fn default_node_id() -> String { "node0".to_string() }
fn default_data_dir() -> String { "data".to_string() }
fn default_timeout_ms() -> u64 { 1000 }
fn default_delta_ms() -> u64 { 500 }
fn default_true() -> bool { true }
fn default_p2p_listen() -> String { "0.0.0.0:26656".to_string() }
fn default_max_peers() -> usize { 10 }
fn default_rpc_listen() -> String { "0.0.0.0:26657".to_string() }
fn default_rate_limit() -> u32 { 100 }
fn default_max_txs() -> usize { 5000 }
fn default_max_bytes() -> usize { 10 * 1024 * 1024 }
fn default_engine() -> String { "rocksdb".to_string() }
fn default_pruning_window() -> u64 { 1000 }
fn default_metrics_listen() -> String { "0.0.0.0:26660".to_string() }
fn default_log_format() -> String { "json".to_string() }
fn default_log_level() -> String { "info".to_string() }
fn default_log_output() -> String { "stdout".to_string() }

// Default impls
impl Default for NodeSection {
    fn default() -> Self {
        Self { chain_id: default_chain_id(), node_id: default_node_id(), data_dir: default_data_dir() }
    }
}
impl Default for ConsensusSection {
    fn default() -> Self {
        Self {
            timeout_propose_ms: default_timeout_ms(),
            timeout_prevote_ms: default_timeout_ms(),
            timeout_precommit_ms: default_timeout_ms(),
            timeout_delta_ms: default_delta_ms(),
            create_empty_blocks: true,
        }
    }
}
impl Default for P2pSection {
    fn default() -> Self {
        Self { listen_addr: default_p2p_listen(), seeds: vec![], max_peers: default_max_peers() }
    }
}
impl Default for RpcSection {
    fn default() -> Self {
        Self { listen_addr: default_rpc_listen(), rate_limit_per_second: default_rate_limit() }
    }
}
impl Default for MempoolSection {
    fn default() -> Self {
        Self { max_txs: default_max_txs(), max_bytes: default_max_bytes() }
    }
}
impl Default for StorageSection {
    fn default() -> Self {
        Self { engine: default_engine(), pruning_window: default_pruning_window() }
    }
}
impl Default for ObservabilitySection {
    fn default() -> Self {
        Self { metrics_enabled: true, metrics_listen_addr: default_metrics_listen() }
    }
}
impl Default for LoggingSection {
    fn default() -> Self {
        Self {
            format: default_log_format(),
            level: default_log_level(),
            module_levels: None,
            output: default_log_output(),
            file_path: None,
        }
    }
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            node: NodeSection::default(),
            consensus: ConsensusSection::default(),
            p2p: P2pSection::default(),
            rpc: RpcSection::default(),
            mempool: MempoolSection::default(),
            storage: StorageSection::default(),
            observability: ObservabilitySection::default(),
            logging: LoggingSection::default(),
        }
    }
}

impl NodeConfig {
    /// Load configuration from a TOML file. Falls back to defaults for missing fields.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: NodeConfig = toml::from_str(&contents)?;
        Ok(config)
    }

    /// Load from file if it exists, otherwise return defaults.
    pub fn load_or_default(path: &Path) -> Self {
        match Self::load(path) {
            Ok(cfg) => cfg,
            Err(_) => Self::default(),
        }
    }

    /// Serialize to TOML string (useful for generating template configs).
    pub fn to_toml(&self) -> anyhow::Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }
}
