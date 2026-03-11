# RustBFT — Observability

**Purpose:** Define the metrics, structured logging, health checks, and debugging infrastructure for operating RustBFT nodes.
**Audience:** Engineers implementing observability and operators running the system.

---

## 1. Observability Pillars

```
┌─────────────────────────────────────────────────────────┐
│                   OBSERVABILITY                          │
│                                                          │
│  ┌──────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │ Metrics  │  │  Structured  │  │  Health Checks   │  │
│  │          │  │  Logging     │  │                  │  │
│  │Prometheus│  │  JSON to     │  │  /health         │  │
│  │ /metrics │  │  stdout/file │  │  /status         │  │
│  └──────────┘  └──────────────┘  └──────────────────┘  │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

---

## 2. Prometheus Metrics

### 2.1 Metrics Endpoint

- **Path:** `GET /metrics`
- **Port:** 26660 (separate from RPC port 26657)
- **Format:** Prometheus text exposition format

### 2.2 Consensus Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `rustbft_consensus_height` | Gauge | `node_id` | Current consensus height |
| `rustbft_consensus_round` | Gauge | `node_id` | Current consensus round |
| `rustbft_consensus_step` | Gauge | `node_id`, `step` | Current consensus step (encoded as int: Propose=0, Prevote=1, Precommit=2, Commit=3) |
| `rustbft_consensus_block_commit_duration_seconds` | Histogram | `node_id` | Time from entering height to commit |
| `rustbft_consensus_round_duration_seconds` | Histogram | `node_id`, `round` | Duration of each round |
| `rustbft_consensus_rounds_per_height` | Histogram | `node_id` | Number of rounds needed to commit |
| `rustbft_consensus_proposals_received_total` | Counter | `node_id`, `valid` | Proposals received (valid/invalid) |
| `rustbft_consensus_votes_received_total` | Counter | `node_id`, `type`, `valid` | Votes received by type (prevote/precommit) and validity |
| `rustbft_consensus_timeouts_total` | Counter | `node_id`, `type` | Timeouts fired by type (propose/prevote/precommit) |
| `rustbft_consensus_equivocations_total` | Counter | `node_id` | Equivocation evidence detected |

### 2.3 Networking Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `rustbft_p2p_peers_connected` | Gauge | `node_id` | Number of connected peers |
| `rustbft_p2p_messages_sent_total` | Counter | `node_id`, `msg_type` | Messages sent by type |
| `rustbft_p2p_messages_received_total` | Counter | `node_id`, `msg_type` | Messages received by type |
| `rustbft_p2p_bytes_sent_total` | Counter | `node_id` | Total bytes sent |
| `rustbft_p2p_bytes_received_total` | Counter | `node_id` | Total bytes received |
| `rustbft_p2p_message_latency_seconds` | Histogram | `node_id`, `msg_type` | Time from send to receive (estimated) |
| `rustbft_p2p_peer_score` | Gauge | `node_id`, `peer_id` | Current peer score |
| `rustbft_p2p_connection_errors_total` | Counter | `node_id`, `error_type` | Connection errors by type |

### 2.4 Mempool Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `rustbft_mempool_size` | Gauge | `node_id` | Number of transactions in mempool |
| `rustbft_mempool_bytes` | Gauge | `node_id` | Total bytes of transactions in mempool |
| `rustbft_mempool_txs_added_total` | Counter | `node_id`, `source` | Transactions added (rpc/p2p) |
| `rustbft_mempool_txs_rejected_total` | Counter | `node_id`, `reason` | Transactions rejected by reason |
| `rustbft_mempool_txs_evicted_total` | Counter | `node_id` | Transactions evicted after commit |
| `rustbft_mempool_reap_duration_seconds` | Histogram | `node_id` | Time to reap transactions for a block |

### 2.5 State Machine Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `rustbft_state_block_execution_duration_seconds` | Histogram | `node_id` | Time to execute a full block |
| `rustbft_state_tx_execution_duration_seconds` | Histogram | `node_id`, `tx_type` | Time per transaction |
| `rustbft_state_tx_success_total` | Counter | `node_id` | Successful transactions |
| `rustbft_state_tx_failure_total` | Counter | `node_id`, `error_type` | Failed transactions by error |
| `rustbft_state_gas_used_total` | Counter | `node_id` | Total gas consumed |
| `rustbft_state_state_root_computation_seconds` | Histogram | `node_id` | Time to compute state root |

### 2.6 Contract Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `rustbft_contract_executions_total` | Counter | `node_id`, `result` | Contract executions (success/failure) |
| `rustbft_contract_gas_used` | Histogram | `node_id` | Gas consumed per contract call |
| `rustbft_contract_deploy_total` | Counter | `node_id` | Contract deployments |
| `rustbft_contract_call_depth` | Histogram | `node_id` | Cross-contract call depth |

### 2.7 Storage Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `rustbft_storage_block_persist_duration_seconds` | Histogram | `node_id` | Time to persist a block |
| `rustbft_storage_wal_write_duration_seconds` | Histogram | `node_id` | Time per WAL write (including fsync) |
| `rustbft_storage_wal_entries` | Gauge | `node_id` | Current WAL entry count |
| `rustbft_storage_db_size_bytes` | Gauge | `node_id`, `cf` | Database size by column family |
| `rustbft_storage_pruning_duration_seconds` | Histogram | `node_id` | Time per pruning cycle |

### 2.8 Channel Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `rustbft_channel_length` | Gauge | `node_id`, `channel` | Current items in channel |
| `rustbft_channel_capacity` | Gauge | `node_id`, `channel` | Channel capacity |
| `rustbft_channel_drops_total` | Counter | `node_id`, `channel` | Messages dropped due to full channel |

### 2.9 RPC Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `rustbft_rpc_requests_total` | Counter | `node_id`, `method`, `status` | RPC requests by method and status |
| `rustbft_rpc_request_duration_seconds` | Histogram | `node_id`, `method` | RPC request latency |
| `rustbft_rpc_active_connections` | Gauge | `node_id` | Active HTTP connections |

---

## 3. Structured Logging

### 3.1 Format

All logs are emitted as JSON to stdout:

```json
{
    "timestamp": "2024-01-15T10:30:45.123456Z",
    "level": "INFO",
    "target": "rustbft_consensus",
    "node_id": "node0",
    "height": 42,
    "round": 0,
    "step": "Prevote",
    "message": "Cast prevote",
    "block_hash": "abcdef12...",
    "fields": {
        "validator_count": 4,
        "votes_received": 3
    }
}
```

### 3.2 Log Levels

| Level | Usage |
|-------|-------|
| `ERROR` | Unrecoverable errors requiring operator attention (state root mismatch, storage corruption, node halt) |
| `WARN` | Recoverable issues (peer disconnected, channel full, invalid message received, timeout fired) |
| `INFO` | Significant state changes (new height committed, validator set changed, node started/stopped) |
| `DEBUG` | Detailed protocol flow (vote received, proposal validated, state transition) |
| `TRACE` | Wire-level detail (raw message bytes, channel send/recv, timer scheduling) |

### 3.3 Mandatory Log Fields

Every consensus-related log entry MUST include:

| Field | Type | Description |
|-------|------|-------------|
| `node_id` | string | This node's identifier |
| `height` | u64 | Current consensus height |
| `round` | u32 | Current consensus round |
| `step` | string | Current consensus step |

### 3.4 Key Log Events

| Event | Level | Message | Additional Fields |
|-------|-------|---------|-------------------|
| Node started | INFO | "Node started" | version, chain_id, listen_addr |
| New height | INFO | "Committed block" | height, block_hash, state_root, tx_count, duration_ms |
| Proposal sent | DEBUG | "Sent proposal" | height, round, block_hash, tx_count |
| Proposal received | DEBUG | "Received proposal" | height, round, block_hash, proposer, valid |
| Vote cast | DEBUG | "Cast vote" | height, round, vote_type, block_hash |
| Vote received | DEBUG | "Received vote" | height, round, vote_type, voter, block_hash |
| Polka formed | INFO | "Polka formed" | height, round, block_hash |
| Timeout fired | WARN | "Timeout fired" | height, round, timeout_type |
| Equivocation | ERROR | "Equivocation detected" | height, round, validator, vote_a_hash, vote_b_hash |
| Peer connected | INFO | "Peer connected" | peer_id, remote_addr, direction |
| Peer disconnected | WARN | "Peer disconnected" | peer_id, reason |
| Channel full | WARN | "Channel full, dropping message" | channel_name, capacity |
| State root mismatch | ERROR | "STATE ROOT MISMATCH — HALTING" | height, expected, computed |
| Validator set changed | INFO | "Validator set updated" | height, added, removed, new_total_power |

### 3.5 Logging Library

Use `tracing` crate with `tracing-subscriber` for:
- Structured JSON output (`tracing-subscriber::fmt::json()`)
- Per-module log level filtering (`RUST_LOG=rustbft_consensus=debug,rustbft_p2p=info`)
- Span-based context (height, round automatically attached to all logs within a span)

### 3.6 Runtime Log Level Changes

Log levels can be changed at runtime via an RPC endpoint:

```
POST /admin/log_level
{
    "target": "rustbft_consensus",
    "level": "TRACE"
}
```

This is gated behind an admin authentication mechanism (API key or localhost-only).

---

## 4. Health Checks

### 4.1 Health Endpoint

```
GET /health → 200 OK or 503 Service Unavailable

