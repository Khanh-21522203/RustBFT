use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use ed25519_dalek::Signer;
use rand_core::OsRng;

pub fn generate_keypair() -> (SigningKey, VerifyingKey) {
    let sk = SigningKey::generate(&mut OsRng);
    let vk = sk.verifying_key();
    (sk, vk)
}

pub fn sign(sk: &SigningKey, msg: &[u8]) -> [u8; 64] {
    let sig: Signature = sk.sign(msg);
    sig.to_bytes()
}

pub fn verify(vk: &VerifyingKey, msg: &[u8], sig_bytes: &[u8; 64]) -> bool {
    let sig = Signature::from_bytes(sig_bytes);
    vk.verify_strict(msg, &sig).is_ok()
}
