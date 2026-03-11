# RustBFT — Failure Models

**Purpose:** Enumerate all failure modes the system may encounter, classify them by type and severity, and define the expected system behavior and recovery for each.
**Audience:** Engineers designing fault tolerance and operators understanding system behavior under failure.

---

## 1. Failure Classification

### 1.1 Failure Types

```
┌─────────────────────────────────────────────────────────────┐
│                    FAILURE TAXONOMY                          │
│                                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │    Crash      │  │   Byzantine  │  │    Network       │  │
│  │              │  │              │  │                  │  │
│  │ Node stops   │  │ Node behaves │  │ Messages lost,   │  │
│  │ responding.  │  │ arbitrarily. │  │ delayed, or      │  │
│  │ No malicious │  │ May lie,     │  │ reordered.       │  │
│  │ behavior.    │  │ equivocate,  │  │ Partitions.      │  │
│  │              │  │ or withhold. │  │                  │  │
│  └──────────────┘  └──────────────┘  └──────────────────┘  │
│                                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │   Storage    │  │  Execution   │  │   Operational    │  │
│  │              │  │              │  │                  │  │
│  │ Disk full,   │  │ Contract     │  │ Misconfiguration │  │
│  │ corruption,  │  │ panics, gas  │  │ key loss, wrong  │  │
│  │ I/O errors.  │  │ exhaustion,  │  │ genesis, version │  │
│  │              │  │ determinism  │  │ mismatch.        │  │
│  │              │  │ bugs.        │  │                  │  │
│  └──────────────┘  └──────────────┘  └──────────────────┘  │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 1.2 Severity Levels

| Level | Definition | System Response |
|-------|-----------|-----------------|
| **S0 — Critical** | Safety violation possible or data corruption | HALT node immediately. Operator must investigate. |
| **S1 — Major** | Liveness impacted for the cluster | Automatic recovery attempted. Alert operator. |
| **S2 — Minor** | Single node degraded, cluster unaffected | Automatic recovery. Log warning. |
| **S3 — Informational** | Expected transient condition | Log and continue. |

---

## 2. Crash Failures

### 2.1 Single Validator Crash

| Property | Value |
|----------|-------|
| **Type** | Crash |
| **Severity** | S2 (if f < n/3 still holds) or S1 (if quorum lost) |
| **Description** | A validator process terminates unexpectedly (OOM, SIGKILL, hardware failure). |
| **Impact on Safety** | None. Crash failures cannot cause safety violations. |
| **Impact on Liveness** | If remaining honest voting power > 2n/3: no impact. If ≤ 2n/3: liveness lost until recovery. |
| **Detection** | Peer disconnection, health check failure, missing votes. |
| **Recovery** | Restart node. WAL recovery restores in-progress consensus state. Node catches up via block sync. |

### 2.2 Multiple Validator Crashes

| Property | Value |
|----------|-------|
| **Type** | Crash |
| **Severity** | S1 |
| **Description** | Multiple validators crash simultaneously (e.g., correlated failure from shared infrastructure). |
| **Impact on Safety** | None. |
| **Impact on Liveness** | Lost if crashed voting power ≥ n/3. |
| **Detection** | Height stops advancing. Multiple peers disconnected. |
| **Recovery** | Restart crashed nodes. System resumes when >2/3 voting power is online. No data loss if WAL is intact. |

### 2.3 Crash During Block Execution

| Property | Value |
|----------|-------|
| **Type** | Crash |
| **Severity** | S2 |
| **Description** | Node crashes after CommitStarted WAL entry but before CommitCompleted. |
| **Impact** | Block may be partially executed. State may be inconsistent. |
| **Recovery** | On restart, WAL recovery detects CommitStarted without CommitCompleted. Re-executes the block from scratch. Atomic storage writes ensure no partial state is visible. |

### 2.4 Crash During WAL Write

| Property | Value |
|----------|-------|
| **Type** | Crash |
| **Severity** | S2 |
| **Description** | Node crashes mid-write to WAL file. |
| **Impact** | Last WAL entry may be truncated. |
| **Recovery** | On restart, CRC32 validation detects the truncated entry. It is discarded. Node recovers from the last valid WAL entry. Worst case: node loses knowledge of a vote it sent (but the vote was already broadcast to peers, so no safety issue). |

---

## 3. Byzantine Failures

### 3.1 Equivocation (Double Voting)

| Property | Value |
|----------|-------|
| **Type** | Byzantine |
| **Severity** | S1 |
| **Description** | A validator sends two different votes for the same (height, round, step). |
| **Impact on Safety** | None, provided f < n/3. The locking mechanism prevents conflicting commits. |
| **Impact on Liveness** | Minimal. May cause some nodes to see different polkas, leading to extra rounds. |
| **Detection** | Any node receiving both conflicting votes generates an Evidence record. |
| **Response** | Evidence is gossiped, included in blocks, and logged. MVP: logged only. Future: slashing. |

### 3.2 Invalid Proposal

| Property | Value |
|----------|-------|
| **Type** | Byzantine |
| **Severity** | S3 |
| **Description** | Proposer sends a block with invalid transactions, wrong parent hash, or invalid structure. |
| **Impact on Safety** | None. Honest validators validate the proposal and prevote nil if invalid. |
| **Impact on Liveness** | One wasted round. System advances to next round with a different proposer. |
| **Detection** | Proposal validation fails. Logged as warning. |
| **Response** | Prevote nil. Advance round. Penalize peer score. |

### 3.3 Selective Message Delivery

| Property | Value |
|----------|-------|
| **Type** | Byzantine |
| **Severity** | S2 |
| **Description** | Byzantine proposer sends the proposal to only a subset of validators. |
| **Impact on Safety** | None. Validators who don't receive the proposal prevote nil. |
| **Impact on Liveness** | May prevent commit in the current round if the subset is too small for a polka. |
| **Detection** | Some validators timeout on propose while others receive the proposal. Visible in logs. |
| **Response** | Timeout → prevote nil → advance round. Self-resolving. |

### 3.4 Conflicting Proposals

| Property | Value |
|----------|-------|
| **Type** | Byzantine |
| **Severity** | S1 |
| **Description** | Byzantine proposer sends different blocks to different validators. |
| **Impact on Safety** | None, provided f < n/3. No block can get >2/3 prevotes if honest validators see different proposals. |
| **Impact on Liveness** | Wasted round. May cause locking on different blocks, requiring additional rounds to resolve. |
| **Detection** | Equivocation evidence when two proposals for the same (height, round) with different block hashes are observed. |
| **Response** | Record evidence. Advance round. |

### 3.5 Vote Withholding

| Property | Value |
|----------|-------|
| **Type** | Byzantine |
| **Severity** | S2 |
| **Description** | Byzantine validator receives messages but does not send votes. |
| **Impact on Safety** | None. |
| **Impact on Liveness** | Reduces effective voting power. If remaining honest power > 2n/3, no impact. |
| **Detection** | Missing votes from a validator across multiple rounds. Observable in metrics. |
| **Response** | System proceeds without the withholding validator's votes. |

### 3.6 Message Replay

| Property | Value |
|----------|-------|
| **Type** | Byzantine / Network |
| **Severity** | S3 |
| **Description** | Old messages are replayed (by attacker or network artifact). |
| **Impact on Safety** | None. Votes are deduplicated by (height, round, step, validator). |
| **Impact on Liveness** | None. Duplicate messages are dropped. |
| **Detection** | Deduplication logic in VoteSet. |
| **Response** | Drop duplicate. No action needed. |

### 3.7 Threshold Byzantine Failure (f ≥ n/3)

| Property | Value |
|----------|-------|
| **Type** | Byzantine |
| **Severity** | S0 |
| **Description** | Byzantine voting power reaches or exceeds n/3. |
| **Impact on Safety** | **SAFETY MAY BE VIOLATED.** The protocol's safety guarantee requires f < n/3. |
| **Impact on Liveness** | Liveness may be lost. |
| **Detection** | Difficult to detect in real-time. Post-hoc analysis of equivocation evidence. |
| **Response** | **Outside the protocol's guarantees.** Requires human intervention, social consensus, and potentially a hard fork. This is a fundamental limitation of all BFT protocols. |

---

## 4. Network Failures

### 4.1 Full Network Partition

| Property | Value |
|----------|-------|
| **Type** | Network |
| **Severity** | S1 |
| **Description** | Network splits into two or more groups that cannot communicate. |
| **Impact on Safety** | None. Neither partition can achieve >2/3 quorum (assuming honest majority). |
| **Impact on Liveness** | Lost. No partition can commit. |
| **Detection** | Height stops advancing. Peer count drops. |
| **Recovery** | Automatic when partition heals. Nodes exchange missing votes and converge. |

### 4.2 Partial Network Partition

| Property | Value |
|----------|-------|
| **Type** | Network |
| **Severity** | S2 |
| **Description** | Some pairs of nodes can communicate, others cannot. |
| **Impact on Safety** | None. |
| **Impact on Liveness** | May be impacted if the partition prevents >2/3 from seeing the same proposal/votes. Gossip protocol mitigates this (votes are forwarded through connected paths). |
| **Detection** | Increased round counts. Some nodes timeout while others don't. |
| **Recovery** | Often self-resolving via gossip. If persistent, may require network fix. |

### 4.3 Message Loss

| Property | Value |
|----------|-------|
| **Type** | Network |
| **Severity** | S3 |
| **Description** | Individual messages are dropped (unreliable network). |
| **Impact on Safety** | None. |
| **Impact on Liveness** | May delay commits. Gossip and timeouts compensate. |
| **Detection** | Increased round counts. Timeout metrics increase. |
| **Recovery** | Automatic. Gossip re-delivers votes. Timeouts trigger round advancement. |

### 4.4 Message Delay

| Property | Value |
|----------|-------|
| **Type** | Network |
| **Severity** | S3 |
| **Description** | Messages arrive but with significant delay. |
| **Impact on Safety** | None. |
| **Impact on Liveness** | May cause unnecessary timeouts and extra rounds. |
| **Detection** | Higher commit latency. Timeout metrics. |
| **Recovery** | Increase timeout values to accommodate higher latency. |

### 4.5 Message Reordering

| Property | Value |
|----------|-------|
| **Type** | Network |
| **Severity** | S3 |
| **Description** | Messages arrive out of order. |
| **Impact on Safety** | None. Consensus protocol is order-independent for safety. |
| **Impact on Liveness** | Minimal. Votes are accumulated regardless of arrival order. |
| **Detection** | Not typically detectable or problematic. |
| **Recovery** | No action needed. |

---

## 5. Storage Failures

### 5.1 Disk Full

| Property | Value |
|----------|-------|
| **Type** | Storage |
| **Severity** | S1 |
| **Description** | No space remaining on the data partition. |
| **Impact** | Node cannot persist blocks or WAL entries. Node halts. |
| **Detection** | Storage write returns error. Node logs ERROR and halts. |
| **Recovery** | Free disk space (delete old logs, increase volume). Restart node. |

### 5.2 RocksDB Corruption

| Property | Value |
|----------|-------|
| **Type** | Storage |
| **Severity** | S1 |
| **Description** | RocksDB detects internal corruption (bad SST file, corrupted manifest). |
| **Impact** | Node cannot read or write state. Node halts. |
| **Detection** | RocksDB returns corruption error. |
| **Recovery** | Attempt `rustbft-node repair-db`. If repair fails: restore from backup or state reset + re-sync. |

### 5.3 WAL Corruption

| Property | Value |
|----------|-------|
| **Type** | Storage |
| **Severity** | S2 |
| **Description** | WAL file is corrupted (bit flip, truncated write). |
| **Impact** | Node may lose knowledge of in-progress votes. |
| **Detection** | CRC32 mismatch during WAL recovery. |
| **Recovery** | Corrupted entries are discarded. Node recovers from last valid entry. In worst case, node may re-vote differently (but this is safe because the original vote was already broadcast). |

### 5.4 Slow Disk I/O

| Property | Value |
|----------|-------|
| **Type** | Storage |
| **Severity** | S2 |
| **Description** | Disk I/O latency is abnormally high (degraded SSD, noisy neighbor). |
| **Impact** | WAL writes slow down. Block persistence slows. Channel backpressure may slow consensus. |
| **Detection** | `rustbft_storage_wal_write_duration_seconds` and `rustbft_storage_block_persist_duration_seconds` increase. |
| **Recovery** | Investigate disk health. Move to faster storage. |

---

## 6. Execution Failures

### 6.1 Contract Panic / Trap

| Property | Value |
|----------|-------|
| **Type** | Execution |
| **Severity** | S3 |
| **Description** | A contract hits an unreachable instruction, out-of-bounds access, or explicit abort. |
| **Impact** | Transaction fails. Contract state is reverted. Gas is charged. |
| **Detection** | Transaction receipt shows `success: false`. |
| **Recovery** | No recovery needed. This is expected behavior. The block is still valid. |

### 6.2 Gas Exhaustion

| Property | Value |
|----------|-------|
| **Type** | Execution |
| **Severity** | S3 |
| **Description** | A transaction runs out of gas during execution. |
| **Impact** | Transaction fails. State reverted. Full gas charged. |
| **Detection** | Receipt shows `error: "out of gas"`. |
| **Recovery** | User should retry with higher gas limit. |

### 6.3 Determinism Bug

| Property | Value |
|----------|-------|
| **Type** | Execution |
| **Severity** | S0 |
| **Description** | Two honest nodes execute the same block and produce different state roots. |
| **Impact** | **CRITICAL.** Nodes diverge. Affected node halts. |
| **Detection** | State root mismatch detected during block verification. |
| **Recovery** | See `docs/runbooks/debugging-consensus.md` §5. Requires code fix, then state reset. |

---

## 7. Operational Failures

### 7.1 Key Loss

| Property | Value |
|----------|-------|
| **Type** | Operational |
| **Severity** | S1 |
| **Description** | Validator's private key is lost or destroyed. |
| **Impact** | Validator cannot sign votes. Effectively a permanent crash. |
| **Recovery** | Remove the validator via ValidatorUpdate. Add a new validator with a new key. |

### 7.2 Key Compromise

| Property | Value |
|----------|-------|
| **Type** | Operational |
| **Severity** | S0 |
| **Description** | Validator's private key is obtained by an attacker. |
| **Impact** | Attacker can equivocate, potentially contributing to f ≥ n/3. |
| **Recovery** | Immediately remove the compromised validator. Rotate keys. Audit for equivocation evidence. |

### 7.3 Wrong Genesis File

| Property | Value |
|----------|-------|
| **Type** | Operational |
| **Severity** | S1 |
| **Description** | A node starts with a different genesis file than other nodes. |
| **Impact** | Node will have a different chain_id or initial state. Cannot connect to peers (chain_id mismatch) or will diverge immediately. |
| **Detection** | Handshake failure (chain_id mismatch). Or state root mismatch at height 1. |
| **Recovery** | Stop node. Replace genesis file. State reset. Restart. |

### 7.4 Version Mismatch

| Property | Value |
|----------|-------|
| **Type** | Operational |
| **Severity** | S1 |
| **Description** | Nodes run different binary versions with incompatible protocol changes. |
| **Impact** | Message parsing failures, state divergence, or consensus stalls. |
| **Detection** | Protocol version check during handshake. Deserialization errors in logs. |
| **Recovery** | Ensure all nodes run the same version. Perform rolling upgrade. |

### 7.5 Clock Skew

| Property | Value |
|----------|-------|
| **Type** | Operational |
| **Severity** | S3 |
| **Description** | Node's system clock is significantly different from other nodes. |
| **Impact** | Block timestamps may be rejected by other nodes (if timestamp validation is strict). Timeout behavior may differ. |
| **Detection** | Block timestamp warnings in logs. |
| **Recovery** | Synchronize clocks via NTP. RustBFT uses timestamps as advisory only, so moderate skew is tolerable. |

---

## 8. Failure Interaction Matrix

This matrix shows what happens when multiple failures occur simultaneously:

| | Crash (1 node) | Byzantine (1 node) | Partition |
|---|---|---|---|
| **Crash (1 node)** | 2 crashed: liveness lost if ≥ n/3 | 1 crash + 1 byz: safe if total < n/3 | 1 crash + partition: depends on partition sizes |
| **Byzantine (1 node)** | — | 2 byzantine: safe if total < n/3 | 1 byz + partition: safe, liveness depends on partition |
| **Partition** | — | — | Multiple partitions: liveness lost, safety preserved |

**Key insight:** Safety is preserved as long as total Byzantine voting power < n/3. Liveness requires >2/3 honest voting power to be connected.

---

## 9. Failure Budget

For a 4-validator cluster with equal voting power (100 each):

```
Total voting power: 400
Quorum threshold:   (400 * 2 / 3) + 1 = 268
Maximum faulty:     400 / 3 = 133 (i.e., 1 validator with power 100)

Failure budget:
    - 1 crash:     OK (300/400 > 2/3) — both safe and live
    - 1 byzantine: OK (f=100 < 133)   — safe
    - 2 crashes:   LIVENESS LOST (200/400 < 2/3) — still safe
    - 1 crash + 1 byzantine: RISKY (f=100, but only 200 honest online)
                              — safe (f < n/3) but liveness lost
    - 2 byzantine: UNSAFE (f=200 ≥ 133) — safety guarantee void
```

---

## 10. Summary Table

| Failure | Type | Severity | Safety Impact | Liveness Impact | Auto-Recovery |
|---------|------|----------|---------------|-----------------|---------------|
| Single crash | Crash | S2 | None | None (if >2/3 remain) | Yes (restart + WAL) |
| Multiple crashes | Crash | S1 | None | Lost if ≥ n/3 crash | Yes (restart all) |
| Crash during execution | Crash | S2 | None | Brief stall | Yes (WAL re-execute) |
| Equivocation | Byzantine | S1 | None (if f<n/3) | Minimal | Evidence recorded |
| Invalid proposal | Byzantine | S3 | None | 1 wasted round | Yes (next round) |
| Selective delivery | Byzantine | S2 | None | May waste round | Yes (timeout) |
| f ≥ n/3 byzantine | Byzantine | S0 | **POSSIBLE** | **POSSIBLE** | **NO** |
| Full partition | Network | S1 | None | Lost | Yes (on heal) |
| Partial partition | Network | S2 | None | May degrade | Yes (gossip) |
| Message loss/delay | Network | S3 | None | Minor delay | Yes (gossip + timeout) |
| Disk full | Storage | S1 | None | Node halts | No (operator) |
| DB corruption | Storage | S1 | None | Node halts | Partial (repair) |
| WAL corruption | Storage | S2 | None | Brief | Yes (discard bad entry) |
| Determinism bug | Execution | S0 | **YES** | Node halts | **NO** (code fix) |
| Contract panic | Execution | S3 | None | None | Yes (tx reverted) |
| Key compromise | Operational | S0 | **POSSIBLE** | None | No (remove validator) |

---

## Definition of Done — Failure Models

- [x] Failure taxonomy defined (crash, byzantine, network, storage, execution, operational)
- [x] Severity levels defined with system response
- [x] Every failure mode documented with: type, severity, description, impact, detection, recovery
- [x] Crash failures (single, multiple, during execution, during WAL write)
- [x] Byzantine failures (equivocation, invalid proposal, selective delivery, conflicting proposals, withholding, replay, threshold)
- [x] Network failures (full partition, partial partition, message loss, delay, reordering)
- [x] Storage failures (disk full, corruption, WAL corruption, slow I/O)
- [x] Execution failures (contract panic, gas exhaustion, determinism bug)
- [x] Operational failures (key loss, key compromise, wrong genesis, version mismatch, clock skew)
- [x] Failure interaction matrix
- [x] Failure budget for 4-validator cluster
- [x] Summary table
- [x] No Rust source code included