Response:
{
    "healthy": true,
    "checks": {
        "consensus": { "status": "ok", "height": 42, "round": 0 },
        "peers": { "status": "ok", "count": 3 },
        "storage": { "status": "ok", "latest_height": 42 },
        "mempool": { "status": "ok", "size": 15 }
    }
}
```

### 4.2 Health Check Criteria

| Check | Healthy | Unhealthy |
|-------|---------|-----------|
| Consensus | Height advanced in last 30s | Height stalled for >30s |
| Peers | ≥1 peer connected | 0 peers connected |
| Storage | Latest height matches consensus | Storage height < consensus height - 1 |
| Mempool | Size < max_size | Size = max_size (full) |

### 4.3 Readiness vs. Liveness

| Endpoint | Purpose | Failure Action |
|----------|---------|----------------|
| `/health` (liveness) | Is the process alive and functional? | Restart container |
| `/ready` (readiness) | Is the node ready to serve requests? | Remove from load balancer |

Readiness requires:
- Node has completed initial sync (not catching up).
- At least one peer is connected.
- Storage is accessible.

---

## 5. Grafana Dashboards

### 5.1 Consensus Dashboard

```
┌─────────────────────────────────────────────────────────┐
│                 CONSENSUS DASHBOARD                      │
│                                                          │
│  ┌──────────────────┐  ┌──────────────────┐             │
│  │ Current Height   │  │ Current Round    │             │
│  │     42           │  │     0            │             │
│  └──────────────────┘  └──────────────────┘             │
│                                                          │
│  ┌──────────────────────────────────────────────────┐   │
│  │ Block Commit Latency (p50, p95, p99)             │   │
│  │ ▁▂▃▄▅▆▇█▇▆▅▄▃▂▁▂▃▄▅▆▇█▇▆▅▄▃▂▁                  │   │
│  └──────────────────────────────────────────────────┘   │
│                                                          │
│  ┌──────────────────────────────────────────────────┐   │
│  │ Rounds Per Height                                │   │
│  │ ▁▁▁▁▁▁▁▂▁▁▁▁▁▁▁▁▁▁▁▃▁▁▁▁▁▁▁▁▁▁                  │   │
│  └──────────────────────────────────────────────────┘   │
│                                                          │
│  ┌──────────────────────────────────────────────────┐   │
│  │ Timeouts (propose / prevote / precommit)         │   │
│  │ ▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁                  │   │
│  └──────────────────────────────────────────────────┘   │
│                                                          │
│  ┌──────────────────────────────────────────────────┐   │
│  │ Equivocations Detected                           │   │
│  │ 0                                                │   │
│  └──────────────────────────────────────────────────┘   │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

