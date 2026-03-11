# Feature: Block Sync

## 1. Purpose

The Block Sync module enables a node that has fallen behind the network — due to a crash, maintenance window, network partition, or fresh installation — to download, verify, and apply all committed blocks it missed before re-entering the BFT consensus loop. Without this module, any node restart after more than one committed block is missed leaves the node permanently stranded at its last known height.

## 2. Responsibilities

- Detect at startup whether the local node is behind the network by querying connected peers for their `latest_height` via `StatusRequest`/`StatusResponse` P2P messages
- Select a sync peer from authenticated connected peers that has a higher `latest_height`
- Download committed blocks in batches of up to `SYNC_BATCH_SIZE = 64` via `SyncRequest`/`SyncResponse` P2P messages
- Verify each downloaded block's `CommitInfo`: ≥2/3 precommit power from the validator set active at that height, correct block hash, valid Ed25519 signatures
- Execute each verified block through `StateExecutor::execute_block()` and persist it via `BlockStore::save_block()` and `StateStore::save_state()`, applying validator updates at each height boundary
- Hand off to the consensus thread once `local_height == network_height`
- Add two new P2P message types (`SyncRequest`, `SyncResponse`) and two status messages (`StatusRequest`, `StatusResponse`) to the existing message set
- Emit Prometheus metrics: `rustbft_sync_mode`, `rustbft_sync_target_height`, `rustbft_sync_blocks_applied_total`, `rustbft_sync_peer_failures_total`, `rustbft_sync_duration_seconds`

## 3. Non-Responsibilities

- Does not implement state sync (downloading only a state snapshot without re-executing blocks)
- Does not implement light client sync (trusting a recent validator set checkpoint without full block verification)
- Does not handle syncing while live consensus is running (live consensus gossip handles single-height gaps; block sync handles multi-height gaps at startup)
- Does not implement parallel block download from multiple peers (MVP: single peer, sequential)
- Does not implement a "trusted sync" mode that skips signature verification

## 4. Architecture Design

```
Node Startup Sequence:
  WAL recovery → local_height = H_L
       │
       ▼
  SyncManager::discover()
    ├── send StatusRequest to all connected peers
    ├── collect StatusResponse { latest_height } responses
    └── target_height = max(peer.latest_height)
       │
       ├── if target_height <= H_L → skip sync → spawn consensus
       │
       ▼
  SyncManager::run(peer, from=H_L+1, target=target_height)
    ┌─────────────────────────────────────────────┐
    │ loop while local_height < target:            │
    │   batch_end = min(local_height + 64, target) │
    │   send SyncRequest { local_height+1,         │
    │                      batch_end }             │
    │   recv SyncResponse { blocks }               │
    │   for block in blocks:                       │
    │     verify_commit_info(block, vset_at_H-1)   │
    │     execute_block(block) → state_root        │
    │     assert state_root == block.header.state_root │
    │     save_block(block)                        │
    │     save_state(local_height, state)          │
    │     apply validator_updates (H → H+1)        │
    │     local_height += 1                        │
    └─────────────────────────────────────────────┘
       │
       ▼
  Spawn consensus thread at local_height + 1

P2P message additions:
    0x08  SyncRequest   { from_height: u64, to_height: u64 }
    0x09  SyncResponse  { blocks: Vec<Block> }
    0x0A  StatusRequest {}
    0x0B  StatusResponse { latest_height: u64, node_id: ValidatorId }
```

## 5. Core Data Structures (Rust)

