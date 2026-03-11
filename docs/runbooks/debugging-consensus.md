# RustBFT — Debugging Consensus Runbook

**Purpose:** Provide step-by-step workflows for diagnosing and resolving consensus issues in a running RustBFT cluster.
**Audience:** Engineers and operators debugging consensus failures.

---

## 1. Debugging Toolkit

### 1.1 Available Tools

| Tool | Purpose | Access |
|------|---------|--------|
| RPC `/status` | Current height, round, step, peer count | `curl http://node:26657/status` |
| RPC `/net_info` | Connected peers and their state | `curl http://node:26657/net_info` |
| RPC `/get_validators` | Current validator set | `curl http://node:26657/get_validators` |
| Prometheus metrics | Time-series data for all subsystems | `http://node:26660/metrics` |
| Grafana dashboards | Visual monitoring | `http://grafana:3000` |
| Structured logs | Detailed event-by-event trace | `docker logs <container>` |
| WAL inspection | In-progress consensus state | `rustbft-node wal-inspect --data-dir /data` |
| Block inspection | Committed block contents | `rustbft-cli query block --height N` |

### 1.2 Log Filtering

```bash
# All consensus events for a specific height
docker logs rustbft-node0 2>&1 | jq 'select(.height == 42)'

# All errors
docker logs rustbft-node0 2>&1 | jq 'select(.level == "ERROR")'

# All timeout events
docker logs rustbft-node0 2>&1 | jq 'select(.message | contains("Timeout"))'

# All vote events for a specific round
docker logs rustbft-node0 2>&1 | jq 'select(.height == 42 and .round == 3 and (.message | contains("vote")))'

# Equivocation evidence
docker logs rustbft-node0 2>&1 | jq 'select(.message | contains("Equivocation"))'
```

---

## 2. Symptom: Height Not Advancing

### 2.1 Diagnosis Flowchart

```
Height not advancing
    │
    ├── Check: How many nodes are online?
    │   │
    │   ├── < 2/3 of voting power online
    │   │   → EXPECTED: System cannot commit without quorum
    │   │   → ACTION: Bring more validators online
    │   │
    │   └── ≥ 2/3 of voting power online
    │       │
    │       ├── Check: Are nodes connected to each other?
    │       │   │
    │       │   ├── Peers = 0 on some nodes
    │       │   │   → NETWORK ISSUE: See §2.2
    │       │   │
    │       │   └── Peers > 0 on all nodes
    │       │       │
    │       │       ├── Check: What round are nodes on?
    │       │       │   │
    │       │       │   ├── All on round 0, step = Propose
    │       │       │   │   → PROPOSER ISSUE: See §2.3
    │       │       │   │
    │       │       │   ├── Different rounds across nodes
    │       │       │   │   → SYNCHRONIZATION ISSUE: See §2.4
    │       │       │   │
    │       │       │   └── High round number (>5)
    │       │       │       → PERSISTENT FAILURE: See §2.5
    │       │       │
    │       │       └── Check: Are nodes at the same height?
    │       │           │
    │       │           ├── Different heights
    │       │           │   → SYNC ISSUE: See §2.6
    │       │           │
    │       │           └── Same height
    │       │               → Check logs for specific errors
    │       │
    │       └── (continue diagnosis)
    │
    └── Check: Is the node halted?
        │
        ├── Log shows "STATE ROOT MISMATCH"
        │   → CRITICAL BUG: See §5
        │
        └── Log shows "Storage error"
            → STORAGE FAILURE: See §6
```

### 2.2 Network Issue

```
Symptoms:
    - Nodes report 0 peers
    - Logs show "Connection refused" or "Handshake timeout"

Steps:
    1. Verify network connectivity:
       docker exec rustbft-node0 ping 172.28.1.2

    2. Verify P2P port is open:
       docker exec rustbft-node0 nc -zv 172.28.1.2 26656

    3. Check seed configuration:
       docker exec rustbft-node0 cat /config/node.toml | grep seeds

    4. Check chain_id matches:
       for port in 26657 26756 26856 26956; do
           curl -s http://localhost:${port}/status | jq .result.chain_id
       done

    5. Check for firewall rules:
       docker exec rustbft-node0 iptables -L

Resolution:
    - Fix network configuration
    - Ensure all nodes use the same chain_id
    - Restart affected nodes after fixing config
```

