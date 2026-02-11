use crate::crypto::hash::sha256;
use crate::state::accounts::AppState;
use crate::types::Hash;

pub fn compute_state_root(state: &AppState) -> Hash {
    let mut bytes = Vec::new();

    for (addr, acc) in state.accounts.iter() {
        bytes.extend_from_slice(b"acct");
        bytes.extend_from_slice(&addr.0);
        bytes.extend_from_slice(&acc.balance.to_be_bytes());
        bytes.extend_from_slice(&acc.nonce.to_be_bytes());
    }

    for (h, code) in state.contract_code.iter() {
        bytes.extend_from_slice(b"code");
        bytes.extend_from_slice(&h.0);
        bytes.extend_from_slice(&(code.len() as u32).to_be_bytes());
        bytes.extend_from_slice(code);
    }

    for ((addr, k), v) in state.contract_storage.iter() {
        bytes.extend_from_slice(b"stor");
        bytes.extend_from_slice(&addr.0);
        bytes.extend_from_slice(&(k.len() as u32).to_be_bytes());
        bytes.extend_from_slice(k);
        bytes.extend_from_slice(&(v.len() as u32).to_be_bytes());
        bytes.extend_from_slice(v);
    }

    sha256(&bytes)
}
