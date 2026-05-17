use std::collections::BTreeMap;
use std::sync::Mutex;

use rustbft_core::Hash;
use rustbft_core::crypto::sha256;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxCommitInfo {
    pub hash: Hash,
    pub height: u64,
    pub index: u32,
    pub success: Option<bool>,
    pub gas_used: Option<u64>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxStatus {
    Pending,
    Committed(TxCommitInfo),
    Rejected { reason: String },
    Expired,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct MempoolTx {
    pub hash: Hash,
    pub bytes: Vec<u8>,
}

#[derive(Default)]
pub struct Mempool {
    pending: Mutex<BTreeMap<Hash, MempoolTx>>,
}

impl Mempool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, bytes: Vec<u8>) -> Hash {
        let hash = tx_hash_bytes(&bytes);
        let mut pending = self.pending.lock().unwrap();
        pending.entry(hash).or_insert(MempoolTx { hash, bytes });
        hash
    }

    pub fn contains(&self, hash: &Hash) -> bool {
        self.pending.lock().unwrap().contains_key(hash)
    }

    pub fn remove(&self, hash: &Hash) -> Option<MempoolTx> {
        self.pending.lock().unwrap().remove(hash)
    }

    pub fn len(&self) -> usize {
        self.pending.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn reap(&self, max_bytes: usize) -> Vec<Vec<u8>> {
        let pending = self.pending.lock().unwrap();
        let mut used = 0usize;
        let mut out = Vec::new();
        for tx in pending.values() {
            if used.saturating_add(tx.bytes.len()) > max_bytes {
                break;
            }
            used = used.saturating_add(tx.bytes.len());
            out.push(tx.bytes.clone());
        }
        out
    }
}

pub fn tx_hash_bytes(bytes: &[u8]) -> Hash {
    sha256(bytes)
}
