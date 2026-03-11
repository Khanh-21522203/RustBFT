# RustBFT — Docker & Local Testing

**Purpose:** Define the Docker-based local development and testing environment: container setup, network configuration, transaction submission, failure simulation.
**Audience:** Engineers and operators running local multi-node clusters.

---

## 1. Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    Docker Compose Cluster                        │
│                                                                  │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐          │
│  │  node0   │ │  node1   │ │  node2   │ │  node3   │          │
│  │          │ │          │ │          │ │          │          │
│  │ RPC:26657│ │ RPC:26757│ │ RPC:26857│ │ RPC:26957│          │
│  │ P2P:26656│ │ P2P:26756│ │ P2P:26856│ │ P2P:26956│          │
│  │ Met:26660│ │ Met:26760│ │ Met:26860│ │ Met:26960│          │
│  └────┬─────┘ └────┬─────┘ └────┬─────┘ └────┬─────┘          │
│       │             │            │             │                │
│       └─────────────┴────────────┴─────────────┘                │
│                         │                                        │
│                    rustbft-net (bridge)                          │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐                             │
│  │  Prometheus   │  │   Grafana    │                             │
│  │  :9090        │  │   :3000      │                             │
│  └──────────────┘  └──────────────┘                             │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## 2. Dockerfile

```dockerfile
# Multi-stage build for minimal image size

# Stage 1: Build
FROM rust:1.75-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --bin rustbft-node

# Stage 2: Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/rustbft-node /usr/local/bin/rustbft-node

# Default directories
RUN mkdir -p /data /config

EXPOSE 26656 26657 26660

ENTRYPOINT ["rustbft-node"]
CMD ["--config", "/config/node.toml"]
```

---

## 3. Docker Compose

```yaml
# devops/docker-compose.yml

version: "3.8"

networks:
  rustbft-net:
    driver: bridge
    ipam:
      config:
        - subnet: 172.28.0.0/16

services:
  node0:
    build:
      context: ..
      dockerfile: devops/Dockerfile
    container_name: rustbft-node0
    hostname: node0
    networks:
      rustbft-net:
        ipv4_address: 172.28.1.1
    ports:
      - "26656:26656"   # P2P
      - "26657:26657"   # RPC
      - "26660:26660"   # Metrics
    volumes:
      - ./config/node0:/config
      - node0-data:/data
    environment:
      - RUSTBFT_NODE_ID=node0
      - RUST_LOG=rustbft_consensus=debug,rustbft_p2p=info
    restart: unless-stopped

  node1:
    build:
      context: ..
      dockerfile: devops/Dockerfile
    container_name: rustbft-node1
    hostname: node1
    networks:
      rustbft-net:
        ipv4_address: 172.28.1.2
    ports:
      - "26756:26657"
      - "26760:26660"
    volumes:
      - ./config/node1:/config
      - node1-data:/data
    environment:
      - RUSTBFT_NODE_ID=node1
      - RUST_LOG=info
    restart: unless-stopped

  node2:
    build:
      context: ..
      dockerfile: devops/Dockerfile
    container_name: rustbft-node2
    hostname: node2
    networks:
      rustbft-net:
        ipv4_address: 172.28.1.3
    ports:
      - "26856:26657"
      - "26860:26660"
    volumes:
      - ./config/node2:/config
      - node2-data:/data
    environment:
      - RUSTBFT_NODE_ID=node2
      - RUST_LOG=info
    restart: unless-stopped

  node3:
    build:
      context: ..
      dockerfile: devops/Dockerfile
    container_name: rustbft-node3
    hostname: node3
    networks:
      rustbft-net:
        ipv4_address: 172.28.1.4
    ports:
      - "26956:26657"
      - "26960:26660"
    volumes:
      - ./config/node3:/config
      - node3-data:/data
    environment:
      - RUSTBFT_NODE_ID=node3
      - RUST_LOG=info
    restart: unless-stopped

  prometheus:
    image: prom/prometheus:v2.48.0
    container_name: rustbft-prometheus
    networks:
      rustbft-net:
        ipv4_address: 172.28.1.10
    ports:
      - "9090:9090"
    volumes:
      - ./prometheus.yml:/etc/prometheus/prometheus.yml
      - prometheus-data:/prometheus
    restart: unless-stopped

  grafana:
    image: grafana/grafana:10.2.0
    container_name: rustbft-grafana
    networks:
      rustbft-net:
        ipv4_address: 172.28.1.11
    ports:
      - "3000:3000"
    volumes:
      - ./grafana/provisioning:/etc/grafana/provisioning
      - ./grafana/dashboards:/var/lib/grafana/dashboards
      - grafana-data:/var/lib/grafana
    environment:
      - GF_SECURITY_ADMIN_PASSWORD=admin
      - GF_AUTH_ANONYMOUS_ENABLED=true
    restart: unless-stopped

volumes:
  node0-data:
  node1-data:
  node2-data:
  node3-data:
  prometheus-data:
  grafana-data:
```

