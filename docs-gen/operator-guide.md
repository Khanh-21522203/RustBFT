# RustBFT Operator Guide

This guide covers building, configuring, and running RustBFT nodes — both locally and with Docker.

---

## Prerequisites

### System Dependencies

| Dependency | Version | Purpose |
|------------|---------|---------|
| Rust | 2024 edition (rustup) | Build toolchain |
| RocksDB | ≥ 8.x system library | Block and state storage |
| Docker + Compose | ≥ 24 | Container deployment |

**Install RocksDB on Ubuntu/Debian:**
```bash
sudo apt-get install -y librocksdb-dev clang
```

**Install RocksDB on macOS:**
```bash
brew install rocksdb
```

### Build

```bash
git clone <repo>
cd RustBFT
cargo build --release
```

Binary: `target/release/RustBFT`

---

## Single-Node Quickstart

```bash
# 1. Run with defaults (data stored in ./data, listens on default ports)
cargo run --release

# 2. Or with a config file
cargo run --release -- config/node.toml

# 3. Set log level via environment
RUST_LOG=debug cargo run --release
```

Default ports:
- `26656` — P2P
- `26657` — JSON-RPC
- `26660` — Prometheus metrics

Verify the node is running:
```bash
curl -s -X POST http://localhost:26657/ \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"status","params":{},"id":1}'
```

---

## Configuration Reference

All configuration lives in a single TOML file (default: `config/node.toml`).  
Missing fields fall back to defaults automatically.

```toml
[node]
chain_id = "localnet"        # Chain identifier (must match across all peers)
node_id  = "node0"           # Human-readable node name (used in logs)
data_dir = "data"            # Directory for RocksDB + WAL + keypair

[consensus]
timeout_propose_ms   = 1000  # Propose phase timeout (base, milliseconds)
timeout_prevote_ms   = 1000  # Prevote phase timeout (base)
timeout_precommit_ms = 1000  # Precommit phase timeout (base)
timeout_delta_ms     = 500   # Added per round: timeout = base + round * delta
create_empty_blocks  = true  # Produce blocks even when mempool is empty

[p2p]
listen_addr = "0.0.0.0:26656"
seeds       = []             # List of seed peer addresses: "nodeID@host:port"
max_peers   = 10

[rpc]
listen_addr            = "0.0.0.0:26657"
rate_limit_per_second  = 100

[mempool]
max_txs   = 5000
max_bytes = 10485760         # 10 MiB

[storage]
engine          = "rocksdb"
pruning_window  = 1000       # Keep last N block heights in storage

[observability]
metrics_enabled      = true
metrics_listen_addr  = "0.0.0.0:26660"

[logging]
format        = "json"       # "json" or "text"
level         = "info"       # "trace" | "debug" | "info" | "warn" | "error"
module_levels = ""           # e.g. "rustbft_consensus=debug,rustbft_p2p=info"
output        = "stdout"     # "stdout" or "file"
file_path     = ""           # Used when output = "file"
```

### Seed Address Format

```
<nodeID>@<host>:<port>
```

`nodeID` is the hex-encoded first 8 bytes of the validator's Ed25519 verify key.  
Example: `a1b2c3d4e5f6a7b8@192.168.1.2:26656`

---

## Four-Node Docker Cluster

The `devops/` directory contains a ready-to-use Docker Compose setup with four validators plus Prometheus and Grafana.

### Start the cluster

```bash
cd devops
docker compose up --build -d
```

This starts:
- `rustbft-node0` — `172.28.1.1`, RPC `localhost:26657`, metrics `localhost:26660`
- `rustbft-node1` — `172.28.1.2`, RPC `localhost:26756`, metrics `localhost:26760`
- `rustbft-node2` — `172.28.1.3`, RPC `localhost:26856`, metrics `localhost:26860`
- `rustbft-node3` — `172.28.1.4`, RPC `localhost:26956`, metrics `localhost:26960`
- `rustbft-prometheus` — `localhost:9090`
- `rustbft-grafana` — `localhost:3000` (admin / admin)

### Verify cluster status

```bash
# All four nodes should report height > 0
for port in 26657 26756 26856 26956; do
  echo -n "Node at :$port → "
  curl -s -X POST http://localhost:$port/ \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","method":"get_latest_height","params":{},"id":1}' \
    | python3 -m json.tool
done
```

### Submit a transaction

```bash
# Hex-encode your raw transaction bytes and submit to node0
curl -X POST http://localhost:26657/ \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"broadcast_tx","params":{"tx":"010203..."},"id":1}'
```

### Stop and clean up

```bash
docker compose down -v      # -v removes named volumes (deletes chain data)
docker compose down         # Keeps volumes (chain data survives)
```