```rust
// src/sync/mod.rs

pub struct SyncConfig {
    pub batch_size: u64,           // blocks per SyncRequest (default 64)
    pub peer_timeout_ms: u64,      // timeout for SyncResponse (default 10_000)
    pub max_retries_per_peer: u32, // before switching peers (default 3)
    pub status_broadcast_interval_ms: u64, // how often to re-broadcast status (default 5_000)
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            batch_size: 64,
            peer_timeout_ms: 10_000,
            max_retries_per_peer: 3,
            status_broadcast_interval_ms: 5_000,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("no peers available for sync")]
    NoPeers,
    #[error("peer timeout after {peer_timeout_ms}ms")]
    PeerTimeout { peer_timeout_ms: u64 },
    #[error("block verification failed at height {height}: {reason}")]
    VerificationFailed { height: u64, reason: String },
    #[error("block execution failed at height {height}: {reason}")]
    ExecutionFailed { height: u64, reason: String },
    #[error("all peers exhausted")]
    AllPeersExhausted,
    #[error("p2p error: {0}")]
    P2p(String),
}

pub struct SyncStatus {
    pub is_syncing: bool,
    pub local_height: u64,
    pub target_height: u64,
    pub sync_peer: Option<ValidatorId>,
}

// src/p2p/msg.rs additions

pub enum NetworkMessage {
    // ... existing variants ...
    StatusRequest,
    StatusResponse { latest_height: u64, node_id: ValidatorId },
    SyncRequest { from_height: u64, to_height: u64 },
    SyncResponse { blocks: Vec<Block> },
}
```

## 6. Public Interfaces

```rust
// src/sync/mod.rs

pub struct SyncManager {
    cfg: SyncConfig,
    block_store: Arc<BlockStore>,
    state_store: Arc<StateStore>,
    executor: StateExecutor,
    metrics: Arc<Metrics>,
}

impl SyncManager {
    pub fn new(
        cfg: SyncConfig,
        block_store: Arc<BlockStore>,
        state_store: Arc<StateStore>,
        executor: StateExecutor,
        metrics: Arc<Metrics>,
    ) -> Self;

    /// Query all peers for their latest_height.
    /// Returns the highest height seen and the peer that has it.
    /// Returns Ok(None) if no peer has a higher height than local.
    pub async fn discover(
        &self,
        peers: &BTreeMap<ValidatorId, mpsc::Sender<NetworkMessage>>,
        local_height: u64,
        timeout_ms: u64,
    ) -> Result<Option<(u64, ValidatorId)>, SyncError>;

    /// Run the full sync loop: download, verify, execute, persist all blocks
    /// from `local_height + 1` to `target_height`.
    /// Returns the final height reached (== target_height on success).
    pub async fn run(
        &self,
        peer_tx: mpsc::Sender<NetworkMessage>,
        peer_rx: &mut mpsc::Receiver<NetworkMessage>,
        local_height: u64,
        target_height: u64,
        app_state: &mut AppState,
        contracts: &mut ContractRuntime,
        validator_set: &mut ValidatorSet,
    ) -> Result<u64, SyncError>;
}

// src/sync/verify.rs

/// Verify that block has valid CommitInfo signed by ≥2/3 of vset.
pub fn verify_commit_info(
    block: &Block,
    commit_info: &CommitInfo,
    vset: &ValidatorSet,
    prev_block_hash: Hash,
) -> Result<(), SyncError>;
```

## 7. Internal Algorithms

### Lag Detection
```
fn discover(peers, local_height, timeout_ms):
    // Send StatusRequest to all peers concurrently
    for (peer_id, tx) in peers:
        tx.send(NetworkMessage::StatusRequest)

    // Collect responses within timeout
    responses = collect_with_timeout(timeout_ms)
    if responses.is_empty():
        return Ok(None)  // no peers responded; skip sync

    // Pick peer with highest height
    (best_height, best_peer) = responses.iter().max_by_key(|(h, _)| h)
    if best_height <= local_height:
        return Ok(None)  // already up to date

    return Ok(Some((best_height, best_peer)))
```

