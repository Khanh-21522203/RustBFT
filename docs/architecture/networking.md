# RustBFT — Networking Layer

**Purpose:** Define the P2P networking layer: peer management, message framing, authentication, gossip, and backpressure.
**Audience:** Engineers implementing or reviewing the networking subsystem.

---

## 1. Responsibilities

The networking layer is responsible for:

1. **Peer lifecycle** — discover, connect, authenticate, maintain, and disconnect peers.
2. **Message transport** — send and receive length-prefixed, serialized messages over TCP.
3. **Signature validation** — verify message signatures before forwarding to consensus.
4. **Gossip** — broadcast votes and evidence to all connected peers.
5. **Backpressure** — enforce rate limits and bounded buffers to prevent resource exhaustion.

The networking layer MUST NOT:

- Mutate consensus state.
- Execute transactions or contracts.
- Decide commits or validator set changes.
- Interpret message semantics beyond validation.

---

## 2. Transport Protocol

### 2.1 Connection Setup

```
Initiator                              Responder
    │                                      │
    │──── TCP connect ────────────────────▶│
    │                                      │
    │◀─── Handshake(node_id, pubkey, ─────│
    │      chain_id, protocol_version)     │
    │                                      │
    │──── Handshake(node_id, pubkey, ─────▶│
    │      chain_id, protocol_version)     │
    │                                      │
    │◀─── HandshakeAck / Reject ──────────│
    │                                      │
    │  (Authenticated, encrypted session)  │
    │◀────────── Messages ────────────────▶│
```

### 2.2 Authentication

- Each node has an Ed25519 keypair.
- During handshake, nodes exchange public keys and sign a challenge (connection nonce + chain_id).
- The peer's identity is verified against the known validator set or a configured peer allowlist.
- Connections from unknown peers are accepted but rate-limited (for future peer discovery).

### 2.3 Encryption

- After handshake, the session is encrypted using a shared secret derived via X25519 key exchange.
- All subsequent messages are encrypted with ChaCha20-Poly1305 (AEAD).
- This prevents eavesdropping and tampering on the wire.

### 2.4 Message Framing

```
┌──────────────┬──────────────┬─────────────────────┐
│ Length (4 BE) │ MsgType (1)  │ Payload (variable)  │
└──────────────┴──────────────┴─────────────────────┘
```

- **Length:** 4-byte big-endian unsigned integer. Includes MsgType + Payload.
- **MsgType:** 1-byte enum discriminant.
- **Payload:** Canonical serialized message body.
- **Maximum message size:** 4 MB (configurable). Messages exceeding this are rejected and the peer is disconnected.

---

## 3. Message Types

```
MsgType:
    0x01  Handshake
    0x02  HandshakeAck
    0x03  Proposal
    0x04  Prevote
    0x05  Precommit
    0x06  Transaction        (mempool gossip)
    0x07  Evidence           (equivocation proof)
    0x08  BlockRequest       (block sync)
    0x09  BlockResponse      (block sync)
    0x0A  Ping
    0x0B  Pong
    0x0C  Disconnect
```

---

## 4. Peer Management

### 4.1 Peer Discovery

MVP uses a **static seed list** configured in the node's TOML config:

```
[p2p]
seeds = [
    "node1@192.168.1.10:26656",
    "node2@192.168.1.11:26656",
    "node3@192.168.1.12:26656",
]
```

On startup, the node connects to all seeds. Peer exchange (PEX) is out of scope for MVP.

### 4.2 Peer State Machine

```
┌────────┐   connect    ┌────────────┐  handshake ok  ┌───────────┐
│  IDLE  │─────────────▶│ CONNECTING │───────────────▶│ CONNECTED │
└────────┘              └────────────┘                └─────┬─────┘
                              │                             │
                         timeout/error                 error/disconnect
                              │                             │
                              ▼                             ▼
                        ┌────────────┐              ┌──────────────┐
                        │DISCONNECTED│◀─────────────│DISCONNECTING │
                        └─────┬──────┘              └──────────────┘
                              │
                         backoff timer
                              │
                              ▼
                        ┌────────┐
                        │  IDLE  │  (retry)
                        └────────┘
```

### 4.3 Reconnection

- Exponential backoff: 1s, 2s, 4s, 8s, ... up to 60s max.
- Maximum reconnection attempts: configurable (default: unlimited for validators).
- Persistent peers (validators) are always reconnected. Non-persistent peers are dropped after max attempts.

### 4.4 Peer Scoring (MVP: Simple)

Each peer has a score starting at 100. Deductions:

| Event | Penalty |
|-------|---------|
| Invalid message format | -10 |
| Invalid signature | -50 |
| Message too large | -20 |
| Timeout (no pong) | -5 |
| Equivocation evidence | -100 |

Peer is disconnected and banned (configurable duration) when score ≤ 0.

---

## 5. Gossip Protocol

### 5.1 Vote Gossip

Votes (prevotes and precommits) are broadcast to **all connected peers**:

```
On receiving a valid vote V from any source (local or peer):
    IF V is not already in our VoteSet:
        Add V to VoteSet
        Forward V to all connected peers EXCEPT the sender
```

This is simple flood gossip. For MVP with 4-7 validators, this is sufficient. Epidemic gossip optimizations are deferred.

### 5.2 Proposal Dissemination