---

## Node Key Management

On first startup, a node generates an Ed25519 keypair and saves it to:
```
{data_dir}/node_key.json
```

**Back up this file.** It contains the node's long-term identity used for P2P handshakes and as the validator key. Losing it requires regenerating the key, which changes the validator ID.

To pre-generate a key:
```bash
# The node generates it automatically on first run.
# Copy node_key.json to a safe location immediately.
cp data/node_key.json backups/node0_key.json
```

---

## Data Directory Layout

```
{data_dir}/
├── node_key.json       # Ed25519 keypair (JSON)
├── blocks/             # RocksDB: block headers and hashes by height
├── state/              # RocksDB: AppState snapshots by height
└── consensus.wal       # Write-ahead log (hex-encoded text, one entry per line)
```

---

## Log Output

**JSON format** (default, suitable for log aggregators):
```json
{"timestamp":"2024-04-11T12:00:00Z","level":"INFO","target":"RustBFT","fields":{"height":42,"msg":"Block committed and persisted"}}
```

**Text format** (human-readable):
```
2024-04-11T12:00:00Z INFO RustBFT height=42 Block committed and persisted
```

**Fine-grained log levels:**
```bash
# Verbose consensus, normal everything else
RUST_LOG=rustbft_consensus=debug,rustbft_p2p=info cargo run --release

# Or via config
[logging]
module_levels = "rustbft_consensus=debug,rustbft_p2p=info"
```

---

## Observability

### Prometheus

Metrics are exposed at `http://localhost:26660/metrics` in Prometheus text format.

Add to `prometheus.yml`:
```yaml
scrape_configs:
  - job_name: rustbft
    static_configs:
      - targets:
          - localhost:26660
          - localhost:26760
          - localhost:26860
          - localhost:26960
    scrape_interval: 5s
```

### Grafana

The `devops/grafana/` directory contains pre-configured provisioning files. Grafana auto-connects to Prometheus and loads dashboards on startup.

Access: `http://localhost:3000` (anonymous read-only, admin/admin for edit)

### Key Metrics to Watch

| Scenario | Metric | Alert threshold |
|----------|--------|-----------------|
| Node is committing blocks | `rustbft_consensus_height` | Stops increasing |
| Too many rounds (slow network) | `rustbft_consensus_rounds_per_height` | Bucket > 4 rounds |
| Equivocation attack | `rustbft_consensus_equivocations_total` | > 0 |
| Channel saturation | `rustbft_channel_drops_total` | > 0 |
| Slow execution | `rustbft_state_block_execution_duration_seconds` | p99 > 0.5s |
| Slow WAL | `rustbft_storage_wal_write_duration_seconds` | p99 > 10ms |
| Lost peers | `rustbft_p2p_peers_connected` | < f+1 = n/3+1 |

---

## Failure Simulation

The `devops/scripts/simulate-failure.sh` script demonstrates BFT fault tolerance by stopping one node.

```bash
# Stop one node (Byzantine tolerance: up to 1 of 4 can fail with f < n/3)
docker stop rustbft-node3

# Cluster continues committing blocks with 3/4 nodes
# After restart, node3 will sync via block store
docker start rustbft-node3
```

---

## Upgrading

1. Build the new binary: `cargo build --release`
2. Stop the node: `Ctrl-C` or `docker compose stop node0`
3. Replace the binary or rebuild the Docker image
4. Restart: the node reads the last committed height from RocksDB and resumes from `height + 1`
5. WAL entries from before shutdown are replayed on startup for safety

> **Caution:** Changing `chain_id` requires wiping all data. Chain IDs must match across all validators.

---

## Troubleshooting

### Node is not committing blocks

1. Check peer connectivity: `rustbft_p2p_peers_connected` should be ≥ 3 in a 4-node cluster.
2. Check for channel drops: `rustbft_channel_drops_total > 0` indicates backpressure.
3. Increase timeout: if the network is slow, raise `timeout_propose_ms` and `timeout_delta_ms`.

### WAL corruption on startup

The WAL reader stops at the first corrupt entry (a crash mid-write) and uses the clean entries. If the WAL is fully corrupt, delete `{data_dir}/consensus.wal` and restart — the node will rebuild from RocksDB.

### RocksDB fails to open

- Check that the `data_dir` path exists and is writable.
- RocksDB acquires a lock file (`LOCK`) — only one process can open each DB. Kill any orphaned processes first.

### "mempool full" error from RPC

In the MVP, the mempool is a bounded channel stub. This error means the submission channel is full. It will self-resolve as the channel drains (or after the placeholder router processes entries). Full mempool integration is a planned feature.
