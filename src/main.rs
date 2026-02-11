use std::path::Path;
use std::sync::Arc;

use crossbeam_channel::bounded;
use tokio::sync::{mpsc, RwLock};

use RustBFT::consensus::{
    CommandRouter, ConsensusCommand, ConsensusConfig, ConsensusCore, ConsensusDeps,
    ConsensusEvent, TimerService,
};
use RustBFT::contracts::ContractRuntime;
use RustBFT::crypto::ed25519::load_or_generate_keypair;
use RustBFT::crypto::hash::sha256;
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
    // 1. Channels: consensus <-> router (crossbeam, bounded)
    // -------------------------------------------------------
    let (tx_consensus_ev, rx_consensus_ev) = bounded::<ConsensusEvent>(256);
    let (tx_consensus_cmd, rx_consensus_cmd) = bounded::<ConsensusCommand>(1024);

    // -------------------------------------------------------
    // 2. Keys + identity
    // -------------------------------------------------------
    let (signing_key, verify_key) = load_or_generate_keypair("node_key.json")?;
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
    let block_store = Arc::new(BlockStore::open(Path::new("data/blocks"))?);
    let state_store = Arc::new(StateStore::open(Path::new("data/state"))?);

    // Determine starting height from persisted data
    let last_height = block_store.last_height()?;
    let starting_height = if last_height > 0 { last_height + 1 } else { 1 };

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
        listen_addr: "0.0.0.0:26656".to_string(),
        seeds: vec![],
        ..Default::default()
    };

    let hs = HandshakeDeps {
        my_id,
        my_signing: signing_key,
        my_verify: verify_key,
        chain_id: "localnet".to_string(),
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

    // Router runs on a dedicated thread (blocks on crossbeam recv)
    tokio::spawn(async move {
        router.run().await;
    });

    // -------------------------------------------------------
    // 9. Consensus core (sync, dedicated OS thread)
    // -------------------------------------------------------
    {
        let my_id_c = my_id;
        let vs = validator_set.clone();
        std::thread::spawn(move || {
            let cfg = ConsensusConfig::default();
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
                cfg,
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
        node_id: hex::encode(&my_id.0[..8]),
        chain_id: "localnet".to_string(),
        tx_submit,
    });

    let rpc_server = RpcServer::new(RpcConfig::default(), rpc_state);
    tokio::spawn(async move {
        if let Err(e) = rpc_server.run().await {
            eprintln!("RPC server error: {}", e);
        }
    });

    // -------------------------------------------------------
    // 11. Run P2P (blocks forever)
    // -------------------------------------------------------
    p2p.run().await?;
    Ok(())
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
}
