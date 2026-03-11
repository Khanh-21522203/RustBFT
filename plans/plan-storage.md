# Feature: Storage Layer

## 1. Purpose

The Storage Layer durably persists committed blocks, versioned application state, and the Write-Ahead Log (WAL) used for crash recovery. It is the only module that performs disk I/O on the critical path. It decouples the consensus core from disk latency: the consensus core emits fire-and-forget commands; the storage thread batches and flushes them asynchronously, using the WAL to guarantee recoverability.

## 2. Responsibilities

- Persist committed blocks with headers, transactions, and commit signatures (`PersistBlock` command)
- Persist application state changesets and compute versioned Merkle state roots (`ApplyChangeset` command)
- Write WAL entries with CRC32 checksums and `fsync` before the corresponding consensus action (`WriteWAL` command)
- Perform WAL recovery on startup: reconstruct consensus state from the last valid WAL entries
- Serve read-only queries for the RPC and state machine layers (block by height/hash, account, contract storage, validator set)
- Prune old state versions on a background thread to prevent unbounded storage growth
- Detect storage corruption on startup and HALT the node
- Abstract the storage engine behind a trait to allow testing with an in-memory implementation

## 3. Non-Responsibilities

- Does not interpret block contents or execute transactions
- Does not participate in consensus decisions or vote on blocks
- Does not access the network
- Does not initiate pruning from the consensus critical path — pruning is background-only
- Does not provide EVM-compatible storage layout

## 4. Architecture Design

```
Consensus Core Thread
        |  WriteWAL, PersistBlock (fire-and-forget)
        v
  crossbeam channel (cap=16)
        |
+-------v-----------------------------------------+
|   Storage Thread (src/storage/mod.rs)            |
|                                                  |
|  recv StorageCommand:                            |
|    WriteWAL(entry)  → WAL file write + fsync     |
|    PersistBlock(b)  → RocksDB WriteBatch         |
|    ApplyChangeset   → Merkle trie update         |
|    Ack(responder)   → send completion signal     |
|                                                  |
+-------+-----------------------------------------+
        |
  WAL file (data/wal/consensus.wal)  ← append-only
  RocksDB: "blocks" CF              ← block store
  RocksDB: "state"  CF              ← state store
        |
+-------v-----------------------------------------+
|   Pruning Thread (background, low priority)      |
|   Prunes state nodes unreachable from current    |
|   retained window. Never touches WAL or blocks.  |
+--------------------------------------------------+

RPC / State Machine (read path):
  Direct RocksDB snapshot reads (no storage thread lock)
```

## 5. Core Data Structures (Rust)

```rust
// src/storage/mod.rs

pub enum StorageCommand {
    WriteWAL {
        entry: WALEntry,
        responder: Option<oneshot::Sender<()>>,  // None = fire-and-forget
    },
    PersistBlock {
        block: Block,
        commit: CommitInfo,
        receipts: Vec<Receipt>,
        responder: oneshot::Sender<Result<(), StorageError>>,
    },
    ApplyChangeset {
        height: u64,
        changeset: StateChangeset,
        responder: oneshot::Sender<Result<Hash, StorageError>>,
    },
    GetLatestHeight {
        responder: oneshot::Sender<u64>,
    },
    Flush {
        responder: oneshot::Sender<()>,
    },
}

// src/storage/block_store.rs

pub struct BlockStore {
    db: Arc<DB>,    // rust-rocksdb
}

// src/storage/state_store.rs

pub struct StateStore {
    db: Arc<DB>,
    trie: SparseMerkleTrie,
}

pub struct StateChangeset {
    pub height: u64,
    pub accounts: BTreeMap<Address, Option<Account>>,   // None = delete
    pub contract_storage: BTreeMap<(Address, Bytes), Option<Bytes>>,
}

// src/storage/wal.rs

pub struct WAL {
    file: BufWriter<File>,
    path: PathBuf,
}

// WAL entry on disk:
// ┌──────────┬──────────┬──────────────┬──────────┐
// │ CRC32 (4)│ Len (4BE)│ Payload (var)│ Pad(0-3) │
// └──────────┴──────────┴──────────────┴──────────┘

// src/storage/trie.rs

pub struct SparseMerkleTrie {
    nodes: BTreeMap<Hash, TrieNode>,
    root: Hash,
}

pub enum TrieNode {
    Leaf { key: Hash, value: Bytes },
    Branch { children: [Option<Hash>; 16] },
}

// src/storage/engine.rs — abstraction for testing

pub trait StorageEngine: Send + Sync {
    fn get(&self, cf: &str, key: &[u8]) -> Result<Option<Vec<u8>>, StorageError>;
    fn write_batch(&self, ops: Vec<BatchOp>) -> Result<(), StorageError>;
    fn iterator(&self, cf: &str, prefix: &[u8]) -> Result<Box<dyn Iterator<Item = (Vec<u8>, Vec<u8>)>>, StorageError>;
    fn snapshot(&self) -> Box<dyn StorageSnapshot>;
}

pub enum BatchOp {
    Put { cf: String, key: Vec<u8>, value: Vec<u8> },
    Delete { cf: String, key: Vec<u8> },
}
```

