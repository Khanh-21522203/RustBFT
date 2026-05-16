use crate::execution::accounts::AppState;
use redb::{Database, TableDefinition};
use std::path::Path;
use std::sync::Mutex;

const T_STATE_SNAPSHOT: TableDefinition<&[u8], &[u8]> = TableDefinition::new("state_snapshot");
const T_META: TableDefinition<&[u8], &[u8]> = TableDefinition::new("state_meta");

fn height_key(h: u64) -> [u8; 8] {
    h.to_be_bytes()
}

/// Versioned state store: persists AppState snapshots keyed by height.
/// In production, this would be a versioned Merkle tree (IAVL / sparse Merkle).
/// For MVP, we store the full AppState JSON per committed height.
pub struct StateStore {
    db: Mutex<Database>,
}

impl StateStore {
    pub fn open(path: &Path) -> Result<Self, redb::Error> {
        let db = Database::create(path)?;
        let write_txn = db.begin_write()?;
        write_txn.open_table(T_STATE_SNAPSHOT)?;
        write_txn.open_table(T_META)?;
        write_txn.commit()?;
        Ok(Self { db: Mutex::new(db) })
    }

    /// Save committed state at a given height.
    pub fn save_state(&self, height: u64, state: &AppState) -> Result<(), anyhow::Error> {
        let state_bytes = serde_json::to_vec(state)?;
        let key = height_key(height);

        let db = self.db.lock().unwrap();
        let write_txn = db.begin_write()?;
        {
            let mut t_snap = write_txn.open_table(T_STATE_SNAPSHOT)?;
            let mut t_meta = write_txn.open_table(T_META)?;
            t_snap.insert(key.as_slice(), state_bytes.as_slice())?;
            t_meta.insert(b"latest".as_slice(), key.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Load state at a given height.
    pub fn load_state(&self, height: u64) -> Result<Option<AppState>, anyhow::Error> {
        let db = self.db.lock().unwrap();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(T_STATE_SNAPSHOT)?;
        match table.get(height_key(height).as_slice())? {
            None => Ok(None),
            Some(guard) => {
                let state: AppState = serde_json::from_slice(guard.value())?;
                Ok(Some(state))
            }
        }
    }

    /// Load the latest committed state.
    pub fn load_latest_state(&self) -> Result<Option<(u64, AppState)>, anyhow::Error> {
        let db = self.db.lock().unwrap();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(T_META)?;
        let height = match table.get(b"latest".as_slice())? {
            None => return Ok(None),
            Some(guard) => {
                let bytes = guard.value();
                if bytes.len() != 8 { return Ok(None); }
                u64::from_be_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3],
                    bytes[4], bytes[5], bytes[6], bytes[7],
                ])
            }
        };
        drop(read_txn);
        drop(db);
        match self.load_state(height)? {
            None => Ok(None),
            Some(state) => Ok(Some((height, state))),
        }
    }

    /// Prune state snapshots below min_height.
    pub fn prune_below(&self, min_height: u64) -> Result<u64, redb::Error> {
        let db = self.db.lock().unwrap();
        let write_txn = db.begin_write()?;
        let mut pruned = 0u64;
        {
            let mut table = write_txn.open_table(T_STATE_SNAPSHOT)?;
            for h in 1..min_height {
                table.remove(height_key(h).as_slice())?;
                pruned += 1;
            }
        }
        write_txn.commit()?;
        Ok(pruned)
    }
}
