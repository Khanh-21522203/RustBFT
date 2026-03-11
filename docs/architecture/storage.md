# RustBFT — Storage Layer

**Purpose:** Define the persistence layer: block storage, state storage, WAL, crash recovery, and data layout.
**Audience:** Engineers implementing the storage subsystem.

---

## 1. Responsibilities

The storage layer is responsible for:

1. **Block persistence** — store committed blocks with headers, transactions, and commit signatures.
2. **State persistence** — store the application state (accounts, contract storage) as a versioned Merkle tree.
3. **Write-Ahead Log (WAL)** — durably record in-progress consensus state for crash recovery.
4. **Crash recovery** — restore the node to a consistent state after an unexpected shutdown.
5. **Query support** — serve block and state queries for the RPC layer.

The storage layer MUST NOT:

- Interpret block contents or execute transactions.
- Participate in consensus decisions.
- Access the network.

---

## 2. Storage Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     STORAGE LAYER                            │
│                                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │
│  │  Block Store  │  │  State Store  │  │     WAL      │      │
│  │              │  │              │  │              │      │
│  │  Blocks by   │  │  Merkle tree │  │  Consensus   │      │
│  │  height/hash │  │  (versioned) │  │  round state │      │
│  │              │  │              │  │              │      │
│  │  RocksDB     │  │  RocksDB     │  │  File-based  │      │
│  │  (or sled)   │  │  (or sled)   │  │  append-only │      │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘      │
│         │                 │                 │               │
│         └─────────────────┴─────────────────┘               │
│                           │                                  │
│              ┌────────────▼─────────────┐                   │
│              │   Storage Engine (KV)     │                   │
│              │   RocksDB / sled          │                   │
│              └──────────────────────────┘                   │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

---

## 3. Storage Engine Choice

### MVP: RocksDB (via `rust-rocksdb` crate)

| Property | RocksDB |
|----------|---------|
| Type | LSM-tree based key-value store |
| Write performance | Excellent (sequential writes) |
| Read performance | Good (with bloom filters) |
| Crash safety | WAL + atomic flush |
| Maturity | Battle-tested in production blockchains |
| Rust bindings | `rust-rocksdb` crate |

### Alternative: sled

sled is a pure-Rust embedded database. It may be used if minimizing C dependencies is a priority. The storage layer is abstracted behind a trait, so the engine can be swapped.

### Storage Trait

```
trait StorageEngine:
    fn get(cf: &str, key: &[u8]) -> Result<Option<Vec<u8>>>
    fn put(cf: &str, key: &[u8], value: &[u8]) -> Result<()>
    fn delete(cf: &str, key: &[u8]) -> Result<()>
    fn write_batch(ops: Vec<BatchOp>) -> Result<()>
    fn iterator(cf: &str, prefix: &[u8]) -> Result<Iterator>
    fn flush() -> Result<()>
```

All storage operations go through this trait, enabling testing with an in-memory implementation.

---

## 4. Block Store

### 4.1 Data Layout

```
Column Family: "blocks"

Keys and values:
    "block/height/{H}"          → serialized Block
    "block/hash/{hash}"         → height (u64 BE)
    "block/latest"              → height (u64 BE)
    "commit/{H}"                → serialized CommitInfo (>2/3 precommit sigs)
    "tx/hash/{tx_hash}"         → (height, tx_index)
    "receipt/{H}/{tx_index}"    → serialized Receipt
```

### 4.2 Block Serialization

Blocks are serialized using the canonical binary format (same as network serialization). This ensures that `sha256(serialized_block)` produces the same hash everywhere.

### 4.3 Operations

| Operation | Description | Caller |
|-----------|-------------|--------|
| `store_block(block, commit)` | Persist block + commit info | Node (after consensus commit) |
| `get_block_by_height(h)` | Retrieve block at height h | RPC, sync |
| `get_block_by_hash(hash)` | Retrieve block by hash | RPC |
| `get_latest_height()` | Return the highest committed height | Consensus (on startup), RPC |
| `get_tx_by_hash(hash)` | Return tx + receipt | RPC |
| `get_receipt(h, idx)` | Return receipt for tx at (height, index) | RPC |

---

## 5. State Store

### 5.1 Merkle Tree

The state is stored as a **sparse Merkle trie** (SMT) with the following properties:

- **Keys:** 32-byte hashes (e.g., `sha256(address)` for accounts, `sha256(address || storage_key)` for contract storage).
- **Values:** serialized account or storage value.
- **Branching factor:** 16 (nibble-based trie).
- **Hash function:** SHA-256.
- **Root hash:** deterministic given the set of (key, value) pairs.

### 5.2 Versioning

The state store maintains versions keyed by block height:

```
Column Family: "state"

Keys:
    "state/root/{H}"                    → state root hash at height H
    "state/node/{node_hash}"            → serialized trie node
    "state/leaf/{key}"                  → (value, last_modified_height)
```

### 5.3 State Pruning

To prevent unbounded storage growth:

- The system retains the full state for the latest `N` heights (configurable, default: 100).
- Older state versions are pruned (trie nodes not referenced by any retained version are deleted).
- Pruning runs asynchronously on a background thread, never blocking consensus.

```
Pruning strategy:
    retain_heights = max(1, latest_height - pruning_window)
    
    For each height < retain_heights:
        Mark trie nodes unique to that version
        Delete marked nodes in a background batch
```

### 5.4 Operations

| Operation | Description | Caller |
|-----------|-------------|--------|
| `apply_changeset(height, changes)` | Apply a batch of state changes and compute new root | State machine (during Commit) |
| `get_state_root(height)` | Return the state root at a given height | Consensus, RPC |
| `get_account(address, height)` | Return account state at a given height | State machine, RPC |
| `get_storage(address, key, height)` | Return contract storage value | State machine, RPC |
| `get_proof(key, height)` | Return Merkle inclusion/exclusion proof | RPC (future) |

---

## 6. Write-Ahead Log (WAL)

### 6.1 Purpose

The WAL ensures that the consensus core can recover its in-progress state after a crash. Without the WAL, a crash during a consensus round could cause the node to:
- Lose votes it has already cast (potentially violating the "vote once" rule).
- Re-propose a different block for the same (height, round).
- Lose its lock state.

### 6.2 WAL Format

The WAL is a **file-based, append-only log** stored separately from the main database.

```
WAL file: data/wal/consensus.wal

Entry format:
    ┌──────────┬──────────┬──────────────┬──────────┐
    │ CRC32 (4)│ Len (4)  │ Payload (var)│ Pad (0-3)│
    └──────────┴──────────┴──────────────┴──────────┘

    CRC32: checksum of (Len + Payload)
    Len:   payload length in bytes (4-byte BE)
    Payload: serialized WALEntry
    Pad:   padding to 4-byte alignment
```

### 6.3 WAL Entry Types

```
WALEntry:
    RoundStarted { height, round }
    ProposalSent { height, round, proposal_hash }
    VoteSent { height, round, vote_type, block_hash }
    Locked { height, round, block_hash }
    Unlocked { height, round }
    CommitStarted { height, block_hash }
    CommitCompleted { height, state_root }
```

### 6.4 WAL Write Rules

| Event | WAL Entry | Timing |
|-------|-----------|--------|
| Enter new round | `RoundStarted` | Before processing any events for the round |
| Send proposal | `ProposalSent` | Before broadcasting the proposal |
| Send vote | `VoteSent` | Before broadcasting the vote |
| Lock on block | `Locked` | Before updating lock state |
| Unlock | `Unlocked` | Before clearing lock state |
| Begin commit | `CommitStarted` | Before executing the block |
| Finish commit | `CommitCompleted` | After state is persisted |

**Critical rule:** WAL entries MUST be `fsync`'d to disk before the corresponding action is taken. This is the "write-ahead" guarantee.

### 6.5 WAL Truncation

After a successful commit (`CommitCompleted`), the WAL is truncated:

```
On CommitCompleted for height H:
    1. Ensure block and state for H are persisted
    2. Truncate WAL (delete all entries)
    3. WAL is now empty, ready for height H+1
```

### 6.6 WAL Recovery

On startup, the node checks the WAL:

```
Recovery procedure:
    1. Read all WAL entries
    2. Validate CRC32 for each entry
    3. Discard entries with invalid CRC (truncated write)
    
    4. IF WAL is empty:
        → Normal startup from last committed height
    
    5. IF WAL contains CommitCompleted:
        → Commit was successful, truncate WAL, start normally
    
    6. IF WAL contains CommitStarted but no CommitCompleted:
        → Crash during block execution
        → Re-execute the block from storage
        → If block not in storage: discard, start from last committed height
    
    7. IF WAL contains VoteSent entries:
        → Restore lock state and vote history
        → Resume consensus at the recorded (height, round)
        → Do NOT re-send votes (peers may have already received them)
    
    8. IF WAL contains only RoundStarted:
        → Resume at that (height, round) with clean state
```

