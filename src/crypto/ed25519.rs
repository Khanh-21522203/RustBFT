use std::fs;
use std::path::Path;
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use ed25519_dalek::Signer;
use rand_core::OsRng;
use anyhow::Result;

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

pub fn load_or_generate_keypair<P: AsRef<Path>>(
    path: P,
) -> Result<(SigningKey, VerifyingKey)> {
    let path = path.as_ref();

    if path.exists() {
        let bytes = fs::read(path)?;
        if bytes.len() != 32 {
            anyhow::bail!("invalid key file length");
        }
        let mut sk_bytes = [0u8; 32];
        sk_bytes.copy_from_slice(&bytes);
        let signing = SigningKey::from_bytes(&sk_bytes);
        let verify = signing.verifying_key();
        Ok((signing, verify))
    } else {
        let (signing, verify) = generate_keypair();
        fs::write(path, signing.to_bytes())?;
        Ok((signing, verify))
    }
}