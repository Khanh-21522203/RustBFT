# RustBFT — Block Sync

**Purpose:** Define how a node that has fallen behind catches up to the current chain height by downloading and verifying committed blocks from peers.
**Audience:** Engineers implementing or reviewing the block sync subsystem.

---

## 1. Responsibilities

The block sync subsystem is responsible for:

1. **Lag detection** — determine at startup (and periodically) whether the local node is behind the network's latest committed height.
2. **Peer selection** — choose a sync peer that can supply the missing blocks.
3. **Block download** — request batches of committed blocks from the sync peer via P2P.
4. **Block verification** — for each received block, verify the `CommitInfo` contains valid precommit signatures from ≥2/3 of the validator set known at that height.
5. **Sequential application** — execute and persist each block in order, updating the state store and block store identically to the live consensus path.
6. **Handoff to consensus** — once the node has reached the network height, exit sync mode and enter the normal BFT consensus loop.

The block sync subsystem MUST NOT:

- Skip signature verification to go faster (no trusted sync modes in MVP).
- Re-enter consensus while still syncing.
- Request blocks from a peer without verifying that peer's identity via the handshake.
- Modify the validator set outside the normal `end_block` → `apply_updates` path.

---

## 2. Sync Architecture

```
                        ┌──────────────────────────────────────┐
                        │              Node Startup             │
                        │                                       │
                        │  1. WAL recovery → local_height H_L  │
                        │  2. Query peers → network_height H_N  │
                        │                                       │
                        │  if H_L < H_N: enter SYNC MODE       │
                        │  else:         enter CONSENSUS MODE   │
                        └──────────────┬───────────────────────┘
                                       │
                    ┌──────────────────▼──────────────────────┐
                    │               SYNC MODE                  │
                    │                                          │
                    │  SyncManager                             │
                    │  ┌──────────────────────────────────┐   │
                    │  │ while local_height < target:      │   │
                    │  │   batch = request_blocks(         │   │
                    │  │     peer, local_height+1,         │   │
                    │  │     min(local_height+BATCH, target│   │
                    │  │   )                               │   │
                    │  │   for block in batch:             │   │
                    │  │     verify_commit_info(block)     │   │
                    │  │     execute_block(block)          │   │
                    │  │     persist_block(block)          │   │
                    │  │     local_height += 1             │   │
                    │  └──────────────────────────────────┘   │
                    │                                          │
                    └──────────────────┬───────────────────────┘
                                       │
                    ┌──────────────────▼──────────────────────┐
                    │           CONSENSUS MODE                  │
                    │  (normal BFT loop, starts at H_L + 1)   │
                    └──────────────────────────────────────────┘
```

---

## 3. P2P Protocol Extension

Two new message types are added to the existing P2P message set:

```
MsgType:
    0x08  SyncRequest   { from_height: u64, to_height: u64 }
    0x09  SyncResponse  { blocks: Vec<Block> }
    0x0A  StatusRequest {}
    0x0B  StatusResponse { latest_height: u64, node_id: ValidatorId }
```

### 3.1 StatusRequest / StatusResponse

On connecting to a peer, the node immediately sends a `StatusRequest`. The peer responds with its `latest_height`. This is used for lag detection and peer selection.

### 3.2 SyncRequest / SyncResponse

`SyncRequest { from_height, to_height }` asks the peer to return blocks in the inclusive range `[from_height, to_height]`. The batch size is capped at `SYNC_BATCH_SIZE = 64` blocks per request to bound memory usage. The peer may return fewer blocks than requested (e.g. it does not have all of them yet); the syncing node must handle partial responses.

---

## 4. Block Verification

Each downloaded block must pass the following checks before being executed:

```
fn verify_synced_block(block: &Block, commit_info: &CommitInfo, vset: &ValidatorSet):
    // 1. Height is exactly local_height + 1
    assert block.header.height == local_height + 1

    // 2. CommitInfo refers to the same block hash
    expected_hash = sha256(canonical_encode(block))
    assert commit_info.block_hash == expected_hash

    // 3. CommitInfo has ≥ 2/3 precommit power from the PREVIOUS validator set
    //    (the vset that was active when this block was proposed)
    power = 0
    for sig in commit_info.signatures:
        assert sig.vote.vote_type == Precommit
        assert sig.vote.height == block.header.height
        assert sig.vote.block_hash == Some(expected_hash)
        assert ed25519_verify(vset.get(sig.vote.validator).public_key, sig)
        power += vset.voting_power(sig.vote.validator)
    assert power >= quorum_threshold(vset.total_power())

    // 4. prev_block_hash matches the hash of the previous committed block
    if block.header.height > 1:
        assert block.header.prev_block_hash == stored_block_hash(block.header.height - 1)
```

