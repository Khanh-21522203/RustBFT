# Feature: P2P Networking Layer

## 1. Purpose

The P2P Networking Layer is the untrusted boundary between the internet and the consensus core. It manages authenticated TCP connections between validator nodes, frames and encrypts messages on the wire, validates all signatures before forwarding to consensus, and implements flood gossip for votes and proposals.

Every byte that arrives from the network is adversarial until proven otherwise. The networking layer's job is to perform that proof — verifying framing, message size, and Ed25519 signatures — so the consensus core receives only messages it can trust without performing its own I/O.

## 2. Responsibilities

- Accept inbound TCP connections and establish outbound connections to configured seed peers
- Perform handshake: exchange Ed25519 node identities, sign a challenge, derive a session encryption key via X25519
- Encrypt all post-handshake traffic with ChaCha20-Poly1305
- Frame messages as `[4-byte BE length][1-byte type][variable payload]`
- Enforce maximum message size (4 MB by default); disconnect peers that exceed it
- Maintain peer lifecycle state machine: `Idle → Connecting → Connected → Disconnecting → Disconnected`
- Reconnect persistent peers (validators) with exponential backoff
- Score peers; disconnect and temporarily ban at score ≤ 0
- Implement flood gossip with LRU deduplication for votes, proposals, and transactions
- Enforce bounded inbound and outbound buffers; apply backpressure to slow peers
- Forward validated proposals and votes to the consensus core via a bounded crossbeam channel
- Forward transactions to the mempool via channel

## 3. Non-Responsibilities

- Does not mutate consensus state
- Does not execute transactions or contracts
- Does not perform peer exchange (PEX) — seed list is static for MVP
- Does not implement block sync (separate future feature)
- Does not interpret message semantics beyond signature validation

## 4. Architecture Design

```
Internet / Peers
       |
  TCP sockets
       |
+------v------------------------------------------+
|  Tokio Runtime                                   |
|                                                  |
|  TcpListener     PeerConn (read)  PeerConn (write)|
|  (accept loop)   per peer         per peer        |
|       |               |                |          |
|       +-------+-------+----------------+          |
|               |                                  |
|         PeerManager (Tokio task)                 |
|         - lifecycle, scoring, reconnect          |
|               |                                  |
|         OutboundDispatcher (Tokio task)          |
|         - routes ConsensusCommand to peers       |
+---------------+---------------------------------+
                |
         crossbeam bounded mpsc (cap=256)
                |
+---------------v---------------------------------+
|         Consensus Core Thread (sync)            |
+------------------------------------------------+
```

## 5. Core Data Structures (Rust)

```rust
// src/p2p/peer.rs

pub enum PeerState {
    Idle,
    Connecting,
    Connected { since: std::time::Instant },
    Disconnecting,
    Disconnected,
}

pub struct Peer {
    pub id: ValidatorId,
    pub remote_addr: SocketAddr,
    pub state: PeerState,
    pub score: i32,              // starts at 100; disconnect at ≤ 0
    pub is_persistent: bool,     // validators are always reconnected
    pub reconnect_attempts: u32,
    pub last_seen: std::time::Instant,
    outbound_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
}

// src/p2p/transport.rs

pub struct Handshake {
    pub node_id: ValidatorId,
    pub public_key: Ed25519PublicKey,
    pub ephemeral_key: X25519PublicKey,
    pub chain_id: String,
    pub protocol_version: u16,
    pub nonce: [u8; 32],
    pub signature: Signature,  // sign(private_key, sha256(ephemeral_key || nonce || chain_id))
}

#[repr(u8)]
pub enum MsgType {
    Handshake    = 0x01,
    HandshakeAck = 0x02,
    Proposal     = 0x03,
    Prevote      = 0x04,
    Precommit    = 0x05,
    Transaction  = 0x06,
    Evidence     = 0x07,
    BlockRequest = 0x08,
    BlockResponse= 0x09,
    Ping         = 0x0A,
    Pong         = 0x0B,
    Disconnect   = 0x0C,
}

// src/p2p/gossip.rs

pub struct SeenSet {
    cache: lru::LruCache<Hash, ()>,  // capacity: 10_000
}

// Score penalty table
pub const PENALTY_INVALID_FORMAT: i32 = -10;
pub const PENALTY_INVALID_SIGNATURE: i32 = -50;
pub const PENALTY_MESSAGE_TOO_LARGE: i32 = -20;
pub const PENALTY_NO_PONG: i32 = -5;
pub const PENALTY_EQUIVOCATION: i32 = -100;

// Events sent by peer read tasks to the PeerManager
pub enum PeerManagerEvent {
    InboundConnection { socket: TcpStream, remote_addr: SocketAddr },
    ConnectTo { addr: SocketAddr, node_id: ValidatorId },
    PeerDisconnected { node_id: ValidatorId, reason: DisconnectReason },
    PeerScorePenalty { node_id: ValidatorId, delta: i32 },
    ReconnectTimer { node_id: ValidatorId },
}

// Commands sent from node binary to P2P layer
pub enum P2PCommand {
    BroadcastProposal { proposal: SignedProposal },
    BroadcastVote { vote: SignedVote },
    BroadcastTransaction { tx: SignedTransaction },
    SendToPeer { node_id: ValidatorId, msg: Vec<u8> },
}
```

## 6. Public Interfaces

```rust
// src/p2p/mod.rs

pub struct P2PConfig {
    pub listen_addr: SocketAddr,
    pub seeds: Vec<(ValidatorId, SocketAddr)>,
    pub chain_id: String,
    pub node_id: ValidatorId,
    pub private_key: Ed25519PrivateKey,
    pub max_peers: usize,
    pub max_message_size_bytes: usize,     // default: 4 MB
    pub inbound_buffer_size: usize,        // default: 64
    pub outbound_buffer_size: usize,       // default: 64
    pub consensus_channel_size: usize,     // default: 256
    pub handshake_timeout_ms: u64,         // default: 5000
    pub ping_interval_ms: u64,             // default: 10_000
    pub pong_timeout_ms: u64,              // default: 5000
    pub reconnect_backoff_max_ms: u64,     // default: 60_000
    pub peer_ban_duration_ms: u64,         // default: 300_000
}

/// Start the P2P layer. Returns:
/// - a Receiver for consensus events (proposals and votes validated and ready to process)
/// - a Sender for P2P commands (broadcast, send-to-peer)
pub async fn start(
    config: P2PConfig,
    mempool_tx: tokio::sync::mpsc::Sender<MempoolCommand>,
) -> (
    crossbeam::channel::Receiver<ConsensusEvent>,
    tokio::sync::mpsc::Sender<P2PCommand>,
);
```

## 7. Internal Algorithms

### Handshake Protocol
```
Initiator:
    1. Send Handshake{node_id, pubkey, ephemeral_pk, chain_id, proto_ver, nonce, sig}
    2. Receive Handshake from responder
    3. Verify responder's signature: verify(responder.pubkey, sha256(responder.ephemeral_key || responder.nonce || chain_id), responder.sig)
    4. Reject if chain_id mismatch or proto_ver incompatible
    5. Derive shared secret: X25519(own_ephemeral_sk, responder.ephemeral_pk)
    6. Derive session keys: HKDF(shared_secret, "rustbft-v1", 64 bytes) → [send_key(32), recv_key(32)]
    7. Send HandshakeAck
    8. All subsequent messages encrypted with ChaCha20-Poly1305
```

### Message Read Loop (per peer)
```
async fn read_loop(reader, decrypt_key, inbound_tx):
    loop:
        length = read_u32_be(reader)        // 4 bytes
        if length > MAX_MESSAGE_SIZE:
            penalize_peer(PENALTY_MESSAGE_TOO_LARGE)
            return  // disconnect
        buf = read_exact(reader, length)    // 1 + payload bytes
        buf = chacha20_poly1305_decrypt(decrypt_key, buf)
        msg_type = buf[0]
        payload = buf[1..]
        if inbound_tx.try_send((msg_type, payload)).is_err():
            // channel full — TCP backpressure will kick in naturally
            warn!("inbound buffer full for peer")
```

### Inbound Message Routing
```
fn route(msg_type, payload, from_peer_id, consensus_tx, mempool_tx, peer_manager_tx):
    match msg_type:
        Proposal | Prevote | Precommit:
            msg = canonical_decode(payload)?   else penalize(INVALID_FORMAT)
            if !verify_signature(msg):          penalize(INVALID_SIGNATURE); return
            if gossip_seen_set.should_forward(sha256(payload)):
                broadcast_to_peers(payload, exclude=from_peer_id)
            consensus_tx.try_send(ConsensusEvent::from(msg))

        Transaction:
            tx = canonical_decode(payload)?    else penalize(INVALID_FORMAT)
            // P2P gossip dedup handled by seen_set
            if gossip_seen_set.should_forward(sha256(payload)):
                mempool_tx.try_send(MempoolCommand::AddTx{tx, source: P2p})
                broadcast_to_peers(payload, exclude=from_peer_id)

        Evidence:
            // record equivocation evidence
        Ping: send_pong(from_peer_id)
        Pong: update_last_seen(from_peer_id)
        Disconnect: close connection gracefully
```

### Gossip Deduplication
```
fn should_forward(seen: &mut SeenSet, msg_bytes: &[u8]) -> bool:
    h = sha256(msg_bytes)
    if seen.cache.contains(&h): return false
    seen.cache.put(h, ())
    true
```

### Reconnection Backoff
```
fn reconnect_delay(attempts: u32) -> Duration:
    secs = min(1u64 << attempts, 60)   // 1s, 2s, 4s, ..., 60s cap
    Duration::from_secs(secs)
```

### Peer Ban
```
on peer.score <= 0:
    disconnect peer
    insert (peer.id, Instant::now() + ban_duration) into ban_list
    if is_persistent: schedule reconnect after ban expires
```

## 8. Persistence Model

No persistence. Peer state is in-memory and rebuilt from config on restart. The seed list is static configuration. Ban lists are in-memory and reset on restart.

## 9. Concurrency Model

All P2P code runs inside the Tokio async runtime. Per-peer tasks:
- One `read_loop` async task reading frames from the TCP socket
- One `write_loop` async task draining the outbound mpsc channel and writing to the socket

The `PeerManager` is a single Tokio task that owns all `Peer` structs (no sharing). The `OutboundDispatcher` receives `P2PCommand`s from the node binary and fans them out to the appropriate peer `write_loop` tasks.

The bridge to the synchronous consensus core uses `crossbeam::channel::bounded` (not Tokio channels), because the consensus core uses blocking `recv()`.

## 10. Configuration

```toml
[p2p]
listen_addr = "0.0.0.0:26656"
seeds = ["node1@172.28.1.2:26656", "node2@172.28.1.3:26656"]
max_peers = 50
handshake_timeout_ms = 5000
ping_interval_ms = 10000
pong_timeout_ms = 5000
max_message_size_bytes = 4194304
inbound_buffer_size = 64
outbound_buffer_size = 64
reconnect_backoff_max_ms = 60000
peer_ban_duration_ms = 300000
```

## 11. Observability

- `rustbft_p2p_peers_connected` (Gauge) — current peer count
- `rustbft_p2p_messages_sent_total{msg_type}` (Counter)
- `rustbft_p2p_messages_received_total{msg_type}` (Counter)
- `rustbft_p2p_bytes_sent_total` / `bytes_received_total` (Counter)
- `rustbft_p2p_peer_score{peer_id}` (Gauge)
- `rustbft_p2p_connection_errors_total{error_type}` (Counter)
- Log INFO on peer connect/disconnect; WARN on invalid message or buffer full; ERROR on repeated handshake failures

## 12. Testing Strategy

- **`test_message_framing_round_trip`**: encode a message, decode it, assert equality
- **`test_handshake_valid`**: two in-process nodes connect over loopback, handshake succeeds, both mark peer as `Connected`
- **`test_handshake_wrong_chain_id`**: mismatched chain_id → connection rejected
- **`test_invalid_signature_penalizes_peer`**: send a Proposal with corrupted signature → peer score -50
- **`test_message_too_large_disconnects`**: send frame with length > 4 MB → peer disconnected
- **`test_gossip_deduplication`**: same vote bytes sent twice → forwarded only once to consensus
- **`test_peer_ban`**: drive score to 0 via penalties → peer disconnected and banned
- **`test_reconnect_backoff`**: disconnect persistent peer; verify reconnect attempts use exponential delay
- **`test_backpressure_inbound`**: fill the inbound buffer → TCP read stops (socket read pauses)
- **`test_consensus_channel_full_drops_message`**: fill the crossbeam channel to consensus → message dropped, metric incremented

## 13. Open Questions

- **Encryption MVP scope**: Should ChaCha20-Poly1305 be in the initial implementation or deferred behind a feature flag? The handshake is defined in full but encryption adds implementation complexity. Decision: implement authenticated-only first, add encryption in production hardening phase.
