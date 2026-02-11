use std::path::Path;
use std::sync::Arc;

use crossbeam_channel::bounded;
use tokio::sync::{mpsc, RwLock};
use tracing::{info, error};

use RustBFT::config::NodeConfig;
use RustBFT::consensus::{
    CommandRouter, ConsensusCommand, ConsensusConfig, ConsensusCore, ConsensusDeps,
    ConsensusEvent, TimerService,
};
use RustBFT::contracts::ContractRuntime;
use RustBFT::crypto::ed25519::load_or_generate_keypair;
use RustBFT::crypto::hash::sha256;
use RustBFT::metrics::registry::Metrics;
use RustBFT::metrics::server::{MetricsConfig, MetricsServer};
use RustBFT::p2p::peer::HandshakeDeps;
use RustBFT::p2p::{P2pConfig, P2pManager};
use RustBFT::rpc::server::{RpcConfig, RpcServer};
use RustBFT::rpc::handlers::RpcState;
use RustBFT::state::accounts::{AppState, ChainParams};
use RustBFT::state::executor::StateExecutor;
use RustBFT::storage::block_store::BlockStore;
use RustBFT::storage::state_store::StateStore;
use RustBFT::types::{SignedProposal, SignedVote, Validator, ValidatorId, ValidatorSet};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // -------------------------------------------------------
    // 0. Load configuration (doc 16 section 2)
    // -------------------------------------------------------
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config/node.toml".to_string());
    let cfg = NodeConfig::load_or_default(Path::new(&config_path));

    // -------------------------------------------------------
    // 0b. Structured logging (doc 14 section 3)
    // -------------------------------------------------------
    init_logging(&cfg.logging);

    info!(
        config_path = %config_path,
        chain_id = %cfg.node.chain_id,
        node_id = %cfg.node.node_id,
        "Loading configuration"
    );

    // -------------------------------------------------------
    // 0c. Metrics (doc 14 section 2)
    // -------------------------------------------------------
    let metrics = Arc::new(Metrics::new());

    if cfg.observability.metrics_enabled {
        let metrics_server = MetricsServer::new(
            MetricsConfig { listen_addr: cfg.observability.metrics_listen_addr.clone() },
            metrics.clone(),
        );
        tokio::spawn(async move {
            if let Err(e) = metrics_server.run().await {
                error!(error = %e, "Metrics server error");
            }
        });
        info!(addr = %cfg.observability.metrics_listen_addr, "Starting metrics exporter");
    }

    // -------------------------------------------------------
    // 1. Channels: consensus <-> router (crossbeam, bounded)
    // -------------------------------------------------------
    let (tx_consensus_ev, rx_consensus_ev) = bounded::<ConsensusEvent>(256);
    let (tx_consensus_cmd, rx_consensus_cmd) = bounded::<ConsensusCommand>(1024);

    // -------------------------------------------------------
    // 2. Keys + identity
    // -------------------------------------------------------
    let key_path = format!("{}/node_key.json", cfg.node.data_dir);
    let (signing_key, verify_key) = load_or_generate_keypair(&key_path)?;
    let my_id = ValidatorId(verify_key.to_bytes());

    // -------------------------------------------------------
    // 3. Bootstrap validator set (single-node MVP)
    // -------------------------------------------------------
    let validator = Validator {
        id: my_id,
        voting_power: 1,
    };
    let set_hash = sha256(&my_id.0);
    let validator_set = ValidatorSet::new(vec![validator], set_hash);

    // -------------------------------------------------------
    // 4. Storage (RocksDB)
    // -------------------------------------------------------
    let blocks_path = format!("{}/blocks", cfg.node.data_dir);
    let state_path = format!("{}/state", cfg.node.data_dir);
    let block_store = Arc::new(BlockStore::open(Path::new(&blocks_path))?);
    let state_store = Arc::new(StateStore::open(Path::new(&state_path))?);

    let last_height = block_store.last_height()?;
    let starting_height = if last_height > 0 { last_height + 1 } else { 1 };

    info!(engine = "rocksdb", data_dir = %cfg.node.data_dir, "Initializing storage");
    info!(height = last_height, "Loading last committed state");

    // -------------------------------------------------------
    // 5. App state + contract runtime
    // -------------------------------------------------------
    let app_state = match state_store.load_latest_state()? {
        Some((_h, state)) => state,
        None => AppState::new(ChainParams::default()),
    };
    let app_state = Arc::new(RwLock::new(app_state));
    let contracts = Arc::new(tokio::sync::Mutex::new(ContractRuntime::new()?));
    let validator_set_shared = Arc::new(RwLock::new(validator_set.clone()));

    info!(validators = validator_set.len(), total_power = validator_set.total_power(), "Loading validator set");

    // -------------------------------------------------------
    // 6. Timer service (Tokio async)
    // -------------------------------------------------------
    let (timer_svc, timer_handle) = TimerService::new(tx_consensus_ev.clone());
    tokio::spawn(async move {
        timer_svc.run().await;
    });

    // -------------------------------------------------------
    // 7. P2P config + manager
    // -------------------------------------------------------
    let p2p_cfg = P2pConfig {
        listen_addr: cfg.p2p.listen_addr.clone(),
        seeds: cfg.p2p.seeds.clone(),
        ..Default::default()
    };

    let hs = HandshakeDeps {
        my_id,
        my_signing: signing_key,
        my_verify: verify_key,
        chain_id: cfg.node.chain_id.clone(),
        protocol_version: 1,
    };

    let verify_proposal: Arc<dyn Fn(&SignedProposal) -> bool + Send + Sync> =
        Arc::new(|_p| true);
    let verify_vote: Arc<dyn Fn(&SignedVote) -> bool + Send + Sync> =
        Arc::new(|_v| true);

    let (p2p, p2p_handle) = P2pManager::new(
        p2p_cfg,
        hs,
        verify_proposal,
        verify_vote,
        tx_consensus_ev.clone(),
    );

    info!(addr = %cfg.p2p.listen_addr, seeds = cfg.p2p.seeds.len(), "Starting P2P listener");

    // -------------------------------------------------------
    // 8. Command router (bridges consensus -> async subsystems)
    // -------------------------------------------------------
    let router = CommandRouter::new(
        rx_consensus_cmd,
        tx_consensus_ev.clone(),
        p2p_handle.tx_cmd.clone(),
        timer_handle.tx.clone(),
        StateExecutor::new(),
        app_state.clone(),
        contracts.clone(),
        block_store.clone(),
        state_store.clone(),
        validator_set_shared.clone(),
    );

    tokio::spawn(async move {
        router.run().await;
    });

    // -------------------------------------------------------
    // 9. Consensus core (sync, dedicated OS thread)
    // -------------------------------------------------------
    let consensus_cfg = ConsensusConfig {
        base_timeout_propose_ms: cfg.consensus.timeout_propose_ms,
        base_timeout_prevote_ms: cfg.consensus.timeout_prevote_ms,
        base_timeout_precommit_ms: cfg.consensus.timeout_precommit_ms,
        timeout_delta_ms: cfg.consensus.timeout_delta_ms,
        allow_empty_blocks: cfg.consensus.create_empty_blocks,
        ..Default::default()
    };

    {
        let my_id_c = my_id;
        let vs = validator_set.clone();
        std::thread::spawn(move || {
            let deps = ConsensusDeps {
                verify_proposal_sig: Box::new(|_| true),
                verify_vote_sig: Box::new(|_| true),
                validate_block: Box::new(|_| true),
                have_full_block: Box::new(|_| true),
                block_hash: Box::new(|block| {
                    let bytes = serde_json::to_vec(block).unwrap_or_default();
                    sha256(&bytes)
                }),
            };
            let core = ConsensusCore::new(
                consensus_cfg,
                deps,
                my_id_c,
                starting_height,
                vs,
                rx_consensus_ev,
                tx_consensus_cmd,
            );
            core.run();
        });
    }

    // -------------------------------------------------------
    // 10. RPC server (Tokio async)
    // -------------------------------------------------------
    let (tx_submit, _rx_submit) = mpsc::channel::<Vec<u8>>(1024);

    let rpc_state = Arc::new(RpcState {
        block_store: block_store.clone(),
        app_state: app_state.clone(),
        validator_set: validator_set_shared.clone(),
        node_id: hex_encode(&my_id.0[..8]),
        chain_id: cfg.node.chain_id.clone(),
        tx_submit,
    });

    let rpc_server = RpcServer::new(
        RpcConfig { listen_addr: cfg.rpc.listen_addr.clone(), ..Default::default() },
        rpc_state,
    );
    info!(addr = %cfg.rpc.listen_addr, "Starting RPC server");
    tokio::spawn(async move {
        if let Err(e) = rpc_server.run().await {
            error!(error = %e, "RPC server error");
        }
    });

    info!(
        height = starting_height,
        node_id = %cfg.node.node_id,
        "Node started"
    );

    // -------------------------------------------------------
    // 11. Graceful shutdown (doc 16 section 1.3)
    // -------------------------------------------------------
    let shutdown = async {
        tokio::signal::ctrl_c().await.ok();
        info!("Shutdown initiated");
    };

    tokio::select! {
        result = p2p.run() => {
            if let Err(e) = result {
                error!(error = %e, "P2P error");
            }
        }
        _ = shutdown => {
            info!("Draining connections and flushing storage");
        }
    }

    info!("Node stopped");
    Ok(())
}

/// Initialize structured logging (doc 14 section 3.5).
fn init_logging(cfg: &RustBFT::config::LoggingSection) {
    use tracing_subscriber::EnvFilter;

    let env_filter = if let Some(ref module_levels) = cfg.module_levels {
        EnvFilter::try_new(module_levels).unwrap_or_else(|_| EnvFilter::new(&cfg.level))
    } else {
        EnvFilter::try_from_env("RUST_LOG").unwrap_or_else(|_| EnvFilter::new(&cfg.level))
    };

    if cfg.format == "json" {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(env_filter)
            .with_target(true)
            .with_thread_ids(false)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_target(true)
            .init();
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}
