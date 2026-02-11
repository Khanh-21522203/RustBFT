use std::sync::Arc;

use crossbeam_channel::bounded;

use RustBFT::consensus::{ConsensusCommand, ConsensusConfig, ConsensusCore, ConsensusDeps, ConsensusEvent};
use RustBFT::crypto::ed25519::load_or_generate_keypair;
use RustBFT::crypto::hash::sha256;
use RustBFT::p2p::peer::HandshakeDeps;
use RustBFT::p2p::{P2pConfig, P2pManager};
use RustBFT::types::{SignedProposal, SignedVote, Validator, ValidatorId, ValidatorSet};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1) Channels: net -> consensus (bounded 256), consensus -> router (bounded 1024)
    let (tx_consensus_ev, rx_consensus_ev) = bounded::<ConsensusEvent>(256);
    let (tx_consensus_cmd, rx_consensus_cmd) = bounded::<ConsensusCommand>(1024);

    // 2) P2P config
    let p2p_cfg = P2pConfig {
        listen_addr: "0.0.0.0:26656".to_string(),
        seeds: vec![],
        ..Default::default()
    };

    // 3) Keys + identity
    let (signing_key, verify_key) = load_or_generate_keypair("node_key.json")?;
    let my_id = ValidatorId(verify_key.to_bytes());

    let hs = HandshakeDeps {
        my_id,
        my_signing: signing_key,
        my_verify: verify_key,
        chain_id: "localnet".to_string(),
        protocol_version: 1,
    };

    // 4) Verify hooks (MVP: pass-through; replace with real sig verification later)
    let verify_proposal: Arc<dyn Fn(&SignedProposal) -> bool + Send + Sync> =
        Arc::new(|_p| true);
    let verify_vote: Arc<dyn Fn(&SignedVote) -> bool + Send + Sync> =
        Arc::new(|_v| true);

    // 5) Bootstrap validator set (single-node MVP)
    let validator = Validator {
        id: my_id,
        voting_power: 1,
    };
    let set_hash = sha256(&my_id.0);
    let validator_set = ValidatorSet::new(vec![validator], set_hash);

    // 6) Consensus dependencies (deterministic callbacks)
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

    // 7) Start consensus thread (sync/blocking)
    {
        let my_id_c = my_id;
        let vs = validator_set.clone();
        std::thread::spawn(move || {
            let cfg = ConsensusConfig::default();
            let core = ConsensusCore::new(
                cfg,
                deps,
                my_id_c,
                1, // starting height
                vs,
                rx_consensus_ev,
                tx_consensus_cmd,
            );
            core.run();
        });
    }

    // 8) Start P2P manager (Tokio)
    let (p2p, p2p_handle) = P2pManager::new(
        p2p_cfg,
        hs,
        verify_proposal,
        verify_vote,
        tx_consensus_ev,
    );

    // 9) Router: consensus commands -> p2p dispatcher
    let tx_p2p_cmd = p2p_handle.tx_cmd.clone();
    tokio::spawn(async move {
        while let Ok(cmd) = rx_consensus_cmd.recv() {
            let _ = tx_p2p_cmd.send(cmd).await;
        }
    });

    // 10) Run p2p forever
    p2p.run().await?;
    Ok(())
}