### 2.3 Proposer Issue

```
Symptoms:
    - All nodes stuck at step = Propose
    - Round 0, no proposal received
    - Timeout_propose fires repeatedly

Steps:
    1. Identify the proposer for the current (height, round):
       # The proposer is deterministic — check which validator should propose
       curl -s http://localhost:26657/status | jq '{height: .result.consensus.height, round: .result.consensus.round}'

    2. Check if the proposer node is online:
       curl -s http://<proposer-node>:26657/health

    3. Check if the proposer is connected to peers:
       curl -s http://<proposer-node>:26657/net_info | jq '.result.peers | length'

    4. Check proposer logs for errors:
       docker logs rustbft-<proposer> 2>&1 | jq 'select(.level == "ERROR")' | tail -5

Resolution:
    - If proposer is offline: wait for timeout, system will advance to next round
    - If proposer is online but not proposing: check mempool, check logs for errors
    - If proposer is partitioned: wait for timeout + round advancement
```

### 2.4 Synchronization Issue

```
Symptoms:
    - Nodes are on different rounds
    - Some nodes advancing rounds faster than others

Steps:
    1. Compare state across all nodes:
       for port in 26657 26756 26856 26956; do
           echo "=== Port ${port} ==="
           curl -s http://localhost:${port}/status | jq '{height: .result.consensus.height, round: .result.consensus.round, step: .result.consensus.step}'
       done

    2. Check message delivery:
       # Look for "Channel full" warnings
       docker logs rustbft-node0 2>&1 | jq 'select(.message | contains("Channel full"))'

    3. Check network latency between nodes:
       docker exec rustbft-node0 ping -c 5 172.28.1.2

Resolution:
    - Usually self-resolving: round skip mechanism will synchronize nodes
    - If persistent: check for network issues or overloaded nodes
    - Increase timeout values if network is high-latency
```

### 2.5 Persistent High Rounds

```
Symptoms:
    - Round number consistently > 3
    - Blocks eventually commit but slowly

Steps:
    1. Check which rounds are failing and why:
       docker logs rustbft-node0 2>&1 | jq 'select(.message | contains("Timeout") and .height == 42)'

    2. Check if proposals are being received:
       docker logs rustbft-node0 2>&1 | jq 'select(.message | contains("Received proposal") and .height == 42)'

    3. Check if votes are being received:
       docker logs rustbft-node0 2>&1 | jq 'select(.message | contains("Received vote") and .height == 42)' | jq .round | sort | uniq -c

    4. Check for equivocation:
       docker logs rustbft-node0 2>&1 | jq 'select(.message | contains("Equivocation"))'

Resolution:
    - If proposals not received: proposer connectivity issue
    - If votes not received: gossip issue, check peer connections
    - If equivocation detected: byzantine validator, investigate
    - Consider increasing timeout values
```

### 2.6 Height Mismatch Between Nodes

```
Symptoms:
    - One or more nodes are at a lower height than others
    - status.catching_up = true

Steps:
    1. Check how far behind:
       # Compare heights
       for port in 26657 26756 26856 26956; do
           curl -s http://localhost:${port}/status | jq '{port: '${port}', height: .result.latest_height, catching_up: .result.catching_up}'
       done

    2. Check if the behind node is making progress:
       watch -n5 'curl -s http://localhost:26857/status | jq .result.latest_height'

    3. Check for storage errors on the behind node:
       docker logs rustbft-node2 2>&1 | jq 'select(.level == "ERROR")'

Resolution:
    - If catching_up = true and height is increasing: wait for sync
    - If height is not increasing: check storage, check peer connections
    - If very far behind: consider state reset and re-sync
```

---

## 3. Symptom: Slow Block Commits

