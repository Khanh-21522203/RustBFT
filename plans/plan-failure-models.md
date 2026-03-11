# Feature: Failure Models & Recovery

## 1. Purpose

The Failure Models module defines how RustBFT detects, classifies, and responds to every category of failure: crash failures, Byzantine behavior, network partitions, storage errors, execution bugs, and operational mistakes. It is not a single Rust module but a cross-cutting set of detection mechanisms, error paths, and recovery procedures that must be implemented correctly across all modules. Getting it wrong can turn a recoverable S2 failure into an S0 safety violation.

## 2. Responsibilities

- Define four severity levels (S0–S3) and the system response for each
- Implement crash recovery via WAL: restore `locked_block`, `locked_round`, `round`, and `height` on restart without re-sending votes
- Detect equivocation (two votes from the same validator for the same `(height, round, type)`) and record `EquivocationEvidence`
- Halt the node immediately on S0 failures: state root mismatch, f ≥ n/3 Byzantine voting power proven, storage corruption
- Detect and handle S1 failures: disk full, network partition, multiple validator crashes, quorum loss
- Detect S2 failures: single crash, WAL truncation, slow disk, partial network partition
- Detect S3 failures: message loss, message delay, message reorder, contract panic, gas exhaustion
- Provide `rustbft-node repair-db` subcommand for RocksDB repair after storage corruption
- Log structured events for every failure detection with severity, type, and recovery action taken

## 3. Non-Responsibilities

- Does not implement slashing or on-chain punishment for Byzantine validators (future governance feature)
- Does not perform network-level filtering or firewall rules
- Does not automatically hard-fork after f ≥ n/3 Byzantine failure (requires human intervention)
- Does not provide automatic backup or restore — those are operator procedures
- Does not implement block sync for catching up after long outage (future feature)

## 4. Architecture Design

```
Failure Detection Points:

Consensus Core:
  ├── Equivocation: VoteSet::add_vote() detects two different votes → RecordEvidence command
  ├── State root mismatch: BlockExecuted result vs block header → HALT
  └── Stale/invalid events: is_relevant() filter → drop (S3)

P2P Layer:
  ├── Invalid signature: verify before forwarding → penalize peer (S3)
  ├── Oversized message: framing check → disconnect (S3)
  └── Handshake chain_id mismatch → reject (S3)

Storage Thread:
  ├── Write error (disk full, I/O error) → return Err → HALT (S1)
  ├── WAL CRC32 mismatch on recovery → discard entry, recover from last valid
  └── Startup integrity check (hash mismatch) → HALT (S0)

State Machine:
  ├── Contract panic/trap → revert tx, charge gas (S3)
  ├── Gas exhaustion → revert tx, charge full gas (S3)
  └── Determinism violation → state root mismatch → HALT (S0)

Node Binary:
  ├── Signal handler → initiate graceful shutdown
  └── Thread panic → log ERROR, initiate shutdown
```

## 5. Core Data Structures (Rust)

```rust
// src/node/failure.rs

pub enum FailureType {
    Crash,
    Byzantine,
    Network,
    Storage,
    Execution,
    Operational,
}

pub enum Severity {
    S0Critical,    // Safety violation possible; HALT immediately
    S1Major,       // Liveness impacted; alert operator, attempt recovery
    S2Minor,       // Single node degraded; auto-recover, log warning
    S3Info,        // Expected transient condition; log and continue
}

pub struct FailureEvent {
    pub failure_type: FailureType,
    pub severity: Severity,
    pub description: String,
    pub recovery_action: RecoveryAction,
}

pub enum RecoveryAction {
    Halt { reason: String },
    Restart,
    DropEvent,
    RevertTransaction,
    PenalizePeer { node_id: ValidatorId, delta: i32 },
    RecordEvidence { evidence: EquivocationEvidence },
    Continue,
}

// Equivocation evidence (also in consensus types)
pub struct EquivocationEvidence {
    pub vote_a: SignedVote,
    pub vote_b: SignedVote,
}
```

## 6. Public Interfaces

```rust
// src/node/failure.rs

/// Called by any module to trigger a node halt.
/// Logs the reason at ERROR level and exits the process.
pub fn halt(reason: &str) -> ! {
    tracing::error!(reason = reason, "NODE HALTING");
    std::process::exit(1);
}

/// Called by the node binary's panic handler.
pub fn install_panic_hook();

// WAL recovery result (also defined in storage plan)
pub struct WALRecoveryResult {
    pub height: u64,
    pub round: u32,
    pub locked_block: Option<Hash>,
    pub locked_round: Option<u32>,
    pub commit_needs_re_execution: bool,
}

// RocksDB repair — CLI subcommand
pub fn repair_db(data_dir: &Path) -> Result<(), RepairError>;

// Failure budget helper (for tests and monitoring)
pub fn quorum_threshold(total_power: u64) -> u64 {
    (total_power * 2 / 3) + 1
}
pub fn max_byzantine_power(total_power: u64) -> u64 {
    (total_power - 1) / 3
}
```

## 7. Internal Algorithms

### S0 Halt: State Root Mismatch
```
// Called in consensus core after BlockExecuted event
fn on_block_executed(result: BlockExecutionResult, expected_header: &BlockHeader):
    if result.state_root != expected_header.state_root:
        log ERROR "STATE ROOT MISMATCH — HALTING"
            { height: result.height,
              expected: expected_header.state_root,
              computed: result.state_root }
        halt("state root mismatch")
```

### S2 Recovery: WAL Truncated Write
```
fn recover_wal_entry(file):
    crc_read = read_u32_be(file)?  else return None  // EOF
    len_read = read_u32_be(file)?  else return None
    payload  = read_exact(file, len_read)? else:
        log WARN "WAL entry truncated — discarding"
        return None  // truncated write — stop reading
    if crc32(len_be(len_read) + payload) != crc_read:
        log WARN "WAL CRC32 mismatch — discarding rest of WAL"
        return None
    return Some(canonical_decode(payload))
```

### S1 Recovery: Crash During Block Execution
```
fn handle_wal_recovery(entries: Vec<WALEntry>):
    if entries contains CommitCompleted:
        truncate WAL
        return WALRecoveryResult { height: committed_height, round: 0, ... }

    if entries contains CommitStarted but no CommitCompleted:
        H = CommitStarted.height
        block = storage.get_block_by_height(H)?
        if block is None:
            // Crash before block was stored; start from H-1
            return WALRecoveryResult { height: H-1, round: 0, ... }
        else:
            // Re-execute the block
            return WALRecoveryResult {
                height: H-1,
                round: 0,
                commit_needs_re_execution: true,
                ...
            }

    // Restore lock state from Locked/Unlocked entries
    locked_block = last Locked entry's block_hash (if no Unlocked after it)
    locked_round = last Locked entry's round
    round = last RoundStarted entry's round
    return WALRecoveryResult { height: ..., round, locked_block, locked_round, ... }
```

### S1 Detection: Equivocation
```
// In VoteSet::add_vote()
fn add_vote(vote: SignedVote, voting_power: u64) -> VoteAddResult:
    key = (vote.round, vote.vote_type, vote.validator_id)
    if let Some(existing) = votes.get(&key):
        if existing.block_hash != vote.block_hash:
            // Two different votes from same validator — equivocation!
            evidence = EquivocationEvidence { vote_a: existing.clone(), vote_b: vote }
            self.evidence.push(evidence)
            log ERROR "Equivocation detected"
                { validator: vote.validator_id, height: vote.height }
            return VoteAddResult::Equivocation
    votes.insert(key, vote)
    power_for[(vote.round, vote.vote_type, vote.block_hash)] += voting_power
    VoteAddResult::Added
```

### S2 Detection: Startup Integrity Check
```
fn check_startup_integrity(storage):
    H = storage.get_latest_height()
    if H == 0: return  // fresh node

    block = storage.get_block_by_height(H)
        .unwrap_or_else(|| halt("missing block at latest height {H}"))
    computed_hash = sha256(canonical_encode(&block))
    if computed_hash != block.header.hash:
        halt("block data corrupted at height {H}")

    stored_root = storage.get_state_root(H)
        .unwrap_or_else(|| halt("missing state root at height {H}"))
    if stored_root != block.header.state_root:
        halt("state root mismatch at height {H}: expected={stored_root}, block={block.header.state_root}")
```

### Failure Budget (4-validator cluster)
```
total_power = 400
quorum = (400 * 2 / 3) + 1 = 268
max_byzantine = (400 - 1) / 3 = 133

1 crash (power=100):      safe=YES, live=YES (300 ≥ 268)
1 byzantine (power=100):  safe=YES (100 < 133)
2 crashes (power=200):    safe=YES, live=NO (200 < 268)
2 byzantine (power=200):  safe=NO (200 > 133) — S0 CRITICAL
```

## 8. Persistence Model

Failure detection state is ephemeral. Equivocation evidence is emitted as `RecordEvidence` commands and included in future block proposals (gossipped to peers). Evidence is stored in the block store alongside the block that included it. The WAL is the primary recovery artifact for crash failures. No separate failure log is maintained.

## 9. Concurrency Model

Each module detects its own failures synchronously on its own thread:
- Consensus core: equivocation detection on the consensus thread
- Storage thread: I/O error detection on the storage thread
- Node binary's panic hook: installs a `std::panic::set_hook` that logs and exits
- Halt is implemented as `std::process::exit(1)` — no cleanup attempted for S0 failures (WAL ensures recoverability)

## 10. Configuration

```toml
[node]
# No separate failure model config.
# Severity S0 always halts — cannot be configured away.

[consensus]
# Equivocation evidence is always recorded

[storage]
# Corruption detection always runs at startup
# repair-db is a CLI subcommand, not auto-triggered

[p2p]
peer_ban_duration_ms = 300000  # Duration to ban a peer after score hits 0
```

## 11. Observability

- `rustbft_consensus_equivocations_total` (Counter) — equivocations detected
- `rustbft_storage_wal_crc_errors_total` (Counter) — WAL CRC32 errors on recovery
- `rustbft_node_restarts_total` (Counter) — node restart count (useful if running under systemd/Docker restart policy)
- Log ERROR `"NODE HALTING"` with `{reason}` on any S0 failure
- Log ERROR `"STATE ROOT MISMATCH"` with `{height, expected, computed}`
- Log ERROR `"Equivocation detected"` with `{validator, height, round, vote_a_hash, vote_b_hash}`
- Log WARN `"WAL CRC32 mismatch — discarding rest of WAL"` on S2 recovery
- Log WARN `"Startup integrity check: block hash mismatch"` → then HALT
- Log INFO `"WAL recovery complete"` with `{height, round, locked_block, entries_replayed}`

## 12. Testing Strategy

- **`test_halt_on_state_root_mismatch`**: inject BlockExecuted with wrong state_root → process calls `halt()` (mock panic or process exit in test)
- **`test_equivocation_detected_in_vote_set`**: add two different prevotes for same validator/round → `VoteAddResult::Equivocation`, evidence in `take_evidence()`
- **`test_equivocation_counted_once`**: after equivocation, voting power is only counted for the first vote
- **`test_wal_recovery_truncated_entry`**: write partial WAL entry, recover → only complete entries returned, no panic
- **`test_wal_recovery_crc_mismatch`**: corrupt a WAL entry's CRC, recover → entry discarded, prior entries restored
- **`test_wal_recovery_commit_started_no_completed`**: WAL has CommitStarted but no CommitCompleted → `commit_needs_re_execution = true`
- **`test_wal_recovery_commit_completed`**: WAL has CommitCompleted → clean startup result
- **`test_startup_integrity_check_clean`**: well-formed DB → check passes
- **`test_startup_integrity_check_bad_block_hash`**: corrupted block bytes → halt triggered
- **`test_startup_integrity_check_state_root_mismatch`**: tampered state root → halt triggered
- **`test_failure_budget_4_validators`**: compute quorum and max_byzantine for total_power=400 → 268 and 133
- **`test_s3_contract_panic_reverts`**: contract hits unreachable → tx reverted, gas charged, block valid
- **`test_s3_gas_exhaustion_reverts`**: tx exceeds gas limit → state reverted, full gas charged
- **`test_s1_disk_full_halts_storage`**: mock storage engine returns disk-full error → storage thread sends halt signal
- **`test_repair_db_runs_without_error`**: call `repair_db` on a valid test database → returns Ok(())

## 13. Open Questions

- **Slashing for equivocation**: The MVP records equivocation evidence but does not punish the offending validator (no on-chain slashing). Future governance work can consume the evidence records to trigger stake slashing.
- **f ≥ n/3 detection**: Real-time detection that Byzantine power has crossed the safety threshold is not possible without out-of-band trust. Post-hoc detection via equivocation evidence is the best available mechanism.
