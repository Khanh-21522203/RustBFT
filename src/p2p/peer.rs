use crate::p2p::codec::{PlainFramer, EncryptedFramer, CryptoState, NetCodecError};
use crate::p2p::msg::MsgType;
use crate::types::ValidatorId;
use crate::types::serialization::{Encoder, Decoder, CodecError};

use ed25519_dalek::{SigningKey, VerifyingKey, Signature, Signer};
use rand_core::{OsRng, RngCore};
use x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey};

#[derive(Clone, Debug)]
pub enum PeerRole { Inbound, Outbound }

#[derive(Clone)]
pub struct HandshakeDeps {
    pub my_id: ValidatorId,
    pub my_signing: SigningKey,
    pub my_verify: VerifyingKey,
    pub chain_id: String,
    pub protocol_version: u32,
}

#[derive(Clone, Debug)]
struct HandshakeInfo {
    node_id: ValidatorId,
    ed_pub: [u8; 32],
    protocol_version: u32,
    chain_id: Vec<u8>,
    x_pub: [u8; 32],
    nonce: [u8; 32],
    sig: [u8; 64],
}

fn encode_hs(h: &HandshakeInfo) -> Vec<u8> {
    let mut e = Encoder::new();
    e.put_bytes32(&h.node_id.0);
    e.put_bytes32(&h.ed_pub);
    e.put_u32(h.protocol_version);
    e.put_vec(&h.chain_id);
    e.put_bytes32(&h.x_pub);
    e.put_bytes32(&h.nonce);
    e.put_bytes64(&h.sig);
    e.into_bytes()
}

fn decode_hs(data: &[u8]) -> Result<HandshakeInfo, CodecError> {
    let mut d = Decoder::new(data);
    Ok(HandshakeInfo {
        node_id: ValidatorId(d.get_bytes32()?),
        ed_pub: d.get_bytes32()?,
        protocol_version: d.get_u32()?,
        chain_id: d.get_vec()?,
        x_pub: d.get_bytes32()?,
        nonce: d.get_bytes32()?,
        sig: d.get_bytes64()?,
    })
}

fn encode_ack(ok: bool, code: u8) -> Vec<u8> { vec![if ok {1} else {0}, code] }

fn decode_ack(data: &[u8]) -> Result<(bool,u8), CodecError> {
    if data.len() < 2 { return Err(CodecError::Invalid("ack len")); }
    Ok((data[0]==1, data[1]))
}

fn challenge(nonce: &[u8;32], chain_id: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(32 + chain_id.len());
    v.extend_from_slice(nonce);
    v.extend_from_slice(chain_id);
    v
}

fn kdf(shared: &[u8; 32], chain_id: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(shared);
    h.update(chain_id);
    let out = h.finalize();
    let mut k = [0u8; 32];
    k.copy_from_slice(&out);
    k
}

fn x25519() -> (EphemeralSecret, [u8;32]) {
    let sec = EphemeralSecret::random_from_rng(OsRng);
    let pubk = X25519PublicKey::from(&sec);
    (sec, pubk.to_bytes())
}

fn rand32() -> [u8;32] {
    let mut r = [0u8;32];
    OsRng.fill_bytes(&mut r);
    r
}

pub async fn handshake(role: PeerRole, mut f: PlainFramer, deps: HandshakeDeps,)
    -> Result<(ValidatorId, EncryptedFramer), NetCodecError> {

    let (my_xsec, my_xpub) = x25519();
    let my_nonce = rand32();
    let chain_id_bytes = deps.chain_id.as_bytes().to_vec();

    let my_sig = deps.my_signing.sign(&challenge(&my_nonce, &chain_id_bytes)).to_bytes();

    let my_hs = HandshakeInfo {
        node_id: deps.my_id,
        ed_pub: deps.my_verify.to_bytes(),
        protocol_version: deps.protocol_version,
        chain_id: chain_id_bytes.clone(),
        x_pub: my_xpub,
        nonce: my_nonce,
        sig: my_sig,
    };

    match role {
        PeerRole::Outbound => {
            f.write_plain(MsgType::Handshake, &encode_hs(&my_hs)).await?;
            let (ty, payload) = f.read_plain().await?;
            if ty != MsgType::Handshake { return Err(NetCodecError::Protocol("expected handshake back")); }
            let peer = decode_hs(&payload).map_err(|_| NetCodecError::Protocol("bad handshake"))?;
            validate_peer(&peer, &deps)?;
            f.write_plain(MsgType::HandshakeAck, &encode_ack(true,0)).await?;

            let peer_x = X25519PublicKey::from(peer.x_pub);
            let shared = my_xsec.diffie_hellman(&peer_x);
            let key = kdf(shared.as_bytes(), &chain_id_bytes);
            Ok((peer.node_id, f.into_encrypted(CryptoState::new(key))))
        }
        PeerRole::Inbound => {
            let (ty, payload) = f.read_plain().await?;
            if ty != MsgType::Handshake { return Err(NetCodecError::Protocol("expected handshake")); }
            let peer = decode_hs(&payload).map_err(|_| NetCodecError::Protocol("bad handshake"))?;
            validate_peer(&peer, &deps)?;

            f.write_plain(MsgType::Handshake, &encode_hs(&my_hs)).await?;
            let (ty2, payload2) = f.read_plain().await?;
            if ty2 != MsgType::HandshakeAck { return Err(NetCodecError::Protocol("expected ack")); }
            let (ok, _) = decode_ack(&payload2).map_err(|_| NetCodecError::Protocol("bad ack"))?;
            if !ok { return Err(NetCodecError::Protocol("handshake rejected")); }

            let peer_x = X25519PublicKey::from(peer.x_pub);
            let shared = my_xsec.diffie_hellman(&peer_x);
            let key = kdf(shared.as_bytes(), &chain_id_bytes);
            Ok((peer.node_id, f.into_encrypted(CryptoState::new(key))))
        }
    }
}

fn validate_peer(peer: &HandshakeInfo, deps: &HandshakeDeps) -> Result<(), NetCodecError> {
    if peer.protocol_version != deps.protocol_version { return Err(NetCodecError::Protocol("version mismatch")); }
    if peer.chain_id != deps.chain_id.as_bytes() { return Err(NetCodecError::Protocol("chain_id mismatch")); }

    let vk = VerifyingKey::from_bytes(&peer.ed_pub).map_err(|_| NetCodecError::Protocol("bad ed pub"))?;
    let ch = challenge(&peer.nonce, &peer.chain_id);
    let sig = Signature::from_bytes(&peer.sig);
    if vk.verify_strict(&ch, &sig).is_err() {
        return Err(NetCodecError::Protocol("bad handshake sig"));
    }
    Ok(())
}
