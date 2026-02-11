use std::sync::Arc;

use crossbeam_channel::bounded;

use RustBFT::p2p::{P2pConfig, P2pManager};
use RustBFT::p2p::peer::HandshakeDeps;

// ===== Bạn map theo code consensus của bạn =====
use RustBFT::consensus::{ConsensusCore, ConsensusConfig, ConsensusEvent, ConsensusCommand};
// ==============================================

use RustBFT::crypto::ed25519::load_or_generate_keypair; // hoặc generate_keypair
use RustBFT::types::{ValidatorId, SignedProposal, SignedVote};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1) net -> consensus bounded channel (docs: capacity 256)
    let (tx_consensus_ev, rx_consensus_ev) = bounded::<ConsensusEvent>(256);

    // 2) consensus -> router commands (capacity tuỳ, 1024 ok)
    let (tx_consensus_cmd, rx_consensus_cmd) = bounded::<ConsensusCommand>(1024);

    // 3) Load config (tạm hardcode để chạy)
    let p2p_cfg = P2pConfig {
        listen_addr: "0.0.0.0:26656".to_string(),
        seeds: vec![], // add "node2@127.0.0.1:26657" etc
        ..Default::default()
    };

    // 4) Keys + identity (ValidatorId)
    // Bạn có thể derive ValidatorId = sha256(pubkey) hoặc config sẵn.
    let (signing_key, verify_key) = load_or_generate_keypair("node_key.json")?;
    let my_id = ValidatorId(verify_key.to_bytes()); // MVP: dùng pubkey bytes làm id luôn

    let hs = HandshakeDeps {
        my_id,
        my_signing: signing_key,
        my_verify: verify_key,
        chain_id: "localnet".to_string(),
        protocol_version: 1,
    };

    // 5) Verify hooks cho networking (P2P phải verify trước khi forward)
    // Ở MVP bạn có thể gọi thẳng crypto verify trong consensus/types.
    // Ở đây để “pass-through” (bạn thay bằng verify thật).
    let verify_proposal: Arc<dyn Fn(&SignedProposal) -> bool + Send + Sync> =
        Arc::new(|_p| true);
    let verify_vote: Arc<dyn Fn(&SignedVote) -> bool + Send + Sync> =
        Arc::new(|_v| true);

    // 6) Start consensus thread (sync/blocking)
    {
        let my_id_for_consensus = my_id;
        std::thread::spawn(move || {
            let cfg = ConsensusConfig::default();

            // Bạn khởi tạo consensus core theo constructor của bạn:
            let mut core = ConsensusCore::new(
                cfg,
                my_id_for_consensus,
                rx_consensus_ev,
                tx_consensus_cmd,
                // + validator_set / deps / etc theo code của bạn
            );

            core.run(); // blocking loop
        });
    }

    // 7) Start P2P manager (Tokio)
    let (p2p, p2p_handle) = P2pManager::new(
        p2p_cfg,
        hs,
        verify_proposal,
        verify_vote,
        tx_consensus_ev, // important
    );

    // 8) Router: consensus commands -> p2p dispatcher
    // Consensus emits BroadcastProposal/BroadcastVote, p2p fanout to peers.
    let tx_p2p_cmd = p2p_handle.tx_cmd.clone();
    tokio::spawn(async move {
        while let Ok(cmd) = rx_consensus_cmd.recv() {
            // If p2p queue full: await sẽ backpressure (ok)
            let _ = tx_p2p_cmd.send(cmd).await;
        }
    });

    // 9) Run p2p forever
    p2p.run().await?;
    Ok(())
}
