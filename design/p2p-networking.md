# P2P Networking

## Purpose

Manages TCP connections to peers, performs authenticated encrypted handshakes, and gossips consensus messages (proposals, prevotes, precommits) to all connected peers.

## Scope

**In scope:**
- TCP listener and outbound seed connections
- X25519 DH + ChaCha20-Poly1305 encrypted framing
- Ed25519 authenticated identity handshake
- Fan-out gossip with LRU-based message deduplication
- Forwarding inbound proposals/votes to the consensus event channel
- Per-peer read/write Tokio tasks with bounded outbound queues

**Out of scope:**
- Peer discovery beyond seed list (no DHT/PEX in MVP)
- Reconnection / backoff (one-shot connect to seeds, no retry loop)
- Ping/Pong keepalive (message types defined but not scheduled)
- Block sync request/response handling (types defined, not implemented)
- Peer banning

## Primary User Flow

1. `P2pManager::new` creates channels and returns a `P2pHandle` (with `tx_cmd: mpsc::Sender<ConsensusCommand>` for the router to push broadcast commands).
2. `P2pManager::run` is spawned; starts TCP listener on `cfg.listen_addr` (default `0.0.0.0:26656`).
3. For each seed in `cfg.seeds`, dials TCP and calls `spawn_peer(PeerRole::Outbound, ...)`.
4. For each inbound connection, `spawn_peer(PeerRole::Inbound, ...)`.
5. `spawn_peer` runs handshake, then splits into writer + reader Tokio tasks.
6. Reader task: decrypt frames → dedup → verify signature → forward to `to_consensus` (try_send, bounded).
7. Manager loop: on `ConsensusCommand::BroadcastProposal/Vote`, calls `fanout` to send to all peers except sender.

## System Flow

```
TCP accept / seed connect
    │
    ▼
spawn_peer (src/p2p/manager.rs)
    │
    ├── PlainFramer::new (src/p2p/codec.rs)
    │       len-prefix framing over raw TCP (no encryption yet)
    │
    ├── handshake (src/p2p/peer.rs)
    │       Outbound: send HandshakeInfo → recv HandshakeInfo → send Ack
    │       Inbound:  recv HandshakeInfo → send HandshakeInfo → recv Ack
    │       HandshakeInfo = {node_id, ed_pub, protocol_version, chain_id, x_pub, nonce, sig}
    │       sig = ed25519_sign(nonce || chain_id)
    │       shared_secret = X25519(my_secret, peer_x_pub)
    │       session_key = sha256(shared_secret || chain_id)   [KDF]
    │       → EncryptedFramer { stream, max_size, CryptoState }
    │
    ├── stream.into_split() → (OwnedReadHalf, OwnedWriteHalf)
    │
    ├── Writer task (tokio::spawn)
    │       rx_out.recv() → encode_message → AEAD encrypt → len-prefix write
    │       nonce = [dir_send(1B)] + [send_ctr(8B)] + [0x000]
    │
    └── Reader task (tokio::spawn)
            read len-prefix → AEAD decrypt → dedup via Seen cache
            → decode_message → verify_proposal/verify_vote
            → to_consensus.try_send(ConsensusEvent)
            → tx_gossip.send((peer_id, msg))   [manager fanout]

P2pManager event loop (src/p2p/manager.rs:P2pManager::run):
    ├── rx_ctl: PeerConnected/Disconnected → update self.peers BTreeMap
    ├── rx_cmd: BroadcastProposal/Vote from CommandRouter → fanout
    └── rx_gossip: (sender_id, msg) from reader tasks → fanout (skip sender)
```

## Data Model

`P2pConfig` (`src/p2p/config.rs`):
- `listen_addr: String` — default `"0.0.0.0:26656"`
- `seeds: Vec<String>` — format `"node_id@ip:port"`; only the `ip:port` part after `@` is used for TCP dial
- `max_peers: usize` — default 50 (not enforced in MVP; `peers` BTreeMap grows without limit)
- `max_message_size_bytes: usize` — default 4MB; enforced at read/write
- `outbound_buffer_size: usize` — default 64 per-peer mpsc queue depth

`CryptoState` (`src/p2p/codec.rs`):
- `aead: ChaCha20Poly1305` — keyed with 32-byte session key
- `send_ctr: u64`, `recv_ctr: u64` — per-direction nonce counters (never reset)
- `dir_send: u8 = 0xA1`, `dir_recv: u8 = 0xB2` — direction byte in nonce

