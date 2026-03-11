# Feature: Docker & DevOps

## 1. Purpose

The Docker & DevOps module provides the containerised multi-node local environment for development, integration testing, and failure simulation. It bundles a four-validator cluster, a Prometheus metrics server, and Grafana dashboards into a single `docker-compose up` command, giving engineers an immediately runnable reference cluster without any cloud infrastructure.

## 2. Responsibilities

- Provide a multi-stage `Dockerfile` that builds a minimal production-grade container image for `rustbft-node`
- Provide a `docker-compose.yml` that starts four validator nodes, Prometheus, and Grafana on an isolated bridge network
- Provide per-node TOML configuration files and a shared genesis file for the local testnet
- Provide a `prometheus.yml` that scrapes all four nodes' metrics endpoints
- Provide Grafana provisioning config and pre-built dashboard JSON files (consensus, network, state machine, infrastructure)
- Provide helper scripts for key generation, cluster init, transaction submission, and failure simulation
- Document all failure simulation procedures: node crash, network partition, slow network, disk full, graceful shutdown

## 3. Non-Responsibilities

- Does not configure production Kubernetes or cloud deployments
- Does not manage TLS certificates or public network firewall rules
- Does not implement CI/CD pipelines — that is handled by `.github/workflows/`
- Does not create Helm charts or Terraform modules

## 4. Architecture Design

```
Docker Compose bridge network: rustbft-net (172.28.0.0/16)

┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  node0 (172.28.1.1)   node1 (172.28.1.2)                    │
│  P2P:  26656          P2P:  26656 (host:26756)               │
│  RPC:  26657 (host)   RPC:  26657 (host:26757)               │
│  Met:  26660 (host)   Met:  26660 (host:26760)               │
│                                                              │
│  node2 (172.28.1.3)   node3 (172.28.1.4)                    │
│  P2P:  26656           P2P:  26656 (host:26956)              │
│  RPC:  26657 (host:26857) RPC: 26657 (host:26957)            │
│  Met:  26660 (host:26860) Met: 26660 (host:26960)            │
│                                                              │
│  prometheus (172.28.1.10)  grafana (172.28.1.11)             │
│  :9090 (host)              :3000 (host)                      │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

## 5. Core Data Structures (Rust)

Not applicable — this feature is infrastructure configuration, not Rust code. The relevant file layout is:

```
devops/
├── Dockerfile
├── docker-compose.yml
├── prometheus.yml
├── grafana/
│   ├── provisioning/
│   │   ├── datasources/prometheus.yml
│   │   └── dashboards/dashboard.yml
│   └── dashboards/
│       ├── consensus.json
│       ├── network.json
│       ├── state-machine.json
│       └── infrastructure.json
├── config/
│   ├── genesis.json               # shared by all nodes
│   ├── node0/node.toml
│   ├── node1/node.toml
│   ├── node2/node.toml
│   └── node3/node.toml
└── scripts/
    ├── init-cluster.sh
    ├── submit-tx.sh
    └── simulate-failure.sh
```

## 6. Public Interfaces

Key configuration files and their purposes:

```dockerfile
# devops/Dockerfile
FROM rust:1.75-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --bin rustbft-node

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/rustbft-node /usr/local/bin/rustbft-node
RUN mkdir -p /data /config
EXPOSE 26656 26657 26660
ENTRYPOINT ["rustbft-node"]
CMD ["--config", "/config/node.toml"]
```

```yaml
# devops/prometheus.yml
global:
  scrape_interval: 5s
scrape_configs:
  - job_name: "rustbft"
    static_configs:
      - targets:
          - "172.28.1.1:26660"
          - "172.28.1.2:26660"
          - "172.28.1.3:26660"
          - "172.28.1.4:26660"
        labels:
          cluster: "local-testnet"
