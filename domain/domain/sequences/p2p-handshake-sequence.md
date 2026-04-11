# P2P Handshake Sequence

Source: `src/p2p/peer.rs` — `handshake()`, `src/p2p/manager.rs` — `spawn_peer()`

## Outbound Handshake (node dials seed)

```mermaid
sequenceDiagram
    participant A as Node A Outbound
    participant B as Node B Inbound

    Note over A,B: TCP established, PlainFramer created

    A->>B: Handshake frame with node_id, ed_pub, version, chain_id, x_pub, nonce, sig
    Note over B: validate_peer, check version and chain_id, verify ed25519 sig
    B->>A: Handshake frame with node_id, ed_pub, version, chain_id, x_pub, nonce, sig
    Note over A: validate_peer, same checks

    A->>B: HandshakeAck ok=true

    Note over A,B: X25519 ECDH, shared = A.x_sec.dh(B.x_pub)
    Note over A,B: session_key = SHA-256(shared concat chain_id)
    Note over A,B: PlainFramer upgraded to EncryptedFramer ChaCha20-Poly1305
    Note over A,B: Control::PeerConnected sent to P2pManager
```

## Inbound Handshake (node accepts connection)

```mermaid
sequenceDiagram
    participant A as Node A Dialer
    participant B as Node B Listener

    Note over A,B: TCP accept, PlainFramer created

    A->>B: Handshake frame
    Note over B: validate_peer
    B->>A: Handshake frame
    Note over A: validate_peer
    A->>B: HandshakeAck

    Note over B: derive session key from ECDH
    Note over A,B: both upgrade to EncryptedFramer
```

## Post-Handshake Encrypted Message Loop

```mermaid
sequenceDiagram
    participant Peer as Remote Peer
    participant Reader as Reader Task
    participant Writer as Writer Task
    participant Manager as P2pManager
    participant Consensus as ConsensusCore

    Note over Reader: loop, read 4-byte length prefix

    Peer->>Reader: length prefix 4 bytes then ciphertext
    Note over Reader: ChaCha20-Poly1305 decrypt with nonce dir and counter
    Note over Reader: Seen cache dedup, LRU 10k entries
    Note over Reader: decode NetworkMessage from MsgType and payload

    alt Proposal received
        Reader->>Consensus: ProposalReceived via try_send, drops if channel full
        Reader->>Manager: gossip to other peers
    else Prevote or Precommit received
        Reader->>Consensus: VoteReceived via try_send
        Reader->>Manager: gossip to other peers
    end

    Note over Writer: loop, drain outbound mpsc queue
    Manager->>Writer: NetworkMessage via outbound channel
    Note over Writer: encode and encrypt with ChaCha20-Poly1305
    Writer->>Peer: length prefix 4 bytes then ciphertext
```

## Handshake Failure Modes

| Failure | Cause | Result |
|---|---|---|
| Version mismatch | `peer.protocol_version != my_version` | Connection dropped |
| Chain ID mismatch | `peer.chain_id != chain_id.as_bytes()` | Connection dropped |
| Bad ed25519 signature | `VerifyingKey::verify_strict` fails | Connection dropped |
| ACK rejected | Inbound receives ok=false | Connection dropped |
| Oversized frame | `len > max_message_size_bytes` | Reader task exits, PeerDisconnected |
| AEAD decrypt failure | Wrong key or tampered frame | Frame silently dropped, reader continues |

> **Verified against:** `src/p2p/peer.rs` — `handshake()`, `validate_peer()`, `spawn_peer()` writer/reader tasks.