### Sync Loop
```
fn run(peer_tx, peer_rx, local_height, target_height,
       app_state, contracts, validator_set):

    current = local_height
    retries = 0

    while current < target_height:
        batch_end = min(current + cfg.batch_size, target_height)

        // Request batch
        peer_tx.send(SyncRequest { from_height: current + 1, to_height: batch_end })

        // Wait for response with timeout
        response = peer_rx.recv_timeout(cfg.peer_timeout_ms)?
            else:
                retries += 1
                if retries >= cfg.max_retries_per_peer:
                    return Err(SyncError::PeerTimeout)
                continue

        retries = 0
        blocks = response.blocks
        if blocks.is_empty():
            // Peer doesn't have these blocks yet; wait and retry
            sleep(500ms)
            continue

        for block in blocks:
            // Load the validator set that was active for this block
            // (stored at height - 1 in block_store, or genesis for height 1)
            prev_vset = if block.header.height == 1:
                validator_set.clone()
            else:
                block_store.load_validator_set(block.header.height - 1)?

            // 1. Verify CommitInfo
            verify_commit_info(&block, &block.last_commit.unwrap(), &prev_vset, prev_hash)?

            // 2. Execute block (deterministic, same as live path)
            snap = app_state.snapshot()
            (state_root, val_updates) = executor.execute_block(
                app_state, contracts, &block)?

            // 3. Verify state root matches header
            if state_root != block.header.state_root:
                halt("state root mismatch during sync at height {}", block.header.height)

            // 4. Persist
            block_store.save_block(&block, state_root, validator_set)?
            state_store.save_state(block.header.height, app_state)?

            // 5. Apply validator updates (H → H+1)
            if !val_updates.is_empty():
                *validator_set = validator_set.apply_updates(
                    &val_updates, &ValidatorSetPolicy::default())?

            current = block.header.height
            metrics.sync_blocks_applied.inc()

    metrics.sync_mode.set(0)
    Ok(current)
```

### CommitInfo Verification
```
fn verify_commit_info(block, commit_info, vset, prev_block_hash):
    // 1. Height must match
    if commit_info.height != block.header.height:
        return Err(VerificationFailed)

    // 2. CommitInfo block_hash must match canonical hash of block
    expected_hash = sha256(canonical_encode(block))
    if commit_info.block_hash != expected_hash:
        return Err(VerificationFailed { reason: "block hash mismatch" })

    // 3. prev_block_hash in header must chain correctly
    if block.header.height > 1:
        if block.header.prev_block_hash != prev_block_hash:
            return Err(VerificationFailed { reason: "prev_block_hash mismatch" })

    // 4. Sum precommit power; each sig must be valid
    power = 0
    seen = BTreeSet::new()
    for sig in commit_info.signatures:
        if sig.vote.vote_type != Precommit: return Err(...)
        if sig.vote.height != block.header.height: return Err(...)
        if sig.vote.block_hash != Some(expected_hash): return Err(...)
        if seen.contains(sig.vote.validator): continue  // deduplicate
        if !ed25519_verify(vset.get(sig.vote.validator)?.public_key,
                           canonical_encode(&sig.vote), sig.signature):
            return Err(VerificationFailed { reason: "invalid signature" })
        power += vset.voting_power(&sig.vote.validator)
        seen.insert(sig.vote.validator)

    thr = quorum_threshold(vset.total_power())
    if power < thr:
        return Err(VerificationFailed { reason: "insufficient precommit power" })

    Ok(())
```

### Peer Handler (responding to SyncRequest)
```
// In P2P peer reader task, handle SyncRequest from a syncing peer
fn handle_sync_request(req: SyncRequest, block_store: &BlockStore) -> SyncResponse:
    let mut blocks = Vec::new()
    let to = min(req.to_height, req.from_height + SYNC_BATCH_SIZE - 1)

    for h in req.from_height..=to:
        match block_store.load_block(h):
            Some(block) => blocks.push(block)
            None => break  // don't have this height yet

    SyncResponse { blocks }
```

## 8. Persistence Model

Block sync writes to the same storage as live consensus:
- `BlockStore::save_block()` — persists each synced block with its state root and validator set
- `StateStore::save_state()` — persists the application state snapshot at each height

The WAL is **not written** during sync. Blocks are already committed; WAL is only for in-progress consensus state. If the node crashes during sync, it resumes from `block_store.last_height()` on the next startup — the partially synced blocks already committed to RocksDB are valid.

## 9. Concurrency Model

Block sync runs **synchronously on the main thread** before the consensus or Tokio async subsystems are started. The sync loop is a simple sequential `async` loop:

```
tokio::runtime::Runtime::new()
    .block_on(sync_manager.run(...))
```

The P2P connection to the sync peer is a dedicated TCP connection, separate from the gossip peer connections. The sync peer's responses are received on a dedicated `mpsc::Receiver`.