```

```json
// devops/config/genesis.json
{
    "chain_id": "rustbft-local-1",
    "genesis_time": 1700000000,
    "initial_validators": [
        { "id": "node0", "public_key": "ed25519:<hex>", "voting_power": 100 },
        { "id": "node1", "public_key": "ed25519:<hex>", "voting_power": 100 },
        { "id": "node2", "public_key": "ed25519:<hex>", "voting_power": 100 },
        { "id": "node3", "public_key": "ed25519:<hex>", "voting_power": 100 }
    ],
    "initial_accounts": [
        { "address": "0xalice...", "balance": 10000000 },
        { "address": "0xbob...",   "balance": 10000000 }
    ],
    "admin_addresses": ["0xalice..."],
    "chain_params": {
        "max_block_gas": 10000000,
        "max_block_bytes": 1048576,
        "max_tx_bytes": 65536
    }
}
```

## 7. Internal Algorithms

### Cluster Init Script
```bash
#!/usr/bin/env bash
# devops/scripts/init-cluster.sh

set -e

# 1. Generate key pairs for all 4 nodes
for i in 0 1 2 3; do
    rustbft-node keygen --output devops/config/node${i}/node_key.json
    echo "node${i} key generated"
done

# 2. Extract public keys and update genesis.json
for i in 0 1 2 3; do
    pubkey=$(jq -r '.public_key' devops/config/node${i}/node_key.json)
    # Update genesis.json in place
    jq --arg i "$i" --arg pk "$pubkey" \
       '.initial_validators[$i|tonumber].public_key = $pk' \
       devops/config/genesis.json > /tmp/genesis_tmp.json
    mv /tmp/genesis_tmp.json devops/config/genesis.json
done

echo "Genesis file updated with public keys"
echo "Run: docker-compose up -d"
```

### Failure Simulation Scripts
```bash
# Node crash
docker kill rustbft-node2                          # SIGKILL — hard crash
docker start rustbft-node2

# Network partition: isolate node2 and node3
docker network disconnect rustbft-net rustbft-node2
docker network disconnect rustbft-net rustbft-node3
# Heal:
docker network connect rustbft-net rustbft-node2
docker network connect rustbft-net rustbft-node3

# Add 200ms network latency to node1
docker exec rustbft-node1 tc qdisc add dev eth0 root netem delay 200ms
# Remove:
docker exec rustbft-node1 tc qdisc del dev eth0 root

# Disk full: fill data volume on node3
docker exec rustbft-node3 dd if=/dev/zero of=/data/fill bs=1M count=1000
# Recover:
docker exec rustbft-node3 rm /data/fill && docker restart rustbft-node3

# Graceful shutdown: send SIGTERM (not SIGKILL)
docker stop rustbft-node1
```

### Grafana Dashboard Provisioning
```yaml
# devops/grafana/provisioning/dashboards/dashboard.yml
apiVersion: 1
providers:
  - name: "rustbft"
    folder: "RustBFT"
    type: file
    options:
      path: /var/lib/grafana/dashboards
```

```yaml
# devops/grafana/provisioning/datasources/prometheus.yml
apiVersion: 1
datasources:
  - name: Prometheus
    type: prometheus
    url: http://prometheus:9090
    isDefault: true
```

### Useful Prometheus Queries
```promql
# Block commit rate (blocks per minute)
rate(rustbft_consensus_height[1m]) * 60

# 95th percentile commit latency
histogram_quantile(0.95,
  rate(rustbft_consensus_block_commit_duration_seconds_bucket[5m]))

# Rounds per height (healthy = ~1)
histogram_quantile(0.99,
  rate(rustbft_consensus_rounds_per_height_bucket[5m]))

# Timeout rate (should be near zero in healthy cluster)
rate(rustbft_consensus_timeouts_total[5m])

# Channel utilisation (detect backpressure)
rustbft_channel_length / rustbft_channel_capacity
```

## 8. Persistence Model

Each node uses a named Docker volume (`node0-data`, `node1-data`, etc.) that persists across container restarts. Running `docker-compose down -v` removes volumes and provides a clean slate. The `devops/config/` directory on the host is mounted read-only into each container at `/config`.

## 9. Concurrency Model

Not applicable — Docker Compose manages container lifecycle. Each node container is isolated; they communicate over the bridge network. The host's test scripts interact with nodes sequentially via HTTP (`curl`, `rustbft-cli`).

## 10. Configuration

```toml
# devops/config/node0/node.toml

