# RustBFT — Operations Guide

**Purpose:** Define operational procedures for running, maintaining, and troubleshooting RustBFT nodes in production-like environments.
**Audience:** Operators and SREs responsible for running RustBFT clusters.

---

## 1. Node Lifecycle

### 1.1 First-Time Setup

```
1. Generate node keypair:
   rustbft-node keygen --output /config/node_key.json

2. Obtain genesis file from the network coordinator.
   Place at /config/genesis.json

3. Create node configuration:
   Copy template to /config/node.toml
   Set node_id, listen addresses, seed peers

4. Initialize data directory:
   rustbft-node init --config /config/node.toml
   (Creates /data/blocks, /data/state, /data/wal directories)

5. Start the node:
   rustbft-node --config /config/node.toml
```

### 1.2 Normal Startup

```
rustbft-node --config /config/node.toml

Startup sequence (logged at INFO):
    1. "Loading configuration"           config_path=/config/node.toml
    2. "Initializing storage"            engine=rocksdb, data_dir=/data
    3. "Loading last committed state"    height=42
    4. "Recovering from WAL"             entries=0 (or N if crash recovery)
    5. "Loading validator set"           validators=4, total_power=400
    6. "Starting P2P listener"           addr=0.0.0.0:26656
    7. "Connecting to seed peers"        seeds=3
    8. "Starting RPC server"             addr=0.0.0.0:26657
    9. "Starting metrics exporter"       addr=0.0.0.0:26660
    10. "Node started"                   height=42, node_id=node0
```

### 1.3 Graceful Shutdown

```
# Send SIGTERM
kill -TERM <pid>
# or
docker stop <container>

Expected behavior:
    1. Stop accepting new RPC requests
    2. Drain in-flight RPC requests (5s timeout)
    3. Close peer connections
    4. Flush WAL
    5. Flush storage
    6. Exit with code 0

If shutdown takes >30s, send SIGKILL as last resort.
```

### 1.4 Forced Shutdown

```
# Only if graceful shutdown hangs
kill -KILL <pid>
# or
docker kill <container>

Risk: In-flight WAL entries may be lost.
Recovery: WAL recovery on next startup handles this safely.
```

---

## 2. Configuration Management

### 2.1 Configuration Precedence

```
Priority (highest to lowest):
    1. Command-line flags
    2. Environment variables (RUSTBFT_ prefix)
    3. Config file (node.toml)
    4. Defaults

Example environment overrides:
    RUSTBFT_P2P_LISTEN_ADDR=0.0.0.0:26656
    RUSTBFT_RPC_LISTEN_ADDR=0.0.0.0:26657
    RUSTBFT_LOGGING_LEVEL=debug
    RUSTBFT_CONSENSUS_TIMEOUT_PROPOSE_MS=2000
```

### 2.2 Configuration Changes

| Parameter | Requires Restart? | Notes |
|-----------|-------------------|-------|
| `logging.level` | No | Can be changed via admin RPC |
| `p2p.seeds` | Yes | New peers discovered on restart |
| `consensus.timeout_*` | Yes | Affects consensus timing |
| `mempool.max_txs` | Yes | |
| `rpc.rate_limit_*` | Yes | |
| `storage.pruning_window` | No | Pruning adjusts on next cycle |

### 2.3 Sensitive Configuration

| Item | Storage | Notes |
|------|---------|-------|
| Node private key | `/config/node_key.json` | File permissions: 0600 |
| Admin API key | Environment variable | `RUSTBFT_ADMIN_API_KEY` |
| Genesis file | `/config/genesis.json` | Read-only after init |

---

## 3. Monitoring Procedures

### 3.1 Daily Checks

```
1. Verify all nodes are at the same height:
   for node in node0 node1 node2 node3; do
       curl -s http://${node}:26657/status | jq .result.latest_height
   done

2. Check peer connectivity:
   curl -s http://node0:26657/net_info | jq '.result.peers | length'

3. Review error logs (last 24h):
   docker logs --since 24h rustbft-node0 2>&1 | jq 'select(.level == "ERROR")'

4. Check Grafana dashboards for anomalies:
   - Block commit latency trending up?
   - Round count > 1 frequently?
   - Any channel drops?
```

### 3.2 Key Metrics to Watch

| Metric | Healthy Range | Action if Abnormal |
|--------|---------------|-------------------|
| `rustbft_consensus_height` | Increasing steadily | Check logs, peers, timeouts |
| `rustbft_consensus_round` | Usually 0 | If consistently >0, check proposer health |
| `rustbft_p2p_peers_connected` | ≥ n-1 (e.g., 3 for 4-node) | Check network, firewall |
| `rustbft_consensus_timeouts_total` | Low rate | If high, check proposer or network |
| `rustbft_channel_drops_total` | 0 | If >0, check downstream processing speed |
| `rustbft_storage_wal_write_duration_seconds` | < 10ms p99 | If high, check disk I/O |
| `rustbft_mempool_size` | < 80% of max | If full, check tx throughput |

### 3.3 Alerting

See `docs/observability/observability.md` §6 for alert definitions.

Recommended alert channels:
- **Critical:** PagerDuty / on-call rotation
- **Warning:** Slack channel

---

## 4. Common Operational Tasks

### 4.1 Adding a New Validator

```
Prerequisites:
    - New node is running and synced
    - Admin key is available

Steps:
    1. Generate keypair on new node
    2. Start new node with current genesis (it will sync blocks)
    3. Wait for new node to catch up to current height
    4. Submit ValidatorUpdate transaction:
       rustbft-cli tx validator-update \
           --action add \
           --validator-pubkey "ed25519:<pubkey>" \
           --voting-power 100 \
           --from admin \
           --key-file admin_key.json \
           --node http://node0:26657
    5. Verify at next height:
       rustbft-cli query validators --node http://node0:26657
    6. Verify new node is producing/signing blocks
```

### 4.2 Removing a Validator

```
Steps:
    1. Submit ValidatorUpdate transaction:
       rustbft-cli tx validator-update \
           --action remove \
           --validator-id nodeX \
           --from admin \
           --key-file admin_key.json \
           --node http://node0:26657
    2. Verify removal at next height
    3. Stop the removed node (optional)

Safety check:
    - Ensure remaining validators have >2/3 of total power
    - The system will reject the update if it would break quorum
```

### 4.3 Rolling Restart

```
For each node (one at a time):
    1. Verify cluster is healthy (all nodes at same height)
    2. Stop node:
       docker stop rustbft-nodeX
    3. Wait for remaining nodes to confirm continued progress:
       curl -s http://other-node:26657/status | jq .result.latest_height
    4. Apply configuration changes or binary update
    5. Start node:
       docker start rustbft-nodeX
    6. Wait for node to catch up:
       watch -n1 'curl -s http://nodeX:26657/status | jq .result.latest_height'
    7. Verify node is participating in consensus
    8. Proceed to next node

IMPORTANT: Never restart more than f nodes simultaneously (f < n/3).
For a 4-node cluster, restart ONE at a time.
```

### 4.4 Binary Upgrade

```
Steps:
    1. Build new binary
    2. Test on a single non-critical node first
    3. Perform rolling restart (§4.3) with new binary
    4. Monitor for errors after each node restart

Rollback:
    1. Stop the upgraded node
    2. Replace binary with previous version
    3. Start the node
    4. Verify it syncs and participates
```

### 4.5 Data Backup

```
# Create a backup of node0's data
docker exec rustbft-node0 rustbft-node backup --output /data/backup

# Copy backup out of container
docker cp rustbft-node0:/data/backup ./backups/node0-$(date +%Y%m%d)

# Restore from backup
docker stop rustbft-node0
docker cp ./backups/node0-20240115 rustbft-node0:/data/restore
docker exec rustbft-node0 rustbft-node restore --input /data/restore
docker start rustbft-node0
```

### 4.6 State Reset

```
# WARNING: This deletes all local data. The node will re-sync from peers.

docker stop rustbft-node0
docker volume rm rustbft_node0-data
docker start rustbft-node0

# The node will sync blocks from peers starting from genesis.
```

---

## 5. Troubleshooting

### 5.1 Node Not Committing Blocks

```
Diagnosis:
    1. Check height: curl -s http://node:26657/status | jq .result.latest_height
    2. Check round: curl -s http://node:26657/status | jq .result.consensus.round
    3. Check peers: curl -s http://node:26657/net_info | jq '.result.peers | length'

Possible causes:
    - Round > 0 consistently → proposer may be offline
    - 0 peers → network issue
    - Height stalled → check if >2/3 of validators are online
    - Logs show "STATE ROOT MISMATCH" → critical bug, see §5.4
```

