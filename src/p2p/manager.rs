use crate::p2p::codec::{PlainFramer, NetCodecError};
use crate::p2p::config::P2pConfig;
use crate::p2p::gossip::Seen;
use crate::p2p::msg::{decode_message, encode_message, NetworkMessage};
use crate::p2p::peer::{handshake, HandshakeDeps, PeerRole};

use crate::consensus::{ConsensusCommand, ConsensusEvent};
use crate::types::{ValidatorId, SignedProposal, SignedVote, VoteType};

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use std::collections::BTreeMap;
use std::sync::Arc;
use chacha20poly1305::aead::Aead;
use chacha20poly1305::Nonce;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
pub struct P2pHandle {
    pub tx_cmd: mpsc::Sender<ConsensusCommand>, // router -> p2p
}

enum Control {
    PeerConnected { peer_id: ValidatorId, tx_out: mpsc::Sender<NetworkMessage> },
    PeerDisconnected { peer_id: ValidatorId },
}

pub struct P2pManager {
    cfg: P2pConfig,
    hs: HandshakeDeps,

    verify_proposal: Arc<dyn Fn(&SignedProposal) -> bool + Send + Sync>,
    verify_vote: Arc<dyn Fn(&SignedVote) -> bool + Send + Sync>,

    to_consensus: crossbeam_channel::Sender<ConsensusEvent>,

    rx_cmd: mpsc::Receiver<ConsensusCommand>,

    peers: BTreeMap<ValidatorId, mpsc::Sender<NetworkMessage>>,

    tx_gossip: mpsc::Sender<(ValidatorId, NetworkMessage)>,
    rx_gossip: mpsc::Receiver<(ValidatorId, NetworkMessage)>,

    tx_ctl: mpsc::Sender<Control>,
    rx_ctl: mpsc::Receiver<Control>,
}

impl P2pManager {
    pub fn new(
        cfg: P2pConfig,
        hs: HandshakeDeps,
        verify_proposal: Arc<dyn Fn(&SignedProposal) -> bool + Send + Sync>,
        verify_vote: Arc<dyn Fn(&SignedVote) -> bool + Send + Sync>,
        to_consensus: crossbeam_channel::Sender<ConsensusEvent>,
    ) -> (Self, P2pHandle) {
        let (tx_cmd, rx_cmd) = mpsc::channel(1024);
        let (tx_gossip, rx_gossip) = mpsc::channel(2048);
        let (tx_ctl, rx_ctl) = mpsc::channel(256);

        (
            Self {
                cfg,
                hs,
                verify_proposal,
                verify_vote,
                to_consensus,
                rx_cmd,
                peers: BTreeMap::new(),
                tx_gossip,
                rx_gossip,
                tx_ctl,
                rx_ctl,
            },
            P2pHandle { tx_cmd },
        )
    }

