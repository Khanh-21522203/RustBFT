use crate::mempool::{TxCommitInfo, tx_hash_bytes};
use redb::{Database, TableDefinition};
use rustbft_core::codec::encode_block;
use rustbft_core::crypto::sha256;
use rustbft_core::{Block, Hash, ValidatorSet};
use std::path::Path;
use std::sync::Mutex;

const T_BLOCKS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("blocks");
const T_BLOCK_HASH: TableDefinition<&[u8], &[u8]> = TableDefinition::new("block_hash");
const T_STATE_ROOT: TableDefinition<&[u8], &[u8]> = TableDefinition::new("state_root");
const T_VALSET: TableDefinition<&[u8], &[u8]> = TableDefinition::new("valset");
const T_META: TableDefinition<&[u8], &[u8]> = TableDefinition::new("meta");
const T_TX_INDEX: TableDefinition<&[u8], &[u8]> = TableDefinition::new("tx_index");

fn height_key(h: u64) -> [u8; 8] {
    h.to_be_bytes()
}

pub struct BlockStore {
    db: Mutex<Database>,
}

impl BlockStore {
    pub fn open(path: &Path) -> Result<Self, redb::Error> {
        let db = Database::create(path)?;
        // Ensure tables exist
        let write_txn = db.begin_write()?;
        write_txn.open_table(T_BLOCKS)?;
        write_txn.open_table(T_BLOCK_HASH)?;
        write_txn.open_table(T_STATE_ROOT)?;
        write_txn.open_table(T_VALSET)?;
        write_txn.open_table(T_META)?;
        write_txn.open_table(T_TX_INDEX)?;
        write_txn.commit()?;
        Ok(Self { db: Mutex::new(db) })
    }

    /// Persist a committed block along with its state root and validator set.
    pub fn save_block(
        &self,
        block: &Block,
        state_root: Hash,
        validator_set: &ValidatorSet,
    ) -> Result<(), anyhow::Error> {
        let height = block.header.height;
        let key = height_key(height);

        let block_bytes = encode_block(block);
        let block_hash = sha256(&block_bytes);
        let vs_bytes = serde_json::to_vec(validator_set)?;

        let db = self.db.lock().unwrap();
        let write_txn = db.begin_write()?;
        {
            let mut t_blocks = write_txn.open_table(T_BLOCKS)?;
            let mut t_hash = write_txn.open_table(T_BLOCK_HASH)?;
            let mut t_sr = write_txn.open_table(T_STATE_ROOT)?;
            let mut t_vs = write_txn.open_table(T_VALSET)?;
            let mut t_meta = write_txn.open_table(T_META)?;
            let mut t_tx = write_txn.open_table(T_TX_INDEX)?;

            t_blocks.insert(key.as_slice(), block_bytes.as_slice())?;
            t_hash.insert(key.as_slice(), block_hash.0.as_slice())?;
            t_sr.insert(key.as_slice(), state_root.0.as_slice())?;
            t_vs.insert(key.as_slice(), vs_bytes.as_slice())?;
            t_meta.insert(b"last_height".as_slice(), key.as_slice())?;

            for (idx, tx) in block.txs.iter().enumerate() {
                let hash = tx_hash_bytes(tx);
                let info = TxCommitInfo {
                    hash,
                    height,
                    index: idx as u32,
                    success: None,
                    gas_used: None,
                    error: None,
                };
                let encoded = serde_json::to_vec(&info)?;
                t_tx.insert(hash.0.as_slice(), encoded.as_slice())?;
            }
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn load_tx_commit(&self, hash: Hash) -> Result<Option<TxCommitInfo>, anyhow::Error> {
        let db = self.db.lock().unwrap();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(T_TX_INDEX)?;
        match table.get(hash.0.as_slice())? {
            None => Ok(None),
            Some(guard) => {
                let info: TxCommitInfo = serde_json::from_slice(guard.value())?;
                Ok(Some(info))
            }
        }
    }

    /// Load a block by height.
    pub fn load_block(&self, height: u64) -> Result<Option<Block>, anyhow::Error> {
        let db = self.db.lock().unwrap();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(T_BLOCKS)?;
        let key = height_key(height);
        match table.get(key.as_slice())? {
            None => Ok(None),
            Some(guard) => {
                let bytes = guard.value();
                let block = rustbft_core::codec::decode_block(bytes)
                    .map_err(|e| anyhow::anyhow!("decode block: {}", e))?;
                Ok(Some(block))
            }
        }
    }

    /// Load block hash by height.
    pub fn load_block_hash(&self, height: u64) -> Result<Option<Hash>, redb::Error> {
        let db = self.db.lock().unwrap();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(T_BLOCK_HASH)?;
        match table.get(height_key(height).as_slice())? {
            None => Ok(None),
            Some(guard) => {
                let bytes = guard.value();
                if bytes.len() == 32 {
                    let mut h = [0u8; 32];
                    h.copy_from_slice(bytes);
                    Ok(Some(Hash(h)))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Load state root by height.
    pub fn load_state_root(&self, height: u64) -> Result<Option<Hash>, redb::Error> {
        let db = self.db.lock().unwrap();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(T_STATE_ROOT)?;
        match table.get(height_key(height).as_slice())? {
            None => Ok(None),
            Some(guard) => {
                let bytes = guard.value();
                if bytes.len() == 32 {
                    let mut h = [0u8; 32];
                    h.copy_from_slice(bytes);
                    Ok(Some(Hash(h)))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Load validator set at a given height.
    pub fn load_validator_set(&self, height: u64) -> Result<Option<ValidatorSet>, anyhow::Error> {
        let db = self.db.lock().unwrap();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(T_VALSET)?;
        match table.get(height_key(height).as_slice())? {
            None => Ok(None),
            Some(guard) => {
                let vs: ValidatorSet = serde_json::from_slice(guard.value())?;
                Ok(Some(vs))
            }
        }
    }

    /// Get the last committed height (0 if none).
    pub fn last_height(&self) -> Result<u64, redb::Error> {
        let db = self.db.lock().unwrap();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(T_META)?;
        match table.get(b"last_height".as_slice())? {
            None => Ok(0),
            Some(guard) => {
                let bytes = guard.value();
                if bytes.len() == 8 {
                    Ok(u64::from_be_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6],
                        bytes[7],
                    ]))
                } else {
                    Ok(0)
                }
            }
        }
    }

    /// Prune blocks below a given height (keep only >= min_height).
    pub fn prune_below(&self, min_height: u64) -> Result<u64, redb::Error> {
        let db = self.db.lock().unwrap();
        let write_txn = db.begin_write()?;
        let mut pruned = 0u64;
        {
            let mut t_blocks = write_txn.open_table(T_BLOCKS)?;
            let mut t_hash = write_txn.open_table(T_BLOCK_HASH)?;
            let mut t_sr = write_txn.open_table(T_STATE_ROOT)?;
            let mut t_vs = write_txn.open_table(T_VALSET)?;

            for h in 1..min_height {
                let key = height_key(h);
                let k = key.as_slice();
                t_blocks.remove(k)?;
                t_hash.remove(k)?;
                t_sr.remove(k)?;
                t_vs.remove(k)?;
                pruned += 1;
            }
        }
        write_txn.commit()?;
        Ok(pruned)
    }
}