### 5.2 Node Falling Behind

```
Symptoms:
    - Node's height < other nodes' height
    - status.catching_up = true

Diagnosis:
    1. Check if node was recently restarted (WAL recovery)
    2. Check disk I/O (slow storage)
    3. Check network (high latency to peers)
    4. Check CPU (block execution bottleneck)

Resolution:
    - Wait for catch-up (normal after restart)
    - If persistently behind: check disk performance
    - If very far behind: consider state reset and re-sync
```

### 5.3 Peer Connection Issues

```
Symptoms:
    - 0 peers connected
    - Logs show "Connection refused" or "Handshake failed"

Diagnosis:
    1. Check firewall rules (port 26656 must be open)
    2. Check seed addresses in config
    3. Check if peers are running
    4. Check chain_id matches across all nodes
    5. Check protocol version compatibility

Resolution:
    - Fix firewall / security groups
    - Correct seed addresses
    - Ensure all nodes use same genesis file
```

### 5.4 State Root Mismatch (Critical)

```
Symptoms:
    - Node halts with ERROR "STATE ROOT MISMATCH"
    - This means the node computed a different state than other nodes

This is a CRITICAL BUG indicating non-deterministic execution.

Immediate actions:
    1. DO NOT restart the node (preserve state for debugging)
    2. Collect logs from ALL nodes
    3. Collect the block that caused the mismatch
    4. Compare state diffs between the divergent node and a correct node
    5. File a bug report with all collected data

Recovery:
    1. After the bug is identified and fixed
    2. State reset the affected node
    3. Re-sync from peers with the fixed binary
```

### 5.5 WAL Recovery Issues

```
Symptoms:
    - Node fails to start after crash
    - Logs show "WAL recovery failed" or "Invalid WAL entry"

Diagnosis:
    1. Check WAL file integrity
    2. Look for CRC32 errors in logs

Resolution:
    - If WAL is corrupted: delete WAL file, node will start from last committed height
      (may lose uncommitted votes, which is safe)
    - If storage is corrupted: restore from backup or state reset
```

---

## 6. Capacity Planning

### 6.1 Resource Requirements (Per Node)

| Resource | Minimum | Recommended |
|----------|---------|-------------|
| CPU | 2 cores | 4 cores |
| RAM | 512 MB | 2 GB |
| Disk | 10 GB | 100 GB (depends on retention) |
| Network | 10 Mbps | 100 Mbps |

### 6.2 Disk Growth

```
Approximate disk usage per day (at 1 block/second):
    Blocks:     ~86,400 blocks × ~1 KB avg = ~84 MB/day
    State:      Depends on transaction volume
    WAL:        Negligible (truncated after each commit)
    Logs:       ~50 MB/day at INFO level

With pruning_window = 1000:
    State overhead ≈ 1000 × state_delta_per_block
```

### 6.3 Scaling Considerations

| Factor | Impact |
|--------|--------|
| More validators | Higher message volume (O(n²) votes) |
| More transactions | Larger blocks, longer execution time |
| Larger contracts | More gas, more state writes |
| Longer retention | More disk usage |

---

## 7. Security Checklist

```
[ ] Node private keys have restrictive file permissions (0600)
[ ] RPC is bound to localhost or behind a reverse proxy
[ ] Admin endpoints require authentication
[ ] P2P port is firewalled to known validator IPs
[ ] Metrics port is not publicly accessible
[ ] Docker containers run as non-root user
[ ] Binary is built from audited source
[ ] Genesis file hash is verified across all nodes
[ ] Log output does not contain private keys
[ ] Backups are encrypted at rest
```

---

## Definition of Done — Operations

- [x] Node lifecycle (setup, start, shutdown) documented
- [x] Configuration management (precedence, changes, sensitive data)
- [x] Monitoring procedures (daily checks, key metrics)
- [x] Common operational tasks (add/remove validator, rolling restart, upgrade, backup)
- [x] Troubleshooting guide for common issues
- [x] Capacity planning (resources, disk growth, scaling)
- [x] Security checklist
- [x] No Rust source code included