## 6. Public Interfaces

```rust
// src/storage/mod.rs

pub struct StorageHandle {
    tx: crossbeam::channel::Sender<StorageCommand>,
}

impl StorageHandle {
    pub fn write_wal(&self, entry: WALEntry);
    pub fn write_wal_sync(&self, entry: WALEntry) -> Result<(), StorageError>;
    pub fn persist_block(&self, block: Block, commit: CommitInfo, receipts: Vec<Receipt>)
        -> Result<(), StorageError>;
    pub fn apply_changeset(&self, height: u64, changeset: StateChangeset)
        -> Result<Hash, StorageError>;
    pub fn get_latest_height(&self) -> u64;
    pub fn flush(&self);
}

/// Start the storage thread. Returns a handle for sending commands
/// and a read-only handle for RPC queries.
pub fn start(config: StorageConfig) -> (StorageHandle, StorageReader);

// StorageReader is Send + Sync, cloneable, used by RPC thread pool
pub struct StorageReader { ... }

impl StorageReader {
    pub fn get_block_by_height(&self, h: u64) -> Option<Block>;
    pub fn get_block_by_hash(&self, hash: &Hash) -> Option<Block>;
    pub fn get_latest_height(&self) -> u64;
    pub fn get_account(&self, addr: &Address, height: Option<u64>) -> Option<Account>;
    pub fn get_storage(&self, addr: &Address, key: &[u8], height: Option<u64>) -> Option<Bytes>;
    pub fn get_receipt(&self, height: u64, tx_index: usize) -> Option<Receipt>;
    pub fn get_tx(&self, hash: &Hash) -> Option<(Block, usize, Receipt)>;
    pub fn get_state_root(&self, height: u64) -> Option<Hash>;
    pub fn get_validator_set(&self, height: Option<u64>) -> Option<ValidatorSet>;
}

// WAL recovery — called at startup before starting the storage thread
pub fn recover_wal(config: &StorageConfig) -> WALRecoveryResult;

pub struct WALRecoveryResult {
    pub height: u64,
    pub round: u32,
    pub locked_block: Option<Hash>,
    pub locked_round: Option<u32>,
    pub votes_sent: Vec<WALEntry>,
    pub needs_block_re_execution: bool,
}
```

## 7. Internal Algorithms

### WAL Write
```
fn write_wal(entry: WALEntry):
    payload = canonical_encode(entry)
    crc = crc32(len_be(payload) + payload)
    write: [crc(4)] [len(4BE)] [payload] [pad to 4-byte align]
    file.flush()
    file.sync_all()     // fsync — must complete before returning
```

### WAL Recovery
```
fn recover_wal(path):
    entries = []
    loop:
        crc_read = read_u32_be(file)?           else break  // EOF
        len_read = read_u32_be(file)?           else break
        payload  = read_exact(file, len_read)?  else break
        pad      = read_exact(file, align_pad(len_read))?
        if crc32(len_be(len_read) + payload) != crc_read:
            break  // Truncated write — discard rest
        entries.push(canonical_decode(payload))

    // Interpret entries
    if entries is empty:
        return WALRecoveryResult { height: latest_committed, round: 0, ... }
    if last entry is CommitCompleted:
        truncate WAL; return normal startup
    if CommitStarted in entries but no CommitCompleted:
        return { needs_block_re_execution: true, ... }
    restore locked_block/round from Locked entries
    restore round from RoundStarted entries
    return WALRecoveryResult { ... }
```

### PersistBlock
```
fn persist_block(block, commit, receipts):
    batch = WriteBatch::new()
    batch.put("blocks", "block/height/{H}", encode(block))
    batch.put("blocks", "block/hash/{hash}", H.to_be_bytes())
    batch.put("blocks", "block/latest", H.to_be_bytes())
    batch.put("blocks", "commit/{H}", encode(commit))
    for (i, (tx, receipt)) in block.txs.zip(receipts).enumerate():
        batch.put("blocks", "tx/hash/{tx.hash}", encode((H, i)))
        batch.put("blocks", "receipt/{H}/{i}", encode(receipt))
    db.write(batch)   // atomic
```

### Sparse Merkle Trie Update
```
fn update_trie(trie, key: Hash, value: Bytes):
    // Nibble-based trie traversal on 32-byte key
    path = nibbles(key)   // 64 nibbles
    insert_or_update(trie.root, path, value)
    // Recompute hashes from leaf up to root
    trie.root = recompute_root(trie)

fn state_root(trie) -> Hash:
    trie.root  // sha256 of root node serialization
```

### Pruning
```
fn prune(latest_height, window):
    prune_below = latest_height.saturating_sub(window)
    for h in 0..prune_below:
        if state_root_at(h) exists:
            collect unreachable nodes from trie at h
            delete in batch
            delete "state/root/{h}"
    // Never prune WAL or block store
```

### Corruption Detection (Startup)
```
fn check_integrity():
    H = get "block/latest"
    if H is None: return  // fresh node
    block = get_block_by_height(H)?
    if sha256(encode(block)) != block.hash:
        HALT("block data corrupted at height {H}")
    state_root_stored = get "state/root/{H}"
    if state_root_stored != block.header.state_root:
        HALT("state root mismatch at height {H}")
```

## 8. Persistence Model

Three separate stores:
1. **WAL file** (`data/wal/consensus.wal`): append-only, CRC32-checksummed, fsync'd before returning. Truncated after each `CommitCompleted`. Not pruned; it only grows by one round's entries between commits.
2. **Block store** (RocksDB, `"blocks"` column family): immutable after write. Never pruned (block history is permanently retained). One `WriteBatch` per block.
3. **State store** (RocksDB, `"state"` column family): versioned by height. Pruned by the background pruning thread after `pruning_window` heights. State root at each retained height is always queryable.

## 9. Concurrency Model

The storage thread owns the RocksDB write handle and the WAL file exclusively. It processes commands sequentially from a `crossbeam::channel::Receiver<StorageCommand>`. No `Mutex` on the write path.

The `StorageReader` holds a cloned `Arc<DB>` and issues read-only RocksDB snapshot reads independently of the write path. Multiple RPC handler threads can read concurrently without coordination with the storage write thread.

The pruning thread holds a separate `Arc<DB>` reference and deletes unreachable trie nodes using standard write batches. It runs on a low-priority OS thread with a configurable interval.

## 10. Configuration

```toml
[storage]
data_dir = "./data"
engine = "rocksdb"                  # "rocksdb" | "memory" (testing)
wal_dir = "./data/wal"
pruning_window = 100                # Retain state for last N heights
pruning_interval_seconds = 60
rocksdb_cache_size_mb = 256
rocksdb_max_open_files = 1000
rocksdb_write_buffer_size_mb = 64
```

## 11. Observability

- `rustbft_storage_block_persist_duration_seconds` (Histogram) — time to persist a block including commit info
- `rustbft_storage_wal_write_duration_seconds` (Histogram) — time per WAL write including fsync
- `rustbft_storage_wal_entries` (Gauge) — current number of entries in the active WAL
- `rustbft_storage_db_size_bytes{cf}` (Gauge) — database size by column family (`blocks`, `state`)
- `rustbft_storage_pruning_duration_seconds` (Histogram) — time per pruning cycle
- `rustbft_storage_command_queue_length` (Gauge) — pending commands in the storage channel
- Log INFO `"Block persisted"` with `{height, hash, duration_ms}` on each commit
- Log WARN on slow WAL fsync (> 100ms)
- Log ERROR and HALT on integrity check failure or RocksDB corruption

## 12. Testing Strategy

- **`test_wal_write_and_recover`**: write a sequence of WAL entries, simulate crash (drop file handle), recover → all entries present with correct CRC
- **`test_wal_truncated_entry_discarded`**: write a complete entry then a partial entry, recover → only complete entry returned
- **`test_wal_truncation_on_commit_completed`**: write through CommitCompleted, recover → WAL truncated, clean startup
- **`test_persist_block_atomic`**: persist block with commit and receipts, read back each field → all present and correct
- **`test_state_root_deterministic`**: apply same changeset twice to fresh tries → same state root
- **`test_state_root_changes_on_different_data`**: two different changesets → different roots
- **`test_get_account_at_height`**: apply changeset at H, update at H+1, query at H → returns H value
- **`test_pruning_removes_old_state`**: persist 110 heights with window=100, trigger pruning → heights 0-9 no longer readable
- **`test_corruption_detection_bad_block`**: corrupt block bytes in RocksDB, startup → HALT triggered
- **`test_corruption_detection_state_root_mismatch`**: tamper with state root, startup → HALT triggered
- **`test_memory_engine_passes_same_tests`**: run all storage tests against the in-memory engine to verify trait correctness
- **`test_storage_reader_concurrent`**: 10 threads reading via StorageReader simultaneously → no panics, correct values

## 13. Open Questions

- **Persistent module cache for WASM**: Should compiled `wasmtime::Module` objects be serialized to RocksDB to avoid re-compilation on cold start? For MVP, in-memory cache only; re-compilation on restart is acceptable.
- **Block pruning**: Block history is currently retained forever. For production, a configurable block pruning window may be needed to control disk usage. Deferred post-MVP.