```
Diagnosis:
    1. Check commit latency:
       # Prometheus query
       histogram_quantile(0.95, rate(rustbft_consensus_block_commit_duration_seconds_bucket[5m]))

    2. Check rounds per height:
       histogram_quantile(0.95, rate(rustbft_consensus_rounds_per_height_bucket[5m]))

    3. Check block execution time:
       histogram_quantile(0.95, rate(rustbft_state_block_execution_duration_seconds_bucket[5m]))

    4. Check WAL write latency:
       histogram_quantile(0.95, rate(rustbft_storage_wal_write_duration_seconds_bucket[5m]))

Possible causes and resolutions:
    - High rounds per height → network or proposer issues (see §2)
    - High execution time → large blocks, complex contracts, slow disk
    - High WAL latency → disk I/O bottleneck, consider faster storage
    - High network latency → check inter-node latency
```

---

## 4. Symptom: Equivocation Detected

```
Severity: HIGH — indicates a byzantine or misconfigured validator

Steps:
    1. Identify the equivocating validator:
       docker logs rustbft-node0 2>&1 | jq 'select(.message | contains("Equivocation"))' | jq .validator

    2. Collect evidence:
       # The log entry contains both conflicting votes
       docker logs rustbft-node0 2>&1 | jq 'select(.message | contains("Equivocation"))'

    3. Check if the validator is running multiple instances:
       # A common cause of accidental equivocation is running
       # two copies of the same validator with the same key
       # Check all hosts for processes with the same node_id

    4. Check the validator's logs for errors:
       docker logs rustbft-<equivocating-node> 2>&1 | tail -50

Resolution:
    - If accidental (duplicate instance): stop the duplicate immediately
    - If intentional (byzantine): remove the validator via ValidatorUpdate
    - Preserve evidence for post-mortem analysis
    - Consider banning the validator's peer connection
```

---

## 5. Symptom: State Root Mismatch

```
Severity: CRITICAL — indicates a determinism bug

This means two honest nodes executed the same block and got different results.
The node that detected the mismatch has HALTED.

IMMEDIATE ACTIONS (do these before anything else):

    1. DO NOT restart the halted node
    2. DO NOT delete any data
    3. Preserve the halted node's state:
       docker cp rustbft-<halted-node>:/data ./debug/halted-node-data
       docker logs rustbft-<halted-node> > ./debug/halted-node-logs.json

    4. Identify the divergent height:
       grep "STATE ROOT MISMATCH" ./debug/halted-node-logs.json

    5. Collect the block at the divergent height from a healthy node:
       curl -s http://healthy-node:26657/get_block?height=<H> > ./debug/block-<H>.json

    6. Collect state at height H-1 from both nodes:
       # From halted node (if accessible):
       rustbft-node state-dump --data-dir ./debug/halted-node-data --height <H-1>
       # From healthy node:
       curl -s http://healthy-node:26657/get_account?height=<H-1>&address=<each-account>

    7. Compare state diffs:
       diff ./debug/halted-state-<H-1>.json ./debug/healthy-state-<H-1>.json

    8. File a detailed bug report with:
       - Divergent height
       - Block contents at that height
       - State before and after on both nodes
       - Full logs from both nodes around the divergent height
       - Binary versions and platform info

ROOT CAUSE INVESTIGATION:
    Common causes of state root mismatch:
    - HashMap used instead of BTreeMap (non-deterministic iteration)
    - Floating-point arithmetic in execution path
    - Platform-dependent serialization
    - Non-deterministic WASM execution (should not happen with wasmtime)
    - Race condition in state machine (should not happen with single-threaded design)
    - Bug in Merkle tree implementation
```

---

## 6. Symptom: Storage Failure

```
Symptoms:
    - Node halts with "Storage error" in logs
    - RocksDB returns corruption or I/O error

Steps:
    1. Check disk space:
       docker exec rustbft-node0 df -h /data

    2. Check disk health:
       # Host-level check
       dmesg | grep -i error
       smartctl -a /dev/sda

    3. Check RocksDB logs:
       docker exec rustbft-node0 cat /data/LOG

    4. Attempt recovery:
       # Stop the node
       docker stop rustbft-node0
       
       # Try RocksDB repair
       rustbft-node repair-db --data-dir /data
       
       # Restart
       docker start rustbft-node0

    5. If repair fails:
       # Restore from backup
       # Or state reset and re-sync from peers
```

