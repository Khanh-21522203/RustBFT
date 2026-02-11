use crate::types::{Address, Hash};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Account {
    pub address: Address,
    pub balance: u128,
    pub nonce: u64,
    pub code_hash: Option<Hash>,
    pub storage_root: Hash,
}

impl Account {
    pub fn empty(address: Address) -> Self {
        Self {
            address,
            balance: 0,
            nonce: 0,
            code_hash: None,
            storage_root: Hash([0u8; 32]),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChainParams {
    pub max_block_gas: u64,
    pub max_tx_bytes: u32,
    pub max_block_bytes: u32,
}

impl Default for ChainParams {
    fn default() -> Self {
        Self {
            max_block_gas: 10_000_000,
            max_tx_bytes: 65_536,
            max_block_bytes: 1_048_576,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotId(pub usize);

#[derive(Clone, Debug)]
struct StateChange {
    key: Key,
    old: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Account(Address),
    Code(Hash),
    Storage(Address, Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct AppState {
    pub accounts: BTreeMap<Address, Account>,
    pub contract_code: BTreeMap<Hash, Vec<u8>>,
    pub contract_storage: BTreeMap<(Address, Vec<u8>), Vec<u8>>,
    pub params: ChainParams,
    stack: Vec<Vec<StateChange>>,
}

impl AppState {
    pub fn new(params: ChainParams) -> Self {
        Self {
            accounts: BTreeMap::new(),
            contract_code: BTreeMap::new(),
            contract_storage: BTreeMap::new(),
            params,
            stack: Vec::new(),
        }
    }

    pub fn snapshot(&mut self) -> SnapshotId {
        self.stack.push(Vec::new());
        SnapshotId(self.stack.len())
    }

    pub fn revert_to(&mut self, id: SnapshotId) {
        while self.stack.len() > id.0 {
            let changes = self.stack.pop().unwrap();
            for ch in changes.into_iter().rev() {
                self.apply_raw(ch.key, ch.old.as_deref());
            }
        }
    }

    pub fn commit_snapshot(&mut self, id: SnapshotId) {
        while self.stack.len() > id.0 {
            let top = self.stack.pop().unwrap();
            if let Some(prev) = self.stack.last_mut() {
                prev.extend(top);
            }
        }
    }

    pub fn get_account(&self, addr: Address) -> Account {
        self.accounts.get(&addr).cloned().unwrap_or_else(|| Account::empty(addr))
    }

    pub fn set_account(&mut self, acc: Account) {
        let key = Key::Account(acc.address);
        let old = self.accounts.get(&acc.address).map(encode_account);

        self.record_change(key.clone(), old);
        self.apply_raw(key, Some(&encode_account(&acc)));
    }

    pub fn set_code(&mut self, h: Hash, code: Vec<u8>) {
        let key = Key::Code(h);
        let old = self.contract_code.get(&h).cloned();

        self.record_change(key.clone(), old);
        self.apply_raw(key, Some(&code));
    }

    pub fn set_storage(&mut self, addr: Address, k: Vec<u8>, v: Vec<u8>) {
        let key = Key::Storage(addr, k.clone());
        let old = self.contract_storage.get(&(addr, k)).cloned();

        self.record_change(key.clone(), old);
        self.apply_raw(key, Some(&v));
    }

    fn record_change(&mut self, key: Key, old: Option<Vec<u8>>) {
        if let Some(frame) = self.stack.last_mut() {
            frame.push(StateChange { key, old });
        }
    }

    fn apply_raw(&mut self, key: Key, value: Option<&[u8]>) {
        match key {
            Key::Account(addr) => {
                if let Some(v) = value {
                    self.accounts.insert(addr, decode_account(addr, v));
                } else {
                    self.accounts.remove(&addr);
                }
            }
            Key::Code(h) => {
                if let Some(v) = value {
                    self.contract_code.insert(h, v.to_vec());
                } else {
                    self.contract_code.remove(&h);
                }
            }
            Key::Storage(addr, k) => {
                if let Some(v) = value {
                    self.contract_storage.insert((addr, k), v.to_vec());
                } else {
                    self.contract_storage.remove(&(addr, k));
                }
            }
        }
    }
}

fn encode_account(a: &Account) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&a.address.0);
    out.extend_from_slice(&a.balance.to_be_bytes());
    out.extend_from_slice(&a.nonce.to_be_bytes());
    match a.code_hash {
        Some(h) => {
            out.push(1);
            out.extend_from_slice(&h.0);
        }
        None => out.push(0),
    }
    out.extend_from_slice(&a.storage_root.0);
    out
}

fn decode_account(addr: Address, bytes: &[u8]) -> Account {
    // MVP: minimal checks; assume correct sizes.
    let mut i = 20;
    let mut bal = [0u8; 16];
    bal.copy_from_slice(&bytes[i..i + 16]);
    i += 16;
    let mut nonce = [0u8; 8];
    nonce.copy_from_slice(&bytes[i..i + 8]);
    i += 8;
    let has_code = bytes[i] == 1;
    i += 1;
    let code_hash = if has_code {
        let mut h = [0u8; 32];
        h.copy_from_slice(&bytes[i..i + 32]);
        i += 32;
        Some(Hash(h))
    } else {
        None
    };
    let mut sr = [0u8; 32];
    sr.copy_from_slice(&bytes[i..i + 32]);

    Account {
        address: addr,
        balance: u128::from_be_bytes(bal),
        nonce: u64::from_be_bytes(nonce),
        code_hash,
        storage_root: Hash(sr),
    }
}
