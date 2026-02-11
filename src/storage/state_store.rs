use crate::state::accounts::AppState;
use rocksdb::{DB, Options, ColumnFamilyDescriptor};
use std::path::Path;

const CF_STATE_SNAPSHOT: &str = "state_snapshot"; // key: height (u64 BE) -> JSON-encoded AppState
const CF_META: &str = "state_meta";               // key: "latest" -> u64 BE height

fn height_key(h: u64) -> [u8; 8] {
    h.to_be_bytes()
}

/// Versioned state store: persists AppState snapshots keyed by height.
/// In production, this would be a versioned Merkle tree (IAVL / sparse Merkle).
/// For MVP, we store the full AppState JSON per committed height.
pub struct StateStore {
    db: DB,
}

impl StateStore {
    pub fn open(path: &Path) -> Result<Self, rocksdb::Error> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        let cfs = vec![
            ColumnFamilyDescriptor::new(CF_STATE_SNAPSHOT, Options::default()),
            ColumnFamilyDescriptor::new(CF_META, Options::default()),
        ];

        let db = DB::open_cf_descriptors(&opts, path, cfs)?;
        Ok(Self { db })
    }

    /// Save committed state at a given height.
    pub fn save_state(&self, height: u64, state: &AppState) -> Result<(), anyhow::Error> {
        let cf_snap = self.db.cf_handle(CF_STATE_SNAPSHOT).unwrap();
        let cf_meta = self.db.cf_handle(CF_META).unwrap();

        let state_bytes = serde_json::to_vec(state)?;
        let key = height_key(height);

        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(&cf_snap, &key, &state_bytes);
        batch.put_cf(&cf_meta, b"latest", &key);
        self.db.write(batch)?;
        Ok(())
    }

    /// Load state at a given height.
    pub fn load_state(&self, height: u64) -> Result<Option<AppState>, anyhow::Error> {
        let cf = self.db.cf_handle(CF_STATE_SNAPSHOT).unwrap();
        match self.db.get_cf(&cf, &height_key(height))? {
            None => Ok(None),
            Some(bytes) => {
                let state: AppState = serde_json::from_slice(&bytes)?;
                Ok(Some(state))
            }
        }
    }

    /// Load the latest committed state.
    pub fn load_latest_state(&self) -> Result<Option<(u64, AppState)>, anyhow::Error> {
        let cf_meta = self.db.cf_handle(CF_META).unwrap();
        let height = match self.db.get_cf(&cf_meta, b"latest")? {
            None => return Ok(None),
            Some(bytes) => {
                if bytes.len() != 8 { return Ok(None); }
                u64::from_be_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3],
                    bytes[4], bytes[5], bytes[6], bytes[7],
                ])
            }
        };
        match self.load_state(height)? {
            None => Ok(None),
            Some(state) => Ok(Some((height, state))),
        }
    }

    /// Prune state snapshots below min_height.
    pub fn prune_below(&self, min_height: u64) -> Result<u64, rocksdb::Error> {
        let cf = self.db.cf_handle(CF_STATE_SNAPSHOT).unwrap();
        let mut pruned = 0u64;
        for h in 1..min_height {
            self.db.delete_cf(&cf, &height_key(h))?;
            pruned += 1;
        }
        Ok(pruned)
    }
}
