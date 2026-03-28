# Storage

## Purpose

Provides durable persistence for committed blocks, state snapshots, and consensus crash-recovery data using RocksDB column families and a text-based Write-Ahead Log.

## Scope

**In scope:**
- `BlockStore` — block bytes, block hashes, state roots, validator sets, and last committed height
- `StateStore` — full `AppState` JSON snapshots per height
- `WAL` — hex-encoded append-only log of consensus events for crash recovery
- Atomic batch writes via RocksDB `WriteBatch`
- Height-based pruning for both `BlockStore` and `StateStore`

**Out of scope:**
- Block sync / serving historical blocks over P2P (types defined, not wired)
- Incremental state trees (MVP uses full JSON snapshots)
- WAL replay on startup (WAL exists but startup code does not read and replay it)

## Primary User Flow

**Persist block (called by `CommandRouter` on `PersistBlock` command):**
1. `BlockStore::save_block(block, state_root, validator_set)` — atomic batch: block bytes, hash, state root, validator set JSON, last_height meta.
2. `StateStore::save_state(height, app_state)` — atomic batch: full AppState JSON + latest meta.
3. `WAL::truncate()` — reopen with truncation, clearing all entries for the committed height.

**Startup recovery (`main.rs`):**
1. `BlockStore::open` + `StateStore::open` → open or create RocksDB databases.
2. `block_store.last_height()` → resume at `last_height + 1`.
3. `state_store.load_latest_state()` → restore `AppState`; if None, use `AppState::new(ChainParams::default())`.
4. WAL replay: not implemented — WAL written but never read on startup.

**WAL writes (called by `CommandRouter` on `WriteWAL` command):**
1. `WAL::write_entry(entry)` — hex-encode binary entry, append `\n`, flush to disk.

## System Flow

```
CommandRouter::PersistBlock { block, state_root }
    ├── BlockStore::save_block (src/storage/block_store.rs)
    │       WriteBatch:
    │         CF_BLOCKS[height_key]    ← encode_block(block)   (binary)
    │         CF_BLOCK_HASH[height_key] ← sha256(block_bytes)  (32 bytes)
    │         CF_STATE_ROOT[height_key] ← state_root.0         (32 bytes)
    │         CF_VALSET[height_key]    ← serde_json(validator_set)
    │         CF_META["last_height"]   ← height_key
    │       db.write(batch)
    │
    └── StateStore::save_state (src/storage/state_store.rs)
            WriteBatch:
              CF_STATE_SNAPSHOT[height_key] ← serde_json(app_state)
              CF_META["latest"]             ← height_key
            db.write(batch)

WAL::write_entry (src/storage/wal.rs)
    entry.encode() → binary: height(8) | round(4) | kind(1) | len(4) | data | sha256_checksum(32)
    hex_encode(binary) + '\n' → writeln! + flush
```

## Data Model

**`BlockStore` RocksDB column families** (`src/storage/block_store.rs`):
- `CF_BLOCKS ("blocks")` — key: `height as u64 BE (8 bytes)`, value: `encode_block(block)` (custom binary)
- `CF_BLOCK_HASH ("block_hash")` — key: `height`, value: `sha256(block_bytes)` (32 bytes)
- `CF_STATE_ROOT ("state_root")` — key: `height`, value: `state_root.0` (32 bytes)
- `CF_VALSET ("valset")` — key: `height`, value: `serde_json(ValidatorSet)`
- `CF_META ("meta")` — key: `"last_height"`, value: `height as u64 BE (8 bytes)`

**`StateStore` RocksDB column families** (`src/storage/state_store.rs`):
- `CF_STATE_SNAPSHOT ("state_snapshot")` — key: `height`, value: `serde_json(AppState)`
- `CF_META ("state_meta")` — key: `"latest"`, value: `height as u64 BE (8 bytes)`

**`WalEntry`** (`src/storage/wal.rs`):
- Binary format: `height(8B) | round(4B) | kind(1B) | data_len(4B) | data | sha256(preceding_bytes)(32B)`
- On-disk: one hex-encoded entry per line
- `WalEntryKind`: `Proposal=0x01`, `Prevote=0x02`, `Precommit=0x03`, `Timeout=0x04`, `RoundStep=0x05`

## Interfaces and Contracts