---

## 7. Symptom: Memory Growth

```
Steps:
    1. Check memory usage:
       docker stats rustbft-node0

    2. Check mempool size:
       curl -s http://localhost:26657/status | jq .result.mempool_size

    3. Check channel lengths:
       curl -s http://localhost:26660/metrics | grep rustbft_channel_length

    4. Check RocksDB cache:
       curl -s http://localhost:26660/metrics | grep rustbft_storage_db_size

Resolution:
    - If mempool is large: reduce max_txs, increase block gas limit
    - If channels are full: investigate downstream bottleneck
    - If DB cache is large: reduce rocksdb_cache_size_mb
    - If none of the above: potential memory leak, collect heap profile
```

---

## 8. WAL Inspection

```bash
# Inspect WAL contents (for debugging, not normal operation)
rustbft-node wal-inspect --data-dir /data

# Output:
# Entry 0: RoundStarted { height: 42, round: 0 }
# Entry 1: ProposalSent { height: 42, round: 0, block_hash: "abc..." }
# Entry 2: VoteSent { height: 42, round: 0, type: Prevote, block_hash: "abc..." }
# Entry 3: Locked { height: 42, round: 0, block_hash: "abc..." }
# Entry 4: VoteSent { height: 42, round: 0, type: Precommit, block_hash: "abc..." }
# Entry 5: CommitStarted { height: 42, block_hash: "abc..." }
# (no CommitCompleted → crash during execution)

# This tells you exactly where the node was when it crashed
```

---

## 9. Deterministic Replay for Debugging

```bash
# Record events during a problematic run
RUSTBFT_RECORD_EVENTS=true rustbft-node --config /config/node.toml
# Events are written to /data/debug/events.log

# Replay events to reproduce the issue
rustbft-node replay --events /data/debug/events.log --verbose

# This runs the consensus core with the exact same event sequence
# and outputs every state transition for inspection
```

---

## 10. Collecting a Debug Bundle

When filing a bug report, collect the following:

```bash
#!/bin/bash
# scripts/collect-debug-bundle.sh

NODE=$1
OUTDIR="debug-bundle-$(date +%Y%m%d-%H%M%S)"
mkdir -p $OUTDIR

# Node status
curl -s http://${NODE}:26657/status > $OUTDIR/status.json
curl -s http://${NODE}:26657/net_info > $OUTDIR/net_info.json
curl -s http://${NODE}:26657/get_validators > $OUTDIR/validators.json

# Logs (last 1 hour)
docker logs --since 1h rustbft-${NODE} > $OUTDIR/logs.json 2>&1

# Metrics snapshot
curl -s http://${NODE}:26660/metrics > $OUTDIR/metrics.txt

# Configuration (redact private keys)
docker exec rustbft-${NODE} cat /config/node.toml > $OUTDIR/config.toml

# WAL state
docker exec rustbft-${NODE} rustbft-node wal-inspect --data-dir /data > $OUTDIR/wal.txt

# System info
docker exec rustbft-${NODE} uname -a > $OUTDIR/system.txt
docker exec rustbft-${NODE} df -h /data >> $OUTDIR/system.txt
docker exec rustbft-${NODE} free -m >> $OUTDIR/system.txt

# Package
tar czf ${OUTDIR}.tar.gz $OUTDIR
echo "Debug bundle: ${OUTDIR}.tar.gz"
```

---

## Definition of Done — Debugging Consensus

- [x] Debugging toolkit enumerated
- [x] Log filtering examples
- [x] Diagnosis flowchart for "height not advancing"
- [x] Sub-workflows for network, proposer, sync, and round issues
- [x] Slow commit diagnosis
- [x] Equivocation investigation procedure
- [x] State root mismatch critical procedure
- [x] Storage failure recovery
- [x] Memory growth diagnosis
- [x] WAL inspection procedure
- [x] Deterministic replay for debugging
- [x] Debug bundle collection script
- [x] No Rust source code included