---

## 4. Prometheus Configuration

```yaml
# devops/prometheus.yml

global:
  scrape_interval: 5s
  evaluation_interval: 5s

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

---

## 5. Node Configuration

Each node has a TOML config file:

```toml
# devops/config/node0/node.toml

[node]
chain_id = "rustbft-local-1"
node_id = "node0"
data_dir = "/data"

[consensus]
timeout_propose_ms = 1000
timeout_prevote_ms = 1000
timeout_precommit_ms = 1000
timeout_delta_ms = 500
create_empty_blocks = true

[p2p]
listen_addr = "0.0.0.0:26656"
seeds = [
    "node1@172.28.1.2:26656",
    "node2@172.28.1.3:26656",
    "node3@172.28.1.4:26656",
]
max_peers = 10

[rpc]
listen_addr = "0.0.0.0:26657"
rate_limit_per_second = 100

[mempool]
max_txs = 5000
max_bytes = 10485760   # 10 MB

[storage]
engine = "rocksdb"
pruning_window = 1000

[observability]
metrics_listen_addr = "0.0.0.0:26660"

[logging]
format = "json"
level = "info"
```

### Genesis File

```json
// devops/config/genesis.json (shared by all nodes)

{
    "chain_id": "rustbft-local-1",
    "genesis_time": 1700000000,
    "initial_validators": [
        {
            "id": "node0",
            "public_key": "ed25519:<node0-pubkey-hex>",
            "voting_power": 100
        },
        {
            "id": "node1",
            "public_key": "ed25519:<node1-pubkey-hex>",
            "voting_power": 100
        },
        {
            "id": "node2",
            "public_key": "ed25519:<node2-pubkey-hex>",
            "voting_power": 100
        },
        {
            "id": "node3",
            "public_key": "ed25519:<node3-pubkey-hex>",
            "voting_power": 100
        }
    ],
    "initial_accounts": [
        {
            "address": "0xalice...",
            "balance": 10000000
        },
        {
            "address": "0xbob...",
            "balance": 10000000
        }
    ],
    "admin_addresses": ["0xalice..."],
    "chain_params": {
        "max_block_gas": 10000000,
        "max_block_bytes": 1048576,
        "max_tx_bytes": 65536
    }
}
```

---

## 6. Key Generation

Before first run, generate keypairs for each validator:

```bash
# Generate keys for all 4 nodes
for i in 0 1 2 3; do
    rustbft-node keygen --output devops/config/node${i}/node_key.json
done

# Output format:
# {
#     "public_key": "ed25519:abcdef1234...",
#     "private_key": "ed25519:secretkey..."
# }

# After generating keys, update genesis.json with the public keys
```

---

## 7. Operations

### 7.1 Start Cluster

```bash
cd devops
docker-compose up -d

# Verify all nodes are running
docker-compose ps

# Check health
for port in 26657 26756 26856 26956; do
    curl -s http://localhost:${port}/health | jq .
done
```

### 7.2 View Logs

```bash
# All nodes
docker-compose logs -f

# Single node
docker-compose logs -f node0

# Filter by level (using jq for JSON logs)
docker-compose logs node0 | jq 'select(.level == "ERROR")'
```

### 7.3 Check Consensus Status

```bash
# Current height and round for each node
for port in 26657 26756 26856 26956; do
    echo "=== Port ${port} ==="
    curl -s http://localhost:${port}/status | jq '{height: .result.latest_height, round: .result.consensus.round, step: .result.consensus.step}'
done
```

### 7.4 Stop Cluster

```bash
docker-compose down

# Stop and remove volumes (clean slate)
docker-compose down -v
```

---

## 8. Transaction Submission

### 8.1 Simple Transfer

```bash
# Submit a transfer transaction
curl -X POST http://localhost:26657 \
  -H 'Content-Type: application/json' \
  -d '{
    "jsonrpc": "2.0",
    "method": "broadcast_tx_commit",
    "params": {
        "tx": "<hex-encoded-signed-transfer-tx>"
    },
    "id": 1
  }'
```

### 8.2 Using a CLI Tool

```bash
# Build the CLI tool
cargo build --release --bin rustbft-cli

# Transfer
rustbft-cli tx transfer \
    --from alice \
    --to bob \
    --amount 1000 \
    --key-file devops/config/node0/alice_key.json \
    --node http://localhost:26657

# Query balance
rustbft-cli query account --address 0xbob... --node http://localhost:26657
```

---

## 9. Contract Deployment

### 9.1 Compile Contract

```bash
# Compile a Rust contract to WASM
cd examples/contracts/counter
cargo build --target wasm32-unknown-unknown --release

# The output is at:
# target/wasm32-unknown-unknown/release/counter.wasm
```

### 9.2 Deploy Contract

```bash
rustbft-cli tx deploy \
    --from alice \
    --code examples/contracts/counter/target/wasm32-unknown-unknown/release/counter.wasm \
    --gas-limit 1000000 \
    --key-file devops/config/node0/alice_key.json \
    --node http://localhost:26657

# Output:
# Contract deployed at address: 0xcontract123...
# Transaction hash: 0xtxhash...
# Gas used: 250000
```

### 9.3 Call Contract

```bash
rustbft-cli tx call \
    --from alice \
    --to 0xcontract123... \
    --input "increment()" \
    --gas-limit 100000 \
    --key-file devops/config/node0/alice_key.json \
    --node http://localhost:26657

# Query contract state
rustbft-cli query contract \
    --address 0xcontract123... \
    --input "get_count()" \
    --node http://localhost:26657
```

---

## 10. Validator Set Updates

### 10.1 Add a Validator

```bash
# Generate key for new validator
rustbft-node keygen --output devops/config/node4/node_key.json

# Submit validator add transaction (must be from admin address)
rustbft-cli tx validator-update \
    --from alice \
    --action add \
    --validator-pubkey "ed25519:<node4-pubkey>" \
    --voting-power 100 \
    --key-file devops/config/node0/alice_key.json \
    --node http://localhost:26657

# Verify at next height
rustbft-cli query validators --node http://localhost:26657
```

### 10.2 Remove a Validator

```bash
rustbft-cli tx validator-update \
    --from alice \
    --action remove \
    --validator-id node3 \
    --key-file devops/config/node0/alice_key.json \
    --node http://localhost:26657
```

### 10.3 Update Voting Power

```bash
rustbft-cli tx validator-update \
    --from alice \
    --action update-power \
    --validator-id node2 \
    --voting-power 200 \
    --key-file devops/config/node0/alice_key.json \
    --node http://localhost:26657
```

---

## 11. Failure Simulation

### 11.1 Node Crash

```bash
# Kill a node (simulates crash — no graceful shutdown)
docker kill rustbft-node2

# Verify cluster continues (3/4 > 2/3)
curl -s http://localhost:26657/status | jq .result.latest_height

# Restart the crashed node
docker start rustbft-node2

# Verify it catches up
sleep 10
curl -s http://localhost:26856/status | jq .result.latest_height
```

### 11.2 Network Partition

```bash
# Partition node2 and node3 from node0 and node1
docker network disconnect rustbft-net rustbft-node2
docker network disconnect rustbft-net rustbft-node3

# Verify: no commits (neither partition has >2/3)
sleep 15
for port in 26657 26756 26856 26956; do
    echo "Port ${port}: $(curl -s http://localhost:${port}/status 2>/dev/null | jq .result.latest_height)"
done

# Heal partition
docker network connect rustbft-net rustbft-node2
docker network connect rustbft-net rustbft-node3

# Verify: commits resume
sleep 10
curl -s http://localhost:26657/status | jq .result.latest_height
```

### 11.3 Slow Network

```bash
# Add 200ms latency to node1
docker exec rustbft-node1 tc qdisc add dev eth0 root netem delay 200ms

# Observe: higher round counts, longer commit times
# Check Grafana dashboard

# Remove latency
docker exec rustbft-node1 tc qdisc del dev eth0 root
```

### 11.4 Disk Full Simulation

```bash
# Fill the data volume for node3
docker exec rustbft-node3 dd if=/dev/zero of=/data/fill bs=1M count=1000

# Observe: node3 halts with storage error
docker logs rustbft-node3 | tail -5

# Clean up
docker exec rustbft-node3 rm /data/fill
docker restart rustbft-node3
```

### 11.5 Graceful Shutdown

```bash
# Send SIGTERM (graceful)
docker stop rustbft-node1

# Observe shutdown logs
docker logs rustbft-node1 | tail -10
# Should see: "Shutdown initiated", "WAL flushed", "Node stopped"
```

---

## 12. Monitoring

### 12.1 Access Dashboards

- **Prometheus:** http://localhost:9090
- **Grafana:** http://localhost:3000 (admin/admin)

### 12.2 Useful Prometheus Queries

```promql
# Current height per node
rustbft_consensus_height

# Block commit rate (blocks per minute)
rate(rustbft_consensus_height[1m]) * 60

# Average commit latency
histogram_quantile(0.95, rate(rustbft_consensus_block_commit_duration_seconds_bucket[5m]))

# Rounds per height (should be ~1 in healthy state)
histogram_quantile(0.99, rate(rustbft_consensus_rounds_per_height_bucket[5m]))

# Timeout rate
rate(rustbft_consensus_timeouts_total[5m])

# Peer count
rustbft_p2p_peers_connected

# Channel utilization
rustbft_channel_length / rustbft_channel_capacity
```

---

## 13. Directory Structure

```
devops/
├── Dockerfile
├── docker-compose.yml
├── prometheus.yml
├── grafana/
│   ├── provisioning/
│   │   ├── datasources/
│   │   │   └── prometheus.yml
│   │   └── dashboards/
│   │       └── dashboard.yml
│   └── dashboards/
│       ├── consensus.json
│       ├── network.json
│       ├── state-machine.json
│       └── infrastructure.json
├── config/
│   ├── genesis.json
│   ├── node0/
│   │   ├── node.toml
│   │   └── node_key.json
│   ├── node1/
│   │   ├── node.toml
│   │   └── node_key.json
│   ├── node2/
│   │   ├── node.toml
│   │   └── node_key.json
│   └── node3/
│       ├── node.toml
│       └── node_key.json
└── scripts/
    ├── init-cluster.sh
    ├── submit-tx.sh
    └── simulate-failure.sh
```

---

## Definition of Done — Docker

- [x] Docker Compose with 4 validators + Prometheus + Grafana
- [x] Dockerfile (multi-stage build)
- [x] Prometheus configuration
- [x] Node configuration (TOML + genesis)
- [x] Key generation procedure
- [x] Cluster start/stop/logs commands
- [x] Transaction submission examples
- [x] Contract deployment and calling examples
- [x] Validator set update examples
- [x] Failure simulation (crash, partition, slow network, disk full)
- [x] Monitoring queries
- [x] Directory structure
- [x] No Rust source code included
