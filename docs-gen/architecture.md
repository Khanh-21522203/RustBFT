# RustBFT Architecture Reference

This document describes the internal architecture of RustBFT: threading model, component boundaries, channel topology, consensus invariants, and crash-recovery design.

---

## Threading Model

RustBFT uses a **hybrid threading model**: a single synchronous OS thread owns the consensus state machine; all I/O is handled by a multi-threaded Tokio async runtime.

```
OS Thread (sync)          Tokio Runtime (async)
─────────────────         ──────────────────────────────────
ConsensusCore.run()       P2pManager.run()
  ↑ crossbeam recv        TimerService.run()
  ↓ crossbeam send        CommandRouter.run() ← crossbeam recv (blocking_recv)
                          RpcServer.run()
                          MetricsServer.run()
```

**Why a dedicated sync thread for consensus?**

- The consensus state machine is a pure functional loop over events with zero I/O. It should never block on I/O or async operations.
- Keeping it on one thread eliminates the need for locks on consensus state.
- crossbeam bounded channels provide backpressure between the sync core and the async shell.

---

## Component Boundaries

The architecture enforces a strict layering rule:

> **Consensus (`src/consensus/`) must NOT import state, storage, or contracts.**

Consensus communicates with those subsystems exclusively through `ConsensusCommand` messages sent to the `CommandRouter`. This boundary is upheld by Rust's module system — consensus has no direct `use crate::state::*` or `use crate::storage::*` imports.

| Layer | Module | May Import |
|-------|--------|-----------|
| Types / Crypto | `types/`, `crypto/` | (nothing above) |
| Storage | `storage/` | types, crypto |
| State Machine | `state/`, `contracts/` | types, crypto, storage |
| Consensus | `consensus/` | types, crypto only |
| Router | `router/` | all of the above |
| P2P | `p2p/` | types, crypto, consensus events |
| RPC | `rpc/` | types, state (read-only), storage |
| Metrics / Config | `metrics/`, `config/` | (nothing above) |

---

## Channel Topology

All inter-component communication uses bounded channels to provide backpressure.

```
                         crossbeam bounded(256)
  P2P inbound ──────────────────────────────────────────▶┐
  Timer fires ──────────────────────────────────────────▶│
                                                         │ ConsensusEvent
                                                    ┌────▼────────────────┐
                                                    │   ConsensusCore     │
                                                    │   (sync, 1 thread)  │
                                                    └────┬────────────────┘
                         crossbeam bounded(1024)         │ ConsensusCommand
  ┌─────────────────────────────────────────────────────◀┘
  │ CommandRouter (Tokio)
  │
  ├──mpsc(1024)──▶ P2pManager       (BroadcastProposal, BroadcastVote)
  ├──mpsc(256)───▶ TimerService      (ScheduleTimeout, CancelTimeout)
  ├──inline──────▶ StateExecutor     (ExecuteBlock — blocks router task)
  ├──inline──────▶ BlockStore        (PersistBlock)
  ├──inline──────▶ StateStore        (save_state after persist)
  └──inline──────▶ WAL               (WriteWAL — sync append to disk)
```

Channel capacities (source: `src/main.rs`):
- `ConsensusEvent` rx: 256
- `ConsensusCommand` rx: 1024
- P2P outbound per peer: `cfg.outbound_buffer_size` (default 512)
- Gossip fanout: 2048
- Peer control: 256

---

## Consensus State Machine

### States

```
NewHeight ──▶ Propose ──▶ Prevote ──▶ Precommit ──▶ Commit ──▶ NewHeight
                │              │              │
             timeout        timeout        timeout
                │              │              │
                ▼              ▼              ▼
            Prevote(nil)  Precommit(nil)  next round
```

Defined in `src/consensus/state.rs:Step`.

### Key Invariants

| Invariant | Enforced at |
|-----------|-------------|
| I1: prevote at most once per round | `prevoted_this_round` flag |
| I2: precommit at most once per round | `precommitted_this_round` flag |
| I3: only accept one proposal per round | early return if `self.proposal.is_some()` |
| I4: locked block only changes forward | locked/valid updated only on polka |
| I5: votes from past heights discarded | height check in `handle_vote` |
| I6: equivocation detected and logged | `VoteSet::insert_vote` returns `Equivocation` |

### Round Skip (f+1 rule)

If `>1/3` of total voting power has votes at a higher round, the node fast-forwards to that round without waiting for a timeout. This prevents a lagging node from blocking liveness.

Implemented in `src/consensus/state.rs:check_round_skip`.

### Cross-Height Commit (any-round precommit)

On every new vote received, the node checks all rounds (not just current) for a `>2/3` precommit quorum. If found, the node commits immediately regardless of its current step.

Implemented in `src/consensus/state.rs:check_any_round_commit`.

### Proposer Selection

Weighted round-robin selection. The proposer for `(height H, round R)` is determined by:
```
index = (H + R) % total_validators
```
(simplified; actual implementation in `src/consensus/proposer.rs` uses cumulative voting power for proportional selection)

---

## Write-Ahead Log (WAL)

The WAL provides crash recovery for the consensus core. Every consensus action is persisted before being broadcast or acted upon.

**Entry format (binary, hex-encoded per line):**
```
height(8 bytes) | round(4 bytes) | kind(1 byte) | data_len(4 bytes) | data(N bytes) | sha256_checksum(32 bytes)
```

**Entry kinds:**
| Kind | Byte | Written when |
|------|------|-------------|
| `Proposal` | 0x01 | Proposal received and accepted |
| `Prevote` | 0x02 | About to broadcast prevote |
| `Precommit` | 0x03 | About to broadcast precommit |
| `Timeout` | 0x04 | (reserved) |
| `RoundStep` | 0x05 | (reserved) |

**Recovery procedure (planned):**
1. On startup, `WAL::read_all` loads entries, stopping at the first corrupt entry.
2. Events are replayed into `ConsensusCore` to restore exact state before crash.
3. WAL is truncated after each successful block commit (`PersistBlock` → `wal.truncate()`).

Source: `src/storage/wal.rs`

---

## Storage Layer

RustBFT uses **RocksDB** for all persistent state. Three separate RocksDB instances are opened at startup:

| Store | Path | Contents |
|-------|------|---------|
| `BlockStore` | `{data_dir}/blocks` | Blocks by height, block hashes |
| `StateStore` | `{data_dir}/state` | AppState snapshots by height |
| WAL file | `{data_dir}/consensus.wal` | Consensus crash-recovery log (text) |

Source: `src/storage/`

---

## State Machine

### AppState

`AppState` holds the entire chain state in memory as sorted maps (`BTreeMap` for determinism):

- `accounts: BTreeMap<Address, Account>` — native token balances, nonces, code hashes
- `contract_code: BTreeMap<Hash, Vec<u8>>` — deployed WASM bytecode (keyed by `sha256(wasm)`)
- `contract_storage: BTreeMap<(Address, Vec<u8>), Vec<u8>>` — per-contract key-value storage

### Snapshot / Revert

All state mutations use a copy-on-write snapshot stack:

```
state.snapshot() → SnapshotId
  ... mutate state ...
state.commit_snapshot(id)   // merge changes up to parent frame
  OR
state.revert_to(id)         // undo all changes since snapshot
```

This provides atomic execution: failed transactions, out-of-gas contracts, and reverted calls leave no trace in state.

Source: `src/state/accounts.rs`

### Block Execution Pipeline

```
ExecuteBlock(block)
  └─ begin_block() (no-op MVP)
  └─ for each tx_bytes:
       decode_tx()
       execute_one_tx()
         ├─ TransferTx     → exec_transfer()
         ├─ ContractDeploy → exec_deploy() + wasmtime constructor
         ├─ ContractCall   → exec_call() + wasmtime call
         └─ ValidatorUpdate → collect in pending_val_updates
  └─ end_block() → convert pending_val_updates to ValidatorUpdate structs
  └─ compute_state_root()
  └─ return (state_root, validator_updates)
```

Source: `src/state/executor.rs`

---

## P2P Networking

### Handshake (X25519 + Ed25519)

1. Each side generates an ephemeral X25519 key pair.
2. Public keys are exchanged in plaintext over `PlainFramer`.
3. Both sides compute a shared secret via X25519 ECDH.
4. Both sides sign `(their_ephemeral_pubkey || remote_ephemeral_pubkey || chain_id || protocol_version)` with their long-term Ed25519 signing key.
5. Signatures are exchanged and verified.
6. A `ChaCha20Poly1305` AEAD cipher is initialized with the shared secret. Each direction uses a distinct nonce counter starting at 0.

Source: `src/p2p/peer.rs`

### Encrypted Frame Format

```
[ len: 4 bytes BE ] [ ciphertext: len bytes ]
```

Nonce: `[dir(1)] || [counter(8 BE)] || [0,0,0]` — 12 bytes total.  
Direction byte is `0` for send, `1` for receive (asymmetric to prevent reflection).

### Gossip Deduplication

An LRU cache (`Seen`, capacity 10,000) deduplicates messages by hash of `[MsgType(1)] || payload`. Messages already seen are dropped without forwarding.

Source: `src/p2p/gossip.rs`

### Message Types

| Type | Description |
|------|-------------|
| `Proposal` | Block proposal from proposer |
| `Prevote` | Prevote from validator |
| `Precommit` | Precommit from validator |

Source: `src/p2p/msg.rs`

---

## Smart Contract Runtime

### WASM Validation

Before deployment, modules are checked for determinism violations:

- No floating-point instructions
- No SIMD instructions
- No thread-related proposals
- No bulk memory operations
- Maximum function count: 1,000
- Maximum memory pages: 256 (16 MiB)
- Maximum import count: 50

Source: `src/contracts/validation.rs`

### Host API

Contracts have access to the following host functions:

**Storage:**
- `host_storage_read(key_ptr, key_len, val_ptr) → i32`
- `host_storage_write(key_ptr, key_len, val_ptr, val_len)`
- `host_storage_delete(key_ptr, key_len)`
- `host_storage_has(key_ptr, key_len) → i32`

**Context:**
- `host_get_caller(out_ptr)` — 20-byte address of tx sender
- `host_get_origin(out_ptr)` — 20-byte address of tx origin
- `host_get_self_address(out_ptr)` — 20-byte address of this contract
- `host_get_block_height() → i64`
- `host_get_block_timestamp() → i64` (milliseconds)
- `host_get_value() → i64` — native tokens transferred with the call
- `host_get_gas_remaining() → i64`

**Crypto:**
- `host_sha256(data_ptr, data_len, out_ptr)` — write 32 bytes
- `host_ed25519_verify(msg_ptr, msg_len, sig_ptr, pk_ptr) → i32`

**Events:**
- `host_emit_event(data_ptr, data_len)` — append to call result events

**Calls:**
- `host_call_contract(addr_ptr, input_ptr, input_len, value, gas_limit) → i32`

Source: `src/contracts/host_api.rs`

---

## Startup Sequence

The node initializes components in dependency order (`src/main.rs`):

```
1.  Load NodeConfig from TOML (or defaults)
2.  Initialize structured logging (JSON or text)
3.  Start Prometheus metrics server (optional)
4.  Create crossbeam channels: ConsensusEvent(256), ConsensusCommand(1024)
5.  Load/generate Ed25519 keypair from {data_dir}/node_key.json
6.  Bootstrap ValidatorSet (single validator = self for MVP)
7.  Open RocksDB: blocks, state
8.  Open WAL file for append
9.  Load latest AppState from StateStore (or create fresh)
10. Initialize ContractRuntime (Wasmtime engine)
11. Start TimerService (Tokio task)
12. Create P2pManager + HandshakeDeps, connect to seed peers
13. Create CommandRouter (Tokio task)
14. Spawn ConsensusCore on dedicated OS thread
15. Start RPC server (Tokio task)
16. Await Ctrl-C or P2P fatal error → graceful shutdown
```

---

## Metrics Reference

All metrics are exposed at `http://localhost:26660/metrics` in Prometheus text format.

| Metric Name | Type | Description |
|-------------|------|-------------|
| `rustbft_consensus_height` | Gauge | Current committed height |
| `rustbft_consensus_round` | Gauge | Current consensus round |
| `rustbft_consensus_proposals_received_total` | Counter | Proposals received |
| `rustbft_consensus_votes_received_total` | Counter | Votes received |
| `rustbft_consensus_timeouts_total` | Counter | Timeouts fired |
| `rustbft_consensus_equivocations_total` | Counter | Equivocations detected |
| `rustbft_consensus_block_commit_duration_seconds` | Histogram | Time from height entry to commit |
| `rustbft_consensus_rounds_per_height` | Histogram | Number of rounds to reach commit |
| `rustbft_p2p_peers_connected` | Gauge | Connected peers |
| `rustbft_p2p_messages_sent_total` | Counter | Messages sent |
| `rustbft_p2p_messages_received_total` | Counter | Messages received |
| `rustbft_p2p_bytes_sent_total` | Counter | Bytes sent |
| `rustbft_p2p_bytes_received_total` | Counter | Bytes received |
| `rustbft_state_block_execution_duration_seconds` | Histogram | Block execution time |
| `rustbft_state_tx_success_total` | Counter | Successful transactions |
| `rustbft_state_tx_failure_total` | Counter | Failed transactions |
| `rustbft_state_gas_used_total` | Counter | Cumulative gas consumed |
| `rustbft_storage_block_persist_duration_seconds` | Histogram | Block persist latency |
| `rustbft_storage_wal_write_duration_seconds` | Histogram | WAL write latency |
| `rustbft_storage_wal_entries` | Gauge | Current WAL entry count |
| `rustbft_channel_drops_total` | Counter | Messages dropped (channel full) |
| `rustbft_rpc_requests_total` | Counter | Total RPC requests |
| `rustbft_rpc_request_duration_seconds` | Histogram | RPC request latency |

Source: `src/metrics/registry.rs`