`Seen` (`src/p2p/gossip.rs`):
- `LruCache<Hash, ()>` with capacity 10,000
- Keyed by `sha256(plaintext_bytes)` — deduplicates at both reader (per-peer) and manager (global)

`NetworkMessage` variants (`src/p2p/msg.rs`):
- `Proposal(SignedProposal)`, `Prevote(SignedVote)`, `Precommit(SignedVote)`
- `Transaction(Vec<u8>)`, `Evidence(Vec<u8>)`, `Ping(u64)`, `Pong(u64)`, `Disconnect(u8)`
- `BlockRequest(Vec<u8>)`, `BlockResponse(Vec<u8>)` — defined but not handled beyond pass-through

## Interfaces and Contracts

**`P2pHandle`** (returned by `P2pManager::new`):
- `tx_cmd: mpsc::Sender<ConsensusCommand>` — `CommandRouter` sends `BroadcastProposal` / `BroadcastVote` here

**Seed address format:** `"<node_id_hex>@<ip>:<port>"` — only `<ip>:<port>` is extracted by `parse_seed_addr`

**Wire frame format (post-handshake):**
- `[len: u32 BE][ciphertext: len bytes]` where ciphertext = `ChaCha20Poly1305(key, nonce, [MsgType(1B)] + payload)`

**Handshake messages** use `PlainFramer` (no encryption):
- Frame: `[total_len: u32 BE][MsgType(1B)][payload]`

## Dependencies

**Internal modules:**
- `src/consensus/events.rs` — `ConsensusEvent` sent to consensus, `ConsensusCommand` received from router
- `src/types/serialization.rs` — `encode/decode_signed_proposal`, `encode/decode_signed_vote`
- `src/crypto/hash.rs` — `sha256` for KDF and dedup hash

**External crates:**
- `tokio` — async TCP, `into_split`, task spawning, `mpsc` channels
- `x25519-dalek` — ephemeral DH key exchange
- `chacha20poly1305` — AEAD encryption/decryption
- `ed25519-dalek` — identity signing and verification during handshake
- `lru` — `LruCache` for `Seen` deduplication

## Failure Modes and Edge Cases

- **Handshake failure:** `NetCodecError::Protocol` returned; peer task exits immediately; no reconnect.
- **AEAD decryption failure:** Frame silently dropped (`continue` in reader loop); peer connection continues.
- **Message too large:** Frame dropped if `len > max_size`; connection broken on write side.
- **Full inbound consensus channel:** `to_consensus.try_send` — drops message silently if consensus `rx` is full (bounded 256).
- **Full outbound peer queue:** Critical messages (Proposal/Prevote/Precommit) use `await` send; non-critical use `try_send`. All current fanout paths use `await`, so slow peers back-pressure the manager loop.
- **Peer disconnect:** Both writer and reader tasks finish → `PeerDisconnected` sent via `tx_ctl` → `peers.remove`.
- **Duplicate gossip:** Global `Seen` in manager + per-peer `Seen` in reader task prevent loops; LRU capacity 10,000 hashes.
- **Chain ID mismatch:** `validate_peer` returns `Protocol("chain_id mismatch")` → handshake rejected.
- **Reconnect:** Not implemented — seed nodes that disconnect are not retried.

## Observability and Debugging

- No structured logs in P2P path; connection errors silently drop.
- `Metrics.p2p_peers_connected` / `p2p_messages_sent/received/bytes_sent/received` exist in registry but are never updated (not wired to P2P code in MVP).
- Debugging starting point: `spawn_peer` at `src/p2p/manager.rs:spawn_peer` — add tracing here for connect/disconnect events.

## Risks and Notes

- `max_peers` is not enforced — the manager will accept unlimited inbound connections.
- No reconnect logic — a disconnected seed is permanently lost for the session.
- Nonce counter overflow at `u64::MAX` would reuse nonces; no guard present.
- CryptoState is cloned for writer/reader split — the same key but separate counters; directional bytes `0xA1`/`0xB2` prevent cross-direction reuse.
- `verify_proposal` and `verify_vote` closures are `|_| true` in `main.rs` — no signature checking on inbound messages.

Changes:

