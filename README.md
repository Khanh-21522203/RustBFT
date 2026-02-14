# RustBFT

A Byzantine Fault Tolerant (BFT) consensus engine written in Rust. RustBFT implements a Tendermint-inspired three-phase consensus protocol with deterministic finality, smart contract execution via WebAssembly, and encrypted peer-to-peer networking.

## Features

- **BFT Consensus** — Three-phase (Propose → Prevote → Precommit) round-based protocol with deterministic finality. Committed blocks are irreversible — no forks, no reorgs.
- **Safety** — Tolerates up to f < n/3 Byzantine validators via locking mechanism and quorum-based voting with integer-only arithmetic.
- **Hybrid Threading** — Single-threaded synchronous consensus core wrapped by an async Tokio I/O shell. The consensus state is never behind a lock.
- **Encrypted P2P** — X25519 key exchange + ChaCha20-Poly1305 AEAD encrypted channels with Ed25519 authenticated handshakes.
- **Smart Contracts** — WebAssembly contract execution via Wasmtime with gas metering (fuel), sandboxed host API, and determinism validation (no floats, no SIMD, no threads).
- **RocksDB Storage** — Persistent block store, state store, and write-ahead log (WAL) for crash recovery.
- **JSON-RPC API** — HTTP JSON-RPC 2.0 server for querying blocks, accounts, validators, and submitting transactions.
- **Prometheus Metrics** — Full observability with counters, gauges, and histograms for consensus, networking, state execution, and storage.
- **TOML Configuration** — Single config file with sensible defaults for all subsystems.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        LAYER STACK                          │
├─────────────────────────────────────────────────────────────┤
│  Layer 7:  Observability    (Prometheus metrics, tracing)   │
│  Layer 6:  RPC / API        (HTTP JSON-RPC 2.0)            │
│  Layer 5:  Networking       (P2P, gossip, encrypted TCP)   │
│  Layer 4:  Mempool          (tx intake — planned)          │
│  Layer 3:  Consensus        (BFT protocol, single-thread)  │
│  Layer 2:  State Machine    (block exec, WASM contracts)   │
│  Layer 1:  Storage          (RocksDB blocks/state, WAL)    │
│  Layer 0:  Crypto / Types   (Ed25519, SHA256, primitives)  │
└─────────────────────────────────────────────────────────────┘
```

### Module Layout

```
src/
├── main.rs              # Binary entry point: wires all modules
├── lib.rs               # Library root: re-exports public modules
├── types/               # Layer 0: shared types and primitives
│   ├── hash.rs          #   32-byte SHA256 hash
│   ├── address.rs       #   20-byte account address
│   ├── block.rs         #   Block, BlockHeader, CommitInfo
│   ├── vote.rs          #   Vote, SignedVote, Evidence
│   ├── proposal.rs      #   Proposal, SignedProposal
│   ├── validator.rs     #   ValidatorId, ValidatorSet, voting power
│   ├── transaction.rs   #   Transaction types (Transfer, Deploy, Call, ValidatorUpdate)
│   └── serialization.rs #   Canonical deterministic binary encoding
├── crypto/              # Layer 0: cryptographic operations
│   ├── ed25519.rs       #   Keypair generation, signing, verification
│   └── hash.rs          #   SHA256 hashing
├── storage/             # Layer 1: persistent storage
│   ├── block_store.rs   #   RocksDB block persistence
│   ├── state_store.rs   #   RocksDB state snapshots
│   └── wal.rs           #   Write-ahead log for crash recovery
├── state/               # Layer 2: state machine
│   ├── accounts.rs      #   Account state, snapshots, contract storage
│   ├── executor.rs      #   Block/transaction execution with gas
│   ├── merkle.rs        #   State root computation
│   └── tx.rs            #   Transaction wire-format decoding
├── contracts/           # Layer 2: WASM contract engine
│   ├── runtime.rs       #   Wasmtime-based contract runtime
│   ├── host_api.rs      #   Host functions (storage, crypto, context)
│   └── validation.rs    #   WASM module validation (determinism)
├── consensus/           # Layer 3: BFT consensus core
│   ├── state.rs         #   Consensus state machine (single-threaded)
│   ├── events.rs        #   ConsensusEvent / ConsensusCommand enums
│   ├── vote_set.rs      #   Vote aggregation, quorum, equivocation
│   ├── proposer.rs      #   Weighted round-robin proposer selection
│   └── timer.rs         #   Async timer service
├── router/              # Bridge: consensus → async subsystems
│   └── mod.rs           #   Command router (P2P, timers, state, storage)
├── p2p/                 # Layer 5: peer-to-peer networking
│   ├── manager.rs       #   P2P manager, gossip fanout
│   ├── peer.rs          #   Handshake (X25519 + Ed25519)
│   ├── codec.rs         #   Plaintext/encrypted framing
│   ├── gossip.rs        #   Deduplication (LRU seen cache)
│   ├── msg.rs           #   Network message types and encoding
│   └── config.rs        #   P2P configuration
├── rpc/                 # Layer 6: JSON-RPC API
│   ├── server.rs        #   Axum HTTP server
│   ├── handlers.rs      #   RPC method dispatch
│   └── types.rs         #   JSON-RPC 2.0 request/response types
├── metrics/             # Layer 7: observability
│   ├── registry.rs      #   Prometheus metric definitions
│   └── server.rs        #   Metrics HTTP endpoint
└── config/              # Configuration
    └── mod.rs           #   TOML config loading with defaults
```

### Data Flow — Happy Path

```
1. Client ──HTTP──▶ RPC ──channel──▶ Mempool (planned)
2. Timer fires ──▶ Consensus: "I am proposer" ──▶ Build Block ──▶ Broadcast Proposal
3. Peers receive Proposal ──▶ Validate ──▶ Prevote (block or nil) ──▶ Broadcast
4. Consensus collects >2/3 prevotes ──▶ Precommit ──▶ Broadcast
5. Consensus collects >2/3 precommits ──▶ Commit:
   ├── Execute block (state machine)
   ├── Persist block + state (RocksDB)
   ├── Truncate WAL
   └── Advance to next height
```

### Channel Topology

```
                    ┌──────────────┐
  P2P ──(crossbeam)▶│              │──(mpsc)──▶ P2P (outbound)
  Timers ──(cross.)▶│  Consensus   │──(mpsc)──▶ Router ──▶ State Machine
                    │    Core      │           │        ──▶ Storage
                    │  (sync,      │           │        ──▶ WAL
                    │   1 thread)  │           │        ──▶ Timers
                    └──────────────┘
```

All channels are **bounded** to provide backpressure and prevent OOM.

## Consensus Protocol

RustBFT uses a three-phase BFT consensus protocol:

| Property | Guarantee |
|----------|-----------|
| **Safety** | No two different blocks committed at the same height (f < n/3) |
| **Liveness** | Commits new blocks when >2/3 voting power is honest and connected |
| **Deterministic finality** | Committed blocks are final — no forks, no reorgs |
| **Accountability** | Equivocation (double-voting) is detected and logged |

### Round Structure

Each height proceeds through rounds. Each round has three phases:

```
┌──────────┐    ┌──────────┐    ┌─────────────┐
│ PROPOSE  │───▶│ PREVOTE  │───▶│ PRECOMMIT   │
│          │    │          │    │             │
│ Proposer │    │ All vote │    │ All vote    │
│ sends    │    │ for block│    │ to commit   │
│ block    │    │ or nil   │    │ or nil      │
└──────────┘    └──────────┘    └─────────────┘
```

### Timeouts

Timeouts grow linearly with round number for liveness under increasing asynchrony:

```
timeout(round) = BASE_TIMEOUT + round * TIMEOUT_DELTA
```

Defaults: base = 1000ms, delta = 500ms.

### Proposer Selection

Deterministic weighted round-robin: validators are selected proportionally to their voting power over time.

## Smart Contracts

RustBFT supports WebAssembly smart contracts with:

- **Gas metering** via Wasmtime fuel
- **Determinism validation** — no floats, no SIMD, no threads, no bulk memory
- **Sandboxed host API**:
  - Storage: `host_storage_read`, `host_storage_write`, `host_storage_delete`, `host_storage_has`
  - Context: `host_get_caller`, `host_get_origin`, `host_get_self_address`, `host_get_block_height`, `host_get_block_timestamp`, `host_get_value`, `host_get_gas_remaining`
  - Crypto: `host_sha256`, `host_ed25519_verify`
  - Events: `host_emit_event`
  - Calls: `host_call_contract`
- **Snapshot/revert** for atomic state changes — failed contract calls are fully reverted

## Configuration

Create `config/node.toml`:

```toml
[node]
chain_id = "localnet"
node_id = "node0"
data_dir = "data"

[consensus]
timeout_propose_ms = 1000
timeout_prevote_ms = 1000
timeout_precommit_ms = 1000
timeout_delta_ms = 500
create_empty_blocks = true

[p2p]
listen_addr = "0.0.0.0:26656"
seeds = []
max_peers = 10

[rpc]
listen_addr = "0.0.0.0:26657"
rate_limit_per_second = 100

[storage]
engine = "rocksdb"
pruning_window = 1000

[observability]
metrics_enabled = true
metrics_listen_addr = "0.0.0.0:26660"

[logging]
format = "json"
level = "info"
output = "stdout"
```

All fields have sensible defaults. Missing fields use defaults automatically.

## Building

```bash
# Prerequisites: Rust 2024 edition, RocksDB system library
cargo build --release
```

## Running

```bash
# With default config
cargo run --release

# With custom config
cargo run --release -- config/node.toml

# Environment variable for log level
RUST_LOG=debug cargo run --release
```

The node will:
1. Load config from TOML (or use defaults)
2. Initialize RocksDB storage and WAL
3. Load or generate Ed25519 keypair
4. Start P2P listener, metrics server, RPC server
5. Start consensus core on a dedicated thread
6. Begin producing blocks

## RPC API

JSON-RPC 2.0 over HTTP POST at `http://localhost:26657/`.

### Methods

| Method | Params | Description |
|--------|--------|-------------|
| `status` | — | Node info (node_id, chain_id, latest height) |
| `get_latest_height` | — | Last committed block height |
| `get_block` | `{"height": N}` | Block at height N |
| `get_block_hash` | `{"height": N}` | Block hash at height N |
| `get_account` | `{"address": "hex"}` | Account balance, nonce, code hash |
| `get_validators` | — | Current validator set |
| `broadcast_tx` | `{"tx": "hex"}` | Submit a transaction |

### Health Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Liveness probe |
| `/ready` | GET | Readiness probe (returns 503 until first block) |

### Example

```bash
# Get node status
curl -X POST http://localhost:26657/ \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"status","params":{},"id":1}'

# Get block at height 1
curl -X POST http://localhost:26657/ \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"get_block","params":{"height":1},"id":1}'
```

## Metrics

Prometheus metrics at `http://localhost:26660/metrics`.

| Metric | Type | Description |
|--------|------|-------------|
| `rustbft_consensus_height` | Gauge | Current consensus height |
| `rustbft_consensus_round` | Gauge | Current consensus round |
| `rustbft_consensus_proposals_received_total` | Counter | Proposals received |
| `rustbft_consensus_votes_received_total` | Counter | Votes received |
| `rustbft_consensus_timeouts_total` | Counter | Timeouts fired |
| `rustbft_consensus_equivocations_total` | Counter | Equivocations detected |
| `rustbft_consensus_block_commit_duration_seconds` | Histogram | Time to commit |
| `rustbft_p2p_peers_connected` | Gauge | Connected peers |
| `rustbft_p2p_messages_sent_total` | Counter | Messages sent |
| `rustbft_state_block_execution_duration_seconds` | Histogram | Block execution time |
| `rustbft_state_tx_success_total` | Counter | Successful transactions |
| `rustbft_state_gas_used_total` | Counter | Total gas consumed |
| `rustbft_storage_block_persist_duration_seconds` | Histogram | Block persist time |
| `rustbft_storage_wal_write_duration_seconds` | Histogram | WAL write time |
| `rustbft_rpc_requests_total` | Counter | RPC requests |

## Transaction Types

| Type | Code | Description |
|------|------|-------------|
| Transfer | `0x01` | Send tokens between accounts |
| ContractDeploy | `0x02` | Deploy a WASM smart contract |
| ContractCall | `0x03` | Call a deployed contract |
| ValidatorUpdate | `0x04` | Add/remove/update validator (admin) |

All transactions use canonical big-endian binary encoding with Ed25519 signatures.

## Security

- **P2P**: X25519 ECDH key exchange → ChaCha20-Poly1305 AEAD encryption, Ed25519 signature verification on handshake
- **Consensus**: All proposals and votes are signature-verified before processing
- **Contracts**: WASM validation rejects non-deterministic features; gas metering prevents DoS; snapshot/revert ensures atomicity
- **RPC**: Input validation on all endpoints; bounded channels prevent OOM

## Project Status

RustBFT is an MVP implementation. The following features are implemented and wired:

- [x] BFT consensus state machine with locking
- [x] Weighted round-robin proposer selection
- [x] Encrypted P2P with gossip
- [x] WASM smart contract execution
- [x] RocksDB block and state persistence
- [x] Write-ahead log for crash recovery
- [x] JSON-RPC API
- [x] Prometheus metrics infrastructure
- [x] TOML configuration
- [x] Equivocation detection and logging

### Planned

- [ ] Full mempool module (tx validation, ordering, eviction)
- [ ] Block sync / fast sync for new nodes
- [ ] Light client verification
- [ ] Evidence gossiping and slashing
- [ ] RPC rate limiting middleware
- [ ] Read-only contract query endpoint
- [ ] Reconnection backoff and peer banning
- [ ] Ping/pong keepalive

## License

MIT