---

## 7. Threading Model

```
┌────────────────────────────────────────────────────────────┐
│                                                            │
│   Consensus Core Thread                                    │
│   ├── Emits: PersistBlock, WriteWAL commands               │
│   └── Does NOT call storage directly                       │
│                                                            │
│   Storage Thread (dedicated, blocking I/O)                 │
│   ├── Receives commands via bounded mpsc channel            │
│   ├── Executes RocksDB / file I/O                          │
│   ├── Sends completion notifications back                  │
│   └── WAL fsync happens here                               │
│                                                            │
│   Pruning Thread (background, low priority)                │
│   ├── Periodically scans for prunable state versions       │
│   └── Deletes old trie nodes in batches                    │
│                                                            │
│   Query Thread Pool (for RPC reads)                        │
│   ├── Serves read-only queries from RPC                    │
│   └── Uses RocksDB snapshots (no locks on write path)      │
│                                                            │
└────────────────────────────────────────────────────────────┘
```

**Key design:** The consensus core never calls storage functions directly. It emits commands. The node binary routes these commands to the storage thread. This ensures the consensus core is never blocked by disk I/O (except indirectly via channel backpressure, which is bounded and measurable).

---

## 8. Data Integrity

### 8.1 Checksums

- WAL entries include CRC32 checksums.
- Block hashes serve as integrity checks for block data.
- State root hashes verify state integrity.

### 8.2 Atomic Writes

- Block + commit + receipts are written in a single RocksDB `WriteBatch` (atomic).
- State changes are applied in a single `WriteBatch`.
- If a `WriteBatch` fails, no partial data is written.

### 8.3 Corruption Detection

On startup:
1. Verify `latest_height` key exists and is valid.
2. Load block at `latest_height` and verify its hash.
3. Load state root at `latest_height` and verify it matches the block header.
4. If any check fails: **HALT** with a corruption error. Operator must restore from backup or re-sync.

---

## 9. Configuration

```toml
[storage]
data_dir = "./data"
engine = "rocksdb"                  # or "sled" or "memory" (testing)
wal_dir = "./data/wal"
pruning_window = 100                # Keep state for last N heights
pruning_interval_seconds = 60       # How often to run pruning
rocksdb_cache_size_mb = 256
rocksdb_max_open_files = 1000
rocksdb_write_buffer_size_mb = 64
```

---

## 10. Backup and Restore

### 10.1 Backup

```
Backup procedure:
    1. Pause pruning
    2. Create RocksDB checkpoint (atomic, COW snapshot)
    3. Copy checkpoint directory to backup location
    4. Resume pruning
```

RocksDB checkpoints are near-instantaneous and do not block writes.

### 10.2 Restore

```
Restore procedure:
    1. Stop the node
    2. Replace data directory with backup
    3. Delete WAL directory (stale WAL from a different state)
    4. Start the node
    5. Node resumes from the backed-up height
    6. Node syncs missing blocks from peers (block sync protocol)
```

---

## 11. Failure Modes

| Failure | Detection | Recovery |
|---------|-----------|----------|
| Disk full | Write returns error | HALT node. Operator must free space. |
| Corrupted WAL entry | CRC32 mismatch | Discard entry, recover from last valid entry. |
| Corrupted block data | Hash mismatch on load | HALT node. Restore from backup or re-sync. |
| State root mismatch | Computed root ≠ stored root | HALT node. This is a critical bug. |
| RocksDB corruption | RocksDB returns corruption error | HALT node. Restore from backup. |
| Slow disk | Write latency exceeds threshold | Log warning. Channel backpressure slows consensus. |

---

## Definition of Done — Storage

- [x] Storage architecture with block store, state store, and WAL
- [x] Storage engine choice justified (RocksDB) with trait abstraction
- [x] Block store data layout and operations
- [x] State store with Merkle tree and versioning
- [x] Pruning strategy defined
- [x] WAL format, entry types, and write rules specified
- [x] WAL recovery procedure with all crash scenarios
- [x] Threading model (dedicated storage thread, no direct calls from consensus)
- [x] Data integrity (checksums, atomic writes, corruption detection)
- [x] Configuration parameters
- [x] Backup and restore procedures
- [x] Failure modes and recovery
- [x] No Rust source code included