The consensus thread is only spawned **after** `SyncManager::run()` returns successfully.

## 10. Configuration

```toml
# node.toml — sync section
[sync]
enabled = true
batch_size = 64                        # max blocks per SyncRequest
peer_timeout_ms = 10000                # timeout waiting for SyncResponse
max_retries_per_peer = 3               # failures before selecting new peer
status_broadcast_interval_ms = 5000   # StatusResponse broadcast cadence
```

Environment variable overrides:
```
RUSTBFT_SYNC_ENABLED=false            # disable sync (single-node dev)
RUSTBFT_SYNC_BATCH_SIZE=128
RUSTBFT_SYNC_PEER_TIMEOUT_MS=30000
```

## 11. Observability

Prometheus metrics:
```
rustbft_sync_mode                    Gauge    1 while syncing, 0 otherwise
rustbft_sync_target_height           Gauge    target height for current sync run
rustbft_sync_blocks_applied_total    Counter  total blocks applied across all sync runs
rustbft_sync_peer_failures_total     Counter  peer failures (timeout / verification error)
rustbft_sync_duration_seconds        Histogram time for a complete sync run
```

Structured log events (logged at INFO):
```
INFO "Starting block sync"    local_height=42 target_height=200 peer="node1"
INFO "Sync batch applied"     from=43 to=106 elapsed_ms=1240
INFO "Sync complete"          final_height=200 elapsed_seconds=8
WARN "Sync peer failed"       peer="node2" reason="timeout" retrying_with="node3"
ERROR "Sync verification failed" height=99 peer="node2" reason="invalid signature" → halt
```

## 12. Testing Strategy

- **`test_sync_single_batch`**: set up two nodes; node B has blocks 1–10, node A has 0 → A syncs from B → A's `last_height` == 10, state roots match
- **`test_sync_multi_batch`**: node B has 200 blocks, batch_size=64 → A makes 4 requests, all succeed, final height=200
- **`test_sync_partial_response`**: peer returns fewer blocks than requested → sync continues from where it left off on the next request
- **`test_sync_invalid_commit_info_signature`**: peer returns block with a forged precommit signature → `VerificationFailed`, peer is abandoned
- **`test_sync_insufficient_precommit_power`**: peer returns block with only 1/3 precommit power → `VerificationFailed`
- **`test_sync_prev_hash_mismatch`**: block chain breaks at height 5 (wrong `prev_block_hash`) → `VerificationFailed`
- **`test_sync_state_root_mismatch`**: block executes to different root than header claims → `halt()` triggered
- **`test_sync_peer_timeout`**: peer does not respond within `peer_timeout_ms` → retry, then `AllPeersExhausted`
- **`test_sync_peer_switch`**: first peer times out; second peer has all blocks → sync succeeds
- **`test_sync_resumes_after_crash`**: crash during sync at height 50; on restart `block_store.last_height()` == 50 → sync resumes from 51
- **`test_sync_already_up_to_date`**: local height == peer height → `discover()` returns `None`, consensus starts immediately
- **`test_sync_no_peers`**: no peers connected → `discover()` returns `NoPeers`, consensus starts (single-node mode)
- **`test_sync_validator_set_transition`**: validator set changes at height 10; blocks 11+ are verified against the updated set → correct
- **`test_sync_consensus_handoff`**: after sync completes, consensus thread starts at the correct height and commits the next block

## 13. Open Questions

- **Sync peer selection strategy**: MVP picks the peer with the highest `latest_height`. A smarter strategy would prefer peers with low latency or historical reliability. Deferred post-MVP.
- **Parallel block download**: downloading from multiple peers simultaneously would improve sync speed for nodes far behind. This requires reassembly ordering and is deferred post-MVP.
- **State sync (snapshot sync)**: for nodes that are thousands of blocks behind, re-executing every block is very slow. State sync (downloading a recent state snapshot and only replaying recent blocks) would drastically reduce catchup time. Deferred post-MVP.
- **StatusResponse broadcast during live consensus**: should a running node periodically broadcast its `StatusResponse` so syncing nodes can discover it without sending `StatusRequest`? Adds a small gossip overhead but simplifies discovery. Deferred post-MVP.