```
Proposer:
    Send Proposal to all connected peers (direct broadcast)

Non-proposer on receiving Proposal:
    Validate signature
    Forward to all connected peers EXCEPT sender (one hop)
```

### 5.3 Transaction Gossip

```
On receiving a new transaction T (via RPC or peer):
    IF T passes stateless validation:
        Add T to mempool
        Forward T to all connected peers EXCEPT sender
```

### 5.4 Deduplication

All gossip uses a **seen set** (bounded LRU cache keyed by message hash) to prevent infinite rebroadcast.

```
seen_messages: LruCache<Hash, ()>   // capacity: 10,000

on_message(msg):
    hash = sha256(msg)
    IF seen_messages.contains(hash):
        RETURN  // drop duplicate
    seen_messages.insert(hash)
    process(msg)
```

---

## 6. Backpressure

### 6.1 Inbound

- Each peer connection has a **bounded read buffer** (default: 64 messages or 16 MB).
- If the buffer is full, the connection applies TCP backpressure (stops reading from socket).
- If the peer continues sending (buffer stays full for >30s), the peer is disconnected.

### 6.2 Outbound

- Each peer connection has a **bounded write buffer** (default: 64 messages or 16 MB).
- If the outbound buffer is full, messages are dropped (with a metric increment).
- Critical messages (proposals, own votes) are never dropped; they use a priority queue.

### 6.3 Channel to Consensus

- The channel from networking to consensus is a **bounded mpsc** (capacity: 256).
- If the channel is full, incoming messages are dropped and a warning is logged.
- This protects the consensus core from being overwhelmed by network traffic.

---

## 7. Async Architecture

The networking layer runs entirely within the Tokio async runtime:

```
┌─────────────────────────────────────────────────────────┐
│                  Tokio Runtime                           │
│                                                          │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐     │
│  │  Listener   │  │  Peer Conn  │  │  Peer Conn  │     │
│  │  (accept)   │  │  (read/     │  │  (read/     │     │
│  │             │  │   write)    │  │   write)    │     │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘     │
│         │                │                │              │
│         └────────────────┴────────────────┘              │
│                          │                               │
│              ┌───────────▼────────────┐                  │
│              │   Peer Manager Task    │                  │
│              │  (connect, disconnect, │                  │
│              │   score, reconnect)    │                  │
│              └───────────┬────────────┘                  │
│                          │                               │
│              ┌───────────▼────────────┐                  │
│              │  Outbound Dispatcher   │                  │
│              │  (route commands to    │                  │
│              │   correct peer tasks)  │                  │
│              └────────────────────────┘                  │
│                          │                               │
└──────────────────────────┼───────────────────────────────┘
                           │ bounded mpsc
                           ▼
              ┌────────────────────────┐
              │    Consensus Core      │
              │   (sync, blocking)     │
              └────────────────────────┘
```

### Key Async Tasks

| Task | Responsibility | Count |
|------|---------------|-------|
| Listener | Accept incoming TCP connections | 1 |
| PeerConnection (read) | Read frames from a peer, validate, forward | 1 per peer |
| PeerConnection (write) | Write outbound frames to a peer | 1 per peer |
| PeerManager | Track peer state, handle connect/disconnect/reconnect | 1 |
| OutboundDispatcher | Route consensus commands to peer write tasks | 1 |

---

## 8. Serialization

All network messages use **canonical serialization** to ensure deterministic hashing:

- Format: Custom binary encoding (not serde-derived, not JSON).
- Field order: fixed, defined by the protocol spec.
- Integer encoding: big-endian, fixed-width.
- Variable-length fields: length-prefixed (4-byte BE length + data).
- No optional fields on the wire; use sentinel values (e.g., 0-length for absent).

**Rationale:** serde's default serialization formats (bincode, JSON) do not guarantee canonical encoding across platforms and versions. A custom canonical format is required for deterministic hashing of proposals and votes.

---

## 9. Configuration

```toml
[p2p]
listen_addr = "0.0.0.0:26656"
seeds = ["node1@192.168.1.10:26656"]
max_peers = 50
handshake_timeout_ms = 5000
ping_interval_ms = 10000
pong_timeout_ms = 5000
max_message_size_bytes = 4194304   # 4 MB
inbound_buffer_size = 64
outbound_buffer_size = 64
reconnect_backoff_max_ms = 60000
peer_ban_duration_ms = 300000      # 5 minutes
```

---

## 10. Failure Modes

| Failure | Detection | Response |
|---------|-----------|----------|
| Peer unreachable | TCP connect timeout | Retry with backoff |
| Peer sends invalid message | Deserialization failure | Penalize score, drop message |
| Peer sends invalid signature | Signature verification failure | Penalize score heavily |
| Peer floods messages | Buffer full | Backpressure, then disconnect |
| Network partition | No messages from subset of peers | Consensus handles via timeouts |
| Peer equivocates | Two conflicting signed votes | Record evidence, disconnect, ban |

---

## Definition of Done — Networking

- [x] Transport protocol with framing specified
- [x] Authentication and encryption defined
- [x] Peer lifecycle state machine documented
- [x] Gossip protocol for votes, proposals, and transactions
- [x] Backpressure strategy for inbound, outbound, and consensus channel
- [x] Async architecture with task breakdown
- [x] Serialization format specified (canonical, not serde default)
- [x] Configuration parameters listed
- [x] Failure modes and responses documented
- [x] ASCII diagrams included
- [x] No Rust source code included