    pub async fn run(mut self) -> anyhow::Result<()> {
        // listener
        let listener = TcpListener::bind(&self.cfg.listen_addr).await?;

        // accept loop
        {
            let cfg = self.cfg.clone();
            let hs = self.hs.clone();
            let tx_ctl = self.tx_ctl.clone();
            let tx_gossip = self.tx_gossip.clone();
            let verify_proposal = self.verify_proposal.clone();
            let verify_vote = self.verify_vote.clone();
            let to_consensus = self.to_consensus.clone();

            tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok((stream, _)) => {
                            let _ = spawn_peer(
                                PeerRole::Inbound,
                                stream,
                                cfg.clone(),
                                hs.clone(),
                                tx_ctl.clone(),
                                tx_gossip.clone(),
                                verify_proposal.clone(),
                                verify_vote.clone(),
                                to_consensus.clone(),
                            ).await;
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        // connect seeds (MVP: connect once; reconnect/backoff sẽ làm sau)
        for seed in self.cfg.seeds.clone() {
            if let Some(addr) = parse_seed_addr(&seed) {
                let cfg = self.cfg.clone();
                let hs = self.hs.clone();
                let tx_ctl = self.tx_ctl.clone();
                let tx_gossip = self.tx_gossip.clone();
                let verify_proposal = self.verify_proposal.clone();
                let verify_vote = self.verify_vote.clone();
                let to_consensus = self.to_consensus.clone();
                tokio::spawn(async move {
                    if let Ok(stream) = TcpStream::connect(addr).await {
                        let _ = spawn_peer(
                            PeerRole::Outbound,
                            stream,
                            cfg,
                            hs,
                            tx_ctl,
                            tx_gossip,
                            verify_proposal,
                            verify_vote,
                            to_consensus,
                        ).await;
                    }
                });
            }
        }

        let mut seen = Seen::new(10_000);

        loop {
            tokio::select! {
                Some(ctl) = self.rx_ctl.recv() => {
                    match ctl {
                        Control::PeerConnected { peer_id, tx_out } => { self.peers.insert(peer_id, tx_out); }
                        Control::PeerDisconnected { peer_id } => { self.peers.remove(&peer_id); }
                    }
                }
                Some(cmd) = self.rx_cmd.recv() => {
                    self.handle_consensus_cmd(cmd, &mut seen).await;
                }
                Some((sender, msg)) = self.rx_gossip.recv() => {
                    self.fanout(sender, msg, &mut seen).await;
                }
                else => break,
            }
        }

        Ok(())
    }

    async fn handle_consensus_cmd(&mut self, cmd: ConsensusCommand, seen: &mut Seen) {
        match cmd {
            ConsensusCommand::BroadcastProposal { proposal } => {
                self.fanout(self.hs.my_id, NetworkMessage::Proposal(proposal), seen).await;
            }
            ConsensusCommand::BroadcastVote { vote } => {
                let msg = match vote.vote.vote_type {
                    VoteType::Prevote => NetworkMessage::Prevote(vote),
                    VoteType::Precommit => NetworkMessage::Precommit(vote),
                };
                self.fanout(self.hs.my_id, msg, seen).await;
            }
            _ => {}
        }
    }

    async fn fanout(&mut self, sender: ValidatorId, msg: NetworkMessage, seen: &mut Seen) {
        let (ty, payload) = encode_message(&msg);

        // dedup hash over [ty||payload]
        let mut raw = Vec::with_capacity(1 + payload.len());
        raw.push(ty as u8);
        raw.extend_from_slice(&payload);
        if !seen.check_and_mark(&raw) { return; }

        let critical = matches!(msg, NetworkMessage::Proposal(_) | NetworkMessage::Prevote(_) | NetworkMessage::Precommit(_));

        for (peer_id, tx) in self.peers.iter() {
            if *peer_id == sender { continue; }
            if critical {
                let _ = tx.send(msg.clone()).await;
            } else {
                let _ = tx.try_send(msg.clone());
            }
        }
    }
}

async fn spawn_peer(
    role: PeerRole,
    stream: TcpStream,
    cfg: P2pConfig,
    hs: HandshakeDeps,
    tx_ctl: mpsc::Sender<Control>,
    tx_gossip: mpsc::Sender<(ValidatorId, NetworkMessage)>,
    verify_proposal: Arc<dyn Fn(&SignedProposal) -> bool + Send + Sync>,
    verify_vote: Arc<dyn Fn(&SignedVote) -> bool + Send + Sync>,
    to_consensus: crossbeam_channel::Sender<ConsensusEvent>,
) -> Result<(), NetCodecError> {
    // Perform plaintext handshake first, then upgrade to encrypted framer
    let plain = PlainFramer::new(stream, cfg.max_message_size_bytes);
    let (peer_id, enc) = handshake(role, plain, hs).await?;

    // Register peer's outbound queue in manager
    let (tx_out, mut rx_out) = mpsc::channel::<NetworkMessage>(cfg.outbound_buffer_size);
    let _ = tx_ctl
        .send(Control::PeerConnected {
            peer_id,
            tx_out: tx_out.clone(),
        })
        .await;

    // Split stream into owned read/write halves (Tokio TcpStream is not clonable)
    let max_size = enc.max_size;
    let (mut read_half, mut write_half) = enc.stream.into_split();

    // Split crypto state: writer uses send_ctr/dir_send, reader uses recv_ctr/dir_recv
    let mut crypto_w = enc.crypto.clone();
    let mut crypto_r = enc.crypto;

    // Local helper to build AEAD nonce (12 bytes): [dir (1)] + [ctr (8)] + [0,0,0]
    fn make_nonce(dir: u8, ctr: u64) -> Nonce {
        let mut n = [0u8; 12];
        n[0] = dir;
        n[1..9].copy_from_slice(&ctr.to_be_bytes());
        Nonce::from_slice(&n).to_owned()
    }

    // Writer task: drains outbound queue and writes encrypted frames
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx_out.recv().await {
            let (ty, payload) = encode_message(&msg);

            // Plaintext = [MsgType(1)] + payload
            let mut pt = Vec::with_capacity(1 + payload.len());
            pt.push(ty as u8);
            pt.extend_from_slice(&payload);

            if pt.len() > max_size {
                // Too large: drop
                continue;
            }

            let nonce = make_nonce(crypto_w.dir_send, crypto_w.send_ctr);
            crypto_w.send_ctr = crypto_w.send_ctr.wrapping_add(1);

            let ct = match crypto_w.aead.encrypt(&nonce, pt.as_ref()) {
                Ok(c) => c,
                Err(_) => {
                    // AEAD failure: drop message
                    continue;
                }
            };

            if ct.len() > max_size {
                continue;
            }

            // Frame = len(4 BE) + ciphertext
            let mut frame = Vec::with_capacity(4 + ct.len());
            frame.extend_from_slice(&(ct.len() as u32).to_be_bytes());
            frame.extend_from_slice(&ct);

            if write_half.write_all(&frame).await.is_err() {
                break;
            }
        }
    });