### 5.2 Network Dashboard

Panels: peer count over time, bytes sent/received, message rates by type, connection errors.

### 5.3 State Machine Dashboard

Panels: block execution time, tx throughput, gas usage, success/failure rates.

### 5.4 Infrastructure Dashboard

Panels: channel utilization, storage write latency, WAL size, DB size, memory/CPU usage (from node_exporter).

---

## 6. Alerting Rules

### 6.1 Critical Alerts (Page Operator)

| Alert | Condition | Severity |
|-------|-----------|----------|
| Node halted | `rustbft_consensus_height` unchanged for 5 minutes | Critical |
| State root mismatch | Log contains "STATE ROOT MISMATCH" | Critical |
| Storage error | Log contains storage error at ERROR level | Critical |
| Zero peers | `rustbft_p2p_peers_connected == 0` for 2 minutes | Critical |

### 6.2 Warning Alerts

| Alert | Condition | Severity |
|-------|-----------|----------|
| High round count | `rustbft_consensus_round > 3` | Warning |
| Mempool full | `rustbft_mempool_size >= max_size * 0.9` | Warning |
| Channel drops | `rate(rustbft_channel_drops_total[5m]) > 0` | Warning |
| Slow block execution | `rustbft_state_block_execution_duration_seconds{quantile="0.99"} > 2` | Warning |
| Peer score low | `rustbft_p2p_peer_score < 20` | Warning |

---

## 7. Graceful Shutdown Observability

During shutdown:

```
INFO  "Shutdown initiated" signal=SIGTERM
INFO  "Draining RPC requests" pending=3
INFO  "Closing peer connections" peers=3
INFO  "Flushing WAL" entries=2
INFO  "Consensus core stopped" height=42 round=0
INFO  "Storage flushed" 
INFO  "Node stopped" uptime_seconds=3600
```

---

## 8. Configuration

```toml
[observability]
metrics_enabled = true
metrics_listen_addr = "0.0.0.0:26660"

[logging]
format = "json"                    # "json" or "text"
level = "info"                     # Default log level
module_levels = "rustbft_consensus=debug,rustbft_p2p=info"
output = "stdout"                  # "stdout" or "file"
file_path = "./logs/rustbft.log"   # If output = "file"
file_rotation = "daily"            # "daily" or "size:100MB"
```

---

## Definition of Done — Observability

- [x] All Prometheus metrics defined with types, labels, and descriptions
- [x] Structured logging format specified (JSON)
- [x] Log levels defined with usage guidelines
- [x] Mandatory log fields for consensus events
- [x] Key log events enumerated
- [x] Health check endpoints with criteria
- [x] Readiness vs. liveness distinction
- [x] Grafana dashboard layouts described
- [x] Alerting rules (critical and warning)
- [x] Shutdown observability
- [x] Configuration parameters
- [x] No Rust source code included