Verification uses the validator set that was **active during that block's consensus round**, which is stored in the block store at `height - 1`.

---

## 5. Sync State Machine

```
States:
    Idle         → not syncing (node is at network height)
    Discovering  → querying peer statuses to find target height
    Syncing      → actively downloading and applying blocks
    Done         → reached target; hand off to consensus

Transitions:
    Idle       → Discovering   on startup or when peer reports higher height
    Discovering→ Syncing       when target_height is known and target > local_height
    Discovering→ Idle          when no peer has a higher height
    Syncing    → Syncing       after each batch is applied (local_height < target)
    Syncing    → Done          when local_height == target
    Syncing    → Discovering   on peer failure (select new peer, re-assess target)
    Done       → Idle          consensus loop takes over
```

---

## 6. Integration Points

| Component | Interaction |
|---|---|
| **Startup (`main.rs`)** | After WAL recovery, `SyncManager::run()` is called before spawning the consensus thread. Consensus thread is only spawned after sync completes. |
| **P2P Manager** | `SyncManager` opens a dedicated sync connection to the chosen peer (does not interfere with gossip connections). |
| **Block Store** | `SyncManager` calls `block_store.save_block()` for each verified block, identical to the live consensus path. |
| **State Store** | `SyncManager` calls `state_store.save_state()` after each block execution. |
| **State Executor** | `SyncManager` calls `executor.execute_block()` for each block in order. |
| **Validator Set** | After each block's `end_block`, validator updates are applied to the in-memory set before processing the next block. |
| **WAL** | WAL is NOT written during sync (blocks are already committed; WAL is only for in-progress consensus state). |

---

## 7. Failure Handling

| Failure | Response |
|---|---|
| Peer returns invalid block | Disconnect peer, select another, retry from last good height |
| Signature verification fails | Disconnect peer (Byzantine behavior), select another |
| Peer returns empty response | Wait with backoff, retry or select another peer |
| All peers exhausted | Wait and retry periodically (peer reconnect may add new peers) |
| Block execution fails (state mismatch) | This is an S0 critical failure → call `halt()` |
| Node restarts during sync | WAL is clean (not written during sync); restart resumes sync from `block_store.last_height()` |

---

## 8. Configuration

```toml
[sync]
enabled = true
batch_size = 64                  # blocks per SyncRequest
peer_timeout_ms = 10000          # timeout waiting for SyncResponse
max_retries_per_peer = 3         # before selecting a different peer
status_broadcast_interval_ms = 5000  # how often to broadcast StatusResponse
```

---

## 9. Non-Responsibilities

- **State sync** — downloading only the state snapshot at a recent height (no block re-execution). Deferred post-MVP.
- **Light client sync** — verifying only block headers with a trusted validator set. Deferred post-MVP.
- **Block sync during live consensus** — if a node falls one or two heights behind while running, it catches up via the normal consensus path (receiving proposals and precommits). Block sync is only for gaps larger than what live gossip can cover.
- **Parallel block download** — MVP downloads blocks sequentially from a single peer. Parallel download from multiple peers is a future optimization.

---

## 10. Security Considerations

- **Every downloaded block must have its `CommitInfo` verified** before execution. Accepting a block without ≥2/3 precommit power allows a single malicious peer to force the node to execute an invalid state transition.
- **The sync peer is chosen from authenticated peers only** (peers that have completed the Ed25519 handshake).
- **A malicious peer can slow sync** by returning valid but old blocks, or by timing out. The timeout and retry mechanism ensures the node eventually switches peers.
- **The node does not expose a "trusted sync" mode** that skips signature verification. All synced blocks go through the full verification pipeline.

---

## 11. Observability

| Metric | Type | Description |
|---|---|---|
| `rustbft_sync_mode` | Gauge (0/1) | 1 when node is in sync mode |
| `rustbft_sync_target_height` | Gauge | The height the node is syncing toward |
| `rustbft_sync_blocks_applied_total` | Counter | Total blocks applied during sync |
| `rustbft_sync_peer_failures_total` | Counter | Times a sync peer was abandoned |
| `rustbft_sync_duration_seconds` | Histogram | Time to complete a full sync run |

Structured log events:

```
INFO "Starting block sync"   local_height=42 target_height=200 peer="node1"
INFO "Sync batch applied"    from=43 to=106 elapsed_ms=1240
INFO "Sync complete"         final_height=200 elapsed_seconds=8
WARN "Sync peer failed"      peer="node2" reason="timeout" retrying_with="node3"
ERROR "Block verification failed during sync"  height=99 peer="node2"  → halt
```