[node]
chain_id = "rustbft-local-1"
node_id = "node0"
data_dir = "/data"
key_file = "/config/node_key.json"

[consensus]
timeout_propose_ms = 1000
timeout_prevote_ms = 1000
timeout_precommit_ms = 1000
create_empty_blocks = true

[p2p]
listen_addr = "0.0.0.0:26656"
seeds = [
    "node1@172.28.1.2:26656",
    "node2@172.28.1.3:26656",
    "node3@172.28.1.4:26656",
]

[rpc]
listen_addr = "0.0.0.0:26657"
rate_limit_per_second = 100

[mempool]
max_txs = 5000
max_bytes = 10485760

[storage]
engine = "rocksdb"
pruning_window = 1000

[observability]
metrics_listen_addr = "0.0.0.0:26660"

[logging]
format = "json"
level = "info"
output = "stdout"
```

## 11. Observability

All observability is provided by Prometheus scraping node metrics endpoints and Grafana visualizing them:

- **Prometheus**: `http://localhost:9090` — query raw metrics
- **Grafana**: `http://localhost:3000` (admin/admin) — pre-built dashboards:
  - **Consensus**: height, round, commit latency (p50/p95/p99), rounds per height, timeout rate, equivocations
  - **Network**: peer count, message rates by type, bytes sent/received, connection errors
  - **State Machine**: block execution time, tx throughput, gas usage, success/failure rates
  - **Infrastructure**: channel utilisation, storage write latency, WAL size, DB size, memory/CPU (node_exporter optional)

Container logs are JSON structured and viewable via:
```bash
docker-compose logs -f node0 | jq 'select(.level == "ERROR")'
```

## 12. Testing Strategy

- **`test_cluster_starts_all_healthy`**: `docker-compose up -d`, poll `/health` on all 4 nodes, assert all return 200 within 30s
- **`test_cluster_commits_blocks`**: after startup, poll `status` every second for 30s, assert `latest_height` advances on all 4 nodes
- **`test_submit_transfer_committed`**: use `rustbft-cli tx transfer --wait`, assert receipt shows `success: true`
- **`test_node_crash_cluster_continues`**: `docker kill rustbft-node3`, submit 5 transactions, assert all committed by remaining 3 nodes
- **`test_node_restart_catches_up`**: `docker kill` then `docker start` node2, wait 30s, assert node2 height matches other nodes
- **`test_network_partition_no_commits`**: disconnect nodes 2 and 3, wait 15s, assert height does not advance on any node
- **`test_network_partition_heals`**: after partition, reconnect nodes, assert commits resume within 30s
- **`test_contract_deploy_and_call`**: deploy counter contract via CLI, call `increment()`, query `get_count()` → count == 1
- **`test_validator_add_takes_effect`**: generate new key, submit ValidatorUpdate add tx, verify `get_validators` includes new validator at next height
- **`test_crash_recovery_wal`**: `docker kill` (SIGKILL, no graceful shutdown), `docker start`, assert node recovers to correct height without safety violation
- **`test_metrics_scraped_by_prometheus`**: query Prometheus API for `rustbft_consensus_height`, assert all 4 node time series present and advancing
- **`test_graceful_shutdown_logs`**: `docker stop` (SIGTERM), inspect logs, assert `"Node stopped"` log line present with uptime

## 13. Open Questions

- **Dockerfile base image**: `debian:bookworm-slim` provides `ca-certificates` for HTTPS. If the node binary links against RocksDB's C library, ensure the shared library is included or switch to a static RocksDB build. For MVP, a static build (`ROCKSDB_STATIC=1`) avoids the dependency.
- **ARM builds**: CI may need a multi-architecture build (`docker buildx`) for Apple Silicon development machines. Deferred; single `linux/amd64` target is sufficient for MVP.