    // Reader task: reads encrypted frames, decrypts, validates, forwards to consensus, gossips
    let reader = tokio::spawn(async move {
        let mut seen = Seen::new(10_000);

        loop {
            // Read length prefix
            let mut len_buf = [0u8; 4];
            if read_half.read_exact(&mut len_buf).await.is_err() {
                break;
            }
            let len = u32::from_be_bytes(len_buf) as usize;

            if len > max_size {
                // Protocol violation: message too large
                break;
            }

            // Read ciphertext
            let mut ct = vec![0u8; len];
            if read_half.read_exact(&mut ct).await.is_err() {
                break;
            }

            // Decrypt
            let nonce = make_nonce(crypto_r.dir_recv, crypto_r.recv_ctr);
            crypto_r.recv_ctr = crypto_r.recv_ctr.wrapping_add(1);

            let pt = match crypto_r.aead.decrypt(&nonce, ct.as_ref()) {
                Ok(p) => p,
                Err(_) => {
                    // Invalid ciphertext/tag: drop
                    continue;
                }
            };

            if pt.is_empty() {
                continue;
            }

            // Dedup using hash over plaintext bytes (MsgType + payload)
            if !seen.check_and_mark(&pt) {
                continue;
            }

            // Parse MsgType + payload
            let ty = match crate::p2p::msg::MsgType::from_u8(pt[0]) {
                Some(t) => t,
                None => continue,
            };
            let payload = &pt[1..];

            let msg = match decode_message(ty, payload) {
                Ok(m) => m,
                Err(_) => continue,
            };

            // Verify signature before forwarding to consensus
            match &msg {
                NetworkMessage::Proposal(sp) => {
                    if !(verify_proposal)(sp) {
                        continue;
                    }
                    // Protect consensus core: bounded channel; drop if full
                    let _ = to_consensus.try_send(ConsensusEvent::ProposalReceived {
                        proposal: sp.clone(),
                    });
                }
                NetworkMessage::Prevote(v) | NetworkMessage::Precommit(v) => {
                    if !(verify_vote)(v) {
                        continue;
                    }
                    let _ = to_consensus.try_send(ConsensusEvent::VoteReceived { vote: v.clone() });
                }
                _ => {}
            }

            // Gossip to manager fanout (excluding sender handled by manager)
            let _ = tx_gossip.send((peer_id, msg)).await;
        }
    });

    // Wait for either task to finish, then disconnect
    let _ = tokio::join!(writer, reader);

    let _ = tx_ctl
        .send(Control::PeerDisconnected { peer_id })
        .await;

    Ok(())
}

fn parse_seed_addr(seed: &str) -> Option<String> {
    seed.split('@').nth(1).map(|s| s.to_string())
}
