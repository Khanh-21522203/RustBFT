use crate::types::{Block, Hash, ValidatorSet};
use crate::types::serialization::encode_block;
use crate::crypto::hash::sha256;
use rocksdb::{DB, Options, ColumnFamilyDescriptor};
use std::path::Path;

const CF_BLOCKS: &str = "blocks";         // key: height (u64 BE) -> encoded Block
const CF_BLOCK_HASH: &str = "block_hash"; // key: height (u64 BE) -> Hash(32)
const CF_STATE_ROOT: &str = "state_root"; // key: height (u64 BE) -> Hash(32)
const CF_VALSET: &str = "valset";         // key: height (u64 BE) -> JSON-encoded ValidatorSet
const CF_META: &str = "meta";             // key: "last_height" -> u64 BE

fn height_key(h: u64) -> [u8; 8] {
    h.to_be_bytes()
}

pub struct BlockStore {
    db: DB,
}

impl BlockStore {
    pub fn open(path: &Path) -> Result<Self, rocksdb::Error> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        let cfs = vec![
            ColumnFamilyDescriptor::new(CF_BLOCKS, Options::default()),
            ColumnFamilyDescriptor::new(CF_BLOCK_HASH, Options::default()),
            ColumnFamilyDescriptor::new(CF_STATE_ROOT, Options::default()),
            ColumnFamilyDescriptor::new(CF_VALSET, Options::default()),
            ColumnFamilyDescriptor::new(CF_META, Options::default()),
        ];

        let db = DB::open_cf_descriptors(&opts, path, cfs)?;
        Ok(Self { db })
    }

    /// Persist a committed block along with its state root and validator set.
    pub fn save_block(
        &self,
        block: &Block,
        state_root: Hash,
        validator_set: &ValidatorSet,
    ) -> Result<(), rocksdb::Error> {
        let height = block.header.height;
        let key = height_key(height);

        let block_bytes = encode_block(block);
        let block_hash = sha256(&block_bytes);

        let cf_blocks = self.db.cf_handle(CF_BLOCKS).unwrap();
        let cf_hash = self.db.cf_handle(CF_BLOCK_HASH).unwrap();
        let cf_sr = self.db.cf_handle(CF_STATE_ROOT).unwrap();
        let cf_vs = self.db.cf_handle(CF_VALSET).unwrap();
        let cf_meta = self.db.cf_handle(CF_META).unwrap();

        let vs_bytes = serde_json::to_vec(validator_set).unwrap_or_default();

        // Atomic batch write
        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(&cf_blocks, &key, &block_bytes);
        batch.put_cf(&cf_hash, &key, &block_hash.0);
        batch.put_cf(&cf_sr, &key, &state_root.0);
        batch.put_cf(&cf_vs, &key, &vs_bytes);
        batch.put_cf(&cf_meta, b"last_height", &key);

        self.db.write(batch)?;
        Ok(())
    }

    /// Load a block by height.
    pub fn load_block(&self, height: u64) -> Result<Option<Block>, anyhow::Error> {
        let cf = self.db.cf_handle(CF_BLOCKS).unwrap();
        let key = height_key(height);
        match self.db.get_cf(&cf, &key)? {
            None => Ok(None),
            Some(bytes) => {
                let block = crate::types::serialization::decode_block(&bytes)
                    .map_err(|e| anyhow::anyhow!("decode block: {}", e))?;
                Ok(Some(block))
            }
        }
    }

    /// Load block hash by height.
    pub fn load_block_hash(&self, height: u64) -> Result<Option<Hash>, rocksdb::Error> {
        let cf = self.db.cf_handle(CF_BLOCK_HASH).unwrap();
        match self.db.get_cf(&cf, &height_key(height))? {
            None => Ok(None),
            Some(bytes) => {
                if bytes.len() == 32 {
                    let mut h = [0u8; 32];
                    h.copy_from_slice(&bytes);
                    Ok(Some(Hash(h)))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Load state root by height.
    pub fn load_state_root(&self, height: u64) -> Result<Option<Hash>, rocksdb::Error> {
        let cf = self.db.cf_handle(CF_STATE_ROOT).unwrap();
        match self.db.get_cf(&cf, &height_key(height))? {
            None => Ok(None),
            Some(bytes) => {
                if bytes.len() == 32 {
                    let mut h = [0u8; 32];
                    h.copy_from_slice(&bytes);
                    Ok(Some(Hash(h)))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Load validator set at a given height.
    pub fn load_validator_set(&self, height: u64) -> Result<Option<ValidatorSet>, anyhow::Error> {
        let cf = self.db.cf_handle(CF_VALSET).unwrap();
        match self.db.get_cf(&cf, &height_key(height))? {
            None => Ok(None),
            Some(bytes) => {
                let vs: ValidatorSet = serde_json::from_slice(&bytes)?;
                Ok(Some(vs))
            }
        }
    }

    /// Get the last committed height (0 if none).
    pub fn last_height(&self) -> Result<u64, rocksdb::Error> {
        let cf = self.db.cf_handle(CF_META).unwrap();
        match self.db.get_cf(&cf, b"last_height")? {
            None => Ok(0),
            Some(bytes) => {
                if bytes.len() == 8 {
                    Ok(u64::from_be_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3],
                        bytes[4], bytes[5], bytes[6], bytes[7],
                    ]))
                } else {
                    Ok(0)
                }
            }
        }
    }

    /// Prune blocks below a given height (keep only >= min_height).
    pub fn prune_below(&self, min_height: u64) -> Result<u64, rocksdb::Error> {
        let cf_blocks = self.db.cf_handle(CF_BLOCKS).unwrap();
        let cf_hash = self.db.cf_handle(CF_BLOCK_HASH).unwrap();
        let cf_sr = self.db.cf_handle(CF_STATE_ROOT).unwrap();
        let cf_vs = self.db.cf_handle(CF_VALSET).unwrap();

        let mut pruned = 0u64;
        // Delete from height 1 to min_height - 1
        for h in 1..min_height {
            let key = height_key(h);
            let mut batch = rocksdb::WriteBatch::default();
            batch.delete_cf(&cf_blocks, &key);
            batch.delete_cf(&cf_hash, &key);
            batch.delete_cf(&cf_sr, &key);
            batch.delete_cf(&cf_vs, &key);
            self.db.write(batch)?;
            pruned += 1;
        }
        Ok(pruned)
    }
}