**`BlockStore` (`src/storage/block_store.rs`):**
- `open(path: &Path) -> Result<Self, rocksdb::Error>` — creates DB and all CFs if missing
- `save_block(block, state_root, validator_set) -> Result<(), anyhow::Error>` — atomic batch
- `load_block(height) -> Result<Option<Block>, anyhow::Error>`
- `load_block_hash(height) -> Result<Option<Hash>, rocksdb::Error>`
- `load_state_root(height) -> Result<Option<Hash>, rocksdb::Error>`
- `load_validator_set(height) -> Result<Option<ValidatorSet>, anyhow::Error>`
- `last_height() -> Result<u64, rocksdb::Error>` — 0 if no blocks committed
- `prune_below(min_height) -> Result<u64, rocksdb::Error>` — deletes heights 1..min_height-1

**`StateStore` (`src/storage/state_store.rs`):**
- `save_state(height, state) -> Result<(), anyhow::Error>` — atomic batch
- `load_state(height) -> Result<Option<AppState>, anyhow::Error>`
- `load_latest_state() -> Result<Option<(u64, AppState)>, anyhow::Error>`
- `prune_below(min_height) -> Result<u64, rocksdb::Error>`

**`WAL` (`src/storage/wal.rs`):**
- `open(path: &Path) -> Result<Self, WalError>` — append-only file
- `write_entry(entry) -> Result<(), WalError>` — appends + flushes
- `read_all(path) -> Result<Vec<WalEntry>, WalError>` — reads + validates checksums
- `truncate() -> Result<(), WalError>` — reopens with `O_TRUNC`
- `truncate_below(min_height)` — reads, truncates, rewrites entries >= min_height

## Dependencies

**Internal modules:**
- `src/types/serialization.rs` — `encode_block` / `decode_block` for `BlockStore`
- `src/state/accounts.rs` — `AppState` serialized/deserialized by `StateStore`
- `src/crypto/hash.rs` — `sha256` for block hash and WAL checksum
- `src/types/validator.rs` — `ValidatorSet` serialized by `BlockStore`

**External crates:**
- `rocksdb v0.22` — `DB`, `WriteBatch`, `ColumnFamilyDescriptor`; opened with `create_if_missing` and `create_missing_column_families`

## Failure Modes and Edge Cases

- **RocksDB write failure:** `save_block` / `save_state` return `Err`; `CommandRouter` logs error but does not halt or retry — state and block store may diverge.
- **WAL write failure:** `CommandRouter` logs error but consensus continues — no crash recovery guarantee if flush fails.
- **Partial WAL entry:** `WalEntry::decode` returns `CorruptEntry` or `ChecksumMismatch`; `read_all` stops iteration at first failure (stop-on-corruption, not skip).
- **`decode_block` failure in `load_block`:** Returns `Err`; callers receive the error.
- **Block bytes encoding:** `encode_block` does not include `last_commit` field — `CommitInfo` is always `None` on decode. Pre-commit signatures are lost in storage.
- **Pruning while serving:** `prune_below` iterates height 1..min_height deleting one-by-one in individual batches — not atomic and not called anywhere in MVP.
- **WAL replay not implemented:** On startup, WAL file exists but is never read; consensus restarts fresh from the last committed block height without replaying any in-progress round data.

## Observability and Debugging

- `Metrics.storage_block_persist_duration` histogram updated by `CommandRouter` per PersistBlock.
- `Metrics.storage_wal_write_duration` histogram updated by `CommandRouter` per WriteWAL.
- `Metrics.storage_wal_entries` gauge never updated in MVP.
- Storage errors logged via `tracing::error!` in `CommandRouter`.
- Debugging: check `data/blocks/` and `data/state/` RocksDB dirs; `data/consensus.wal` for WAL entries.

## Risks and Notes

- `BlockStore` and `StateStore` are separate RocksDB instances — they are not written atomically. A crash between the two writes leaves them inconsistent.
- Full `AppState` JSON snapshot per height — storage grows linearly; no compaction or incremental storage.
- `encode_block` / `decode_block` drops `last_commit` (commit signatures) — historical blocks cannot be verified against their precommit quorum.
- WAL is written but never replayed — its crash-recovery purpose is not fulfilled in MVP.
- Pruning functions exist but are never called in `main.rs` or `CommandRouter`.

Changes:

