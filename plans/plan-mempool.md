# Feature: Mempool

## 1. Purpose

The Mempool is the transaction intake buffer. It accepts signed transactions from RPC clients and P2P gossip, validates them stateless-ly (signature, format, gas limit), enforces size caps, and provides ordered transaction batches to the consensus proposer on demand. After a block is committed, it evicts the included transactions.

The mempool is not on the consensus critical path — a bug here can degrade throughput but cannot violate safety. However, it must be efficient because the proposer waits for it to respond before broadcasting a proposal.

## 2. Responsibilities

- Accept `SignedTransaction` from RPC (via channel) and P2P gossip
- Perform stateless validation: Ed25519 signature, format, `gas_limit > 0`, `gas_limit ≤ max_gas_limit`, size ≤ `max_tx_bytes`
- Reject duplicate transactions (same `tx_hash` already in pool)
- Enforce configurable capacity: maximum transaction count and maximum total bytes
- Reap: return an ordered subset of transactions up to a byte budget, for block proposal
- Evict: remove transactions whose hashes appear in a newly committed block
- Gossip signal: when a new transaction is accepted from RPC, signal P2P to broadcast it
- Expose pool size metrics

## 3. Non-Responsibilities

- No stateful validation (nonce ordering, balance check) — that is the state machine's job during `DeliverTx`
- No transaction prioritization by gas price — FIFO ordering for MVP
- No replay protection across heights (nonce checking is stateful, done at execution time)
- No transaction expiry by TTL for MVP

## 4. Architecture Design

```
Client (HTTP)              Peer (P2P)
     |                          |
     v                          v
 RPC Layer              P2P Networking
     |                          |
     +---[MempoolCommand]-------+
                 |
         +-------v--------+
         |    Mempool      |
         |  (sync thread  |
         |   or Tokio     |
         |   task)        |
         |                |
         |  BTreeMap txs  |
         |  by_sender idx |
         +-------+--------+
                 |
     +-----------+----------+
     |                      |
ReapTxs response       EvictTxs
     |                      |
Consensus Core         (after commit)
```

## 5. Core Data Structures (Rust)

```rust
// src/mempool/pool.rs

pub struct Mempool {
    // Ordered by tx_hash for deterministic iteration (BTreeMap, never HashMap)
    txs: BTreeMap<Hash, MempoolEntry>,
    // Per-sender nonce ordering index: Address → set of (nonce, tx_hash)
    by_sender: BTreeMap<Address, BTreeSet<(u64, Hash)>>,
    total_bytes: usize,
    config: MempoolConfig,
}

pub struct MempoolEntry {
    pub tx: SignedTransaction,
    pub tx_hash: Hash,
    pub received_at: std::time::Instant,
    pub source: TxSource,
    pub size_bytes: usize,
}

pub enum TxSource { Rpc, P2p }

pub struct MempoolConfig {
    pub max_txs: usize,         // default 5_000
    pub max_bytes: usize,       // default 10 MB
    pub max_tx_bytes: usize,    // default 64 KB per tx
    pub max_gas_limit: u64,     // ceiling per tx gas_limit
}

pub struct MempoolStats {
    pub tx_count: usize,
    pub total_bytes: usize,
}

pub enum AddError {
    Duplicate,
    InvalidSignature,
    InvalidFormat,
    TxTooLarge,
    ZeroGasLimit,
    GasLimitExceeded,
    MempoolFull,
}

// Channel-based interface (src/mempool/mod.rs)
pub enum MempoolCommand {
    AddTx {
        tx: SignedTransaction,
        source: TxSource,
        responder: oneshot::Sender<Result<Hash, AddError>>,
    },
    ReapTxs {
        max_bytes: usize,
        responder: oneshot::Sender<Vec<SignedTransaction>>,
    },
    EvictTxs {
        tx_hashes: Vec<Hash>,
    },
    GetStats {
        responder: oneshot::Sender<MempoolStats>,
    },
}
```

## 6. Public Interfaces

```rust
// src/mempool/mod.rs

/// Start the mempool on a dedicated thread.
/// Returns a Sender that all producers (RPC, P2P) use to submit commands.
pub fn start(config: MempoolConfig) -> crossbeam::channel::Sender<MempoolCommand>;

// Mempool internal API
impl Mempool {
    pub fn new(config: MempoolConfig) -> Self;
    pub fn add_tx(&mut self, tx: SignedTransaction, source: TxSource) -> Result<Hash, AddError>;
    pub fn reap(&self, max_bytes: usize) -> Vec<SignedTransaction>;
    pub fn evict(&mut self, tx_hashes: &[Hash]);
    pub fn stats(&self) -> MempoolStats;
    pub fn contains(&self, tx_hash: &Hash) -> bool;
}

// Stateless validation
fn validate_tx(tx: &SignedTransaction, config: &MempoolConfig) -> Result<(), AddError>;
```

## 7. Internal Algorithms

### Stateless Validation
```
fn validate_tx(tx, config):
    size = tx.canonical_encode().len()
    if size > config.max_tx_bytes:     return TxTooLarge
    if tx.gas_limit() == 0:            return ZeroGasLimit
    if tx.gas_limit() > max_gas_limit: return GasLimitExceeded
    pubkey = recover_pubkey_from_address_and_sig(tx)?
    if !verify(pubkey, tx.signing_bytes(), tx.signature): return InvalidSignature
    Ok(())
```

### Add Transaction
```
fn add_tx(tx, source):
    tx_hash = sha256(tx.canonical_encode())
    if txs.contains_key(tx_hash): return Err(Duplicate)
    validate_tx(tx)?
    size = tx.canonical_encode().len()
    if txs.len() >= config.max_txs:           return Err(MempoolFull)
    if total_bytes + size > config.max_bytes:  return Err(MempoolFull)
    by_sender[tx.from()].insert((tx.nonce(), tx_hash))
    txs.insert(tx_hash, MempoolEntry{tx, tx_hash, ...})
    total_bytes += size
    Ok(tx_hash)
```

### Reap
```
fn reap(max_bytes):
    // Collect transactions ordered by insertion sequence within sender nonce order
    // Simple MVP strategy: iterate txs (BTreeMap, deterministic) and take greedily
    result = []
    bytes_used = 0
    for (_, entry) in txs.iter():
        if bytes_used + entry.size_bytes > max_bytes: continue
        result.push(entry.tx.clone())
        bytes_used += entry.size_bytes
    result
```

The reap ordering is FIFO (by tx_hash-sorted iteration for determinism). A more sophisticated strategy (e.g., priority by nonce within sender groups) can be added later without changing the interface.

### Evict
```
fn evict(tx_hashes):
    for hash in tx_hashes:
        if let Some(entry) = txs.remove(hash):
            total_bytes -= entry.size_bytes
            let nonce = entry.tx.nonce()
            let from  = entry.tx.from()
            by_sender.entry(from).and_modify(|s| { s.remove(&(nonce, hash)); })
            if by_sender[from].is_empty(): by_sender.remove(from)
```

### Mempool Thread Loop
```
fn run(rx: Receiver<MempoolCommand>, config: MempoolConfig):
    pool = Mempool::new(config)
    while let Ok(cmd) = rx.recv():
        match cmd:
            AddTx { tx, source, responder }  => responder.send(pool.add_tx(tx, source))
            ReapTxs { max_bytes, responder } => responder.send(pool.reap(max_bytes))
            EvictTxs { tx_hashes }           => pool.evict(&tx_hashes)
            GetStats { responder }           => responder.send(pool.stats())
```

## 8. Persistence Model

No persistence. The mempool is in-memory only. Transactions are lost on restart. Clients must resubmit unincluded transactions after a node restart. This is acceptable for MVP; a persistent mempool is a future enhancement.

## 9. Concurrency Model

The mempool runs on a single dedicated thread. All concurrent access is via a `crossbeam::channel::Receiver<MempoolCommand>` (multiple producers, one consumer). Producers hold `crossbeam::channel::Sender<MempoolCommand>` clones. No `Mutex`, no `RwLock`, no shared mutable state.

The `oneshot` channel in `AddTx` and `ReapTxs` commands allows producers to await responses without blocking the mempool thread.

## 10. Configuration

```toml
[mempool]
max_txs = 5000
max_bytes = 10485760       # 10 MB total
max_tx_bytes = 65536       # 64 KB per transaction
max_gas_limit = 10000000   # matches chain_params.max_block_gas
```

## 11. Observability

- `rustbft_mempool_size` (Gauge) — current transaction count
- `rustbft_mempool_bytes` (Gauge) — current total bytes
- `rustbft_mempool_txs_added_total{source}` (Counter) — accepted transactions by source (rpc/p2p)
- `rustbft_mempool_txs_rejected_total{reason}` (Counter) — rejected transactions by error type
- `rustbft_mempool_txs_evicted_total` (Counter) — evicted after commit
- `rustbft_mempool_reap_duration_seconds` (Histogram) — time to respond to a ReapTxs command
- Log DEBUG for each accepted transaction; WARN when pool is at 90% capacity

## 12. Testing Strategy

- **`test_add_valid_tx`**: valid tx added, hash returned, `stats()` shows count=1
- **`test_duplicate_rejected`**: add same tx twice → second call returns `Err(Duplicate)`
- **`test_invalid_signature_rejected`**: tx with wrong signature → `Err(InvalidSignature)`
- **`test_zero_gas_limit_rejected`**: tx with `gas_limit=0` → `Err(ZeroGasLimit)`
- **`test_tx_too_large`**: tx exceeding `max_tx_bytes` → `Err(TxTooLarge)`
- **`test_max_tx_count_enforced`**: add `max_txs + 1` transactions → last returns `Err(MempoolFull)`
- **`test_max_bytes_enforced`**: add transactions until bytes exceed `max_bytes` → last rejected
- **`test_reap_respects_max_bytes`**: reap with limit of 1000 bytes → result always ≤ 1000 bytes
- **`test_evict_removes_txs`**: add 5 txs, evict 3 by hash, assert `stats().tx_count == 2`
- **`test_evict_unknown_hash_noop`**: evict non-existent hash → no panic, pool unchanged
- **`test_reap_then_evict`**: reap txs, commit them, evict → pool shrinks correctly
- **`test_btreemap_order_deterministic`**: two identical pools iterate in the same order

## 13. Open Questions

None.
