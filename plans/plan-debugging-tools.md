# Feature: Debugging Tools

## 1. Purpose

The Debugging Tools module provides the `rustbft-node` subcommands and runtime flags that operators and engineers use to diagnose consensus failures, inspect internal state, and reproduce bugs deterministically. The tools are designed around the three most common failure categories: height-not-advancing (consensus stall), state root mismatch (determinism bug), and storage corruption. They require no running node — they operate on files on disk.

## 2. Responsibilities

- Implement `rustbft-node wal-inspect --data-dir <dir>`: parse and pretty-print all WAL entries in a WAL file, showing exactly where the node was when it crashed
- Implement `rustbft-node state-dump --data-dir <dir> --height <H>`: dump all accounts and contract storage at a given height to JSON for comparison between nodes
- Implement `rustbft-node repair-db --data-dir <dir>`: invoke RocksDB's built-in repair facility on a corrupted database
- Implement `rustbft-node replay --events <file> [--verbose]`: replay a recorded sequence of `ConsensusEvent`s into a fresh `ConsensusState` and print every state transition
- Support `RUSTBFT_RECORD_EVENTS=true` environment flag: write all `ConsensusEvent`s received by the consensus core to `/data/debug/events.log` for later replay
- Provide `scripts/collect-debug-bundle.sh` shell script that collects logs, WAL state, metrics, and node status into a tarball for bug reports

## 3. Non-Responsibilities

- Does not implement a full blockchain explorer or block browser — use `rustbft-cli query block` for that
- Does not repair WAL files (corrupted WAL entries are discarded, not repaired)
- Does not provide real-time consensus introspection — only offline analysis
- Does not implement remote debugging or attach to a running process
- Does not provide profiling or heap analysis tooling

## 4. Architecture Design

```
$ rustbft-node wal-inspect --data-dir /data

  WAL file: /data/wal/consensus.wal
  ┌───────────────────────────────────────────────────────────┐
  │  Entry 0: RoundStarted     height=42 round=0              │
  │  Entry 1: ProposalSent     height=42 round=0 hash=abc123  │
  │  Entry 2: VoteSent         height=42 round=0 type=Prevote │
  │  Entry 3: Locked           height=42 round=0 hash=abc123  │
  │  Entry 4: VoteSent         height=42 round=0 type=Precommit│
  │  Entry 5: CommitStarted    height=42 hash=abc123          │
  │  [no CommitCompleted — crash during block execution]       │
  └───────────────────────────────────────────────────────────┘

$ rustbft-node state-dump --data-dir /data --height 41
  { "height": 41, "state_root": "...", "accounts": {...}, "contract_storage": {...} }

$ rustbft-node replay --events /data/debug/events.log --verbose
  [height=42 round=0] event=EnterRound → actions=[ScheduleTimeout]
  [height=42 round=0] event=ProposalReceived → actions=[BroadcastVote(Prevote)]
  ...

$ rustbft-node repair-db --data-dir /data
  Running RocksDB repair on /data ...
  Repair completed. Start the node to verify.
```

## 5. Core Data Structures (Rust)

```rust
// src/tools/wal_inspect.rs

pub struct WALInspectResult {
    pub entries: Vec<WALInspectEntry>,
    pub crc_errors: usize,
    pub diagnosis: WALDiagnosis,
}

pub struct WALInspectEntry {
    pub index: usize,
    pub entry: WALEntry,
    pub byte_offset: u64,
}

pub enum WALDiagnosis {
    Clean,                         // CommitCompleted present — normal recovery
    CrashedDuringExecution { height: u64, block_hash: Hash },  // CommitStarted, no Completed
    CrashedMidRound { height: u64, round: u32, last_action: String },
    Corrupt { last_valid_index: usize },
}

// src/tools/state_dump.rs

pub struct StateDump {
    pub height: u64,
    pub state_root: Hash,
    pub accounts: BTreeMap<String, AccountDump>,       // address → account
    pub contract_storage: BTreeMap<String, BTreeMap<String, String>>,  // addr → key → value
}

pub struct AccountDump {
    pub balance: String,    // decimal string (u128 may exceed JSON f64)
    pub nonce: u64,
    pub code_hash: Option<String>,
}

// src/tools/event_replay.rs

pub struct EventReplayResult {
    pub final_height: u64,
    pub final_round: u32,
    pub final_step: String,
    pub commands_emitted: Vec<ConsensusCommand>,
    pub state_transitions: Vec<StateTransitionLog>,
}

pub struct StateTransitionLog {
    pub event: ConsensusEvent,
    pub commands: Vec<ConsensusCommand>,
    pub state_after: ConsensusStateSummary,
}

pub struct ConsensusStateSummary {
    pub height: u64,
    pub round: u32,
    pub step: String,
    pub locked_block: Option<String>,
    pub valid_block: Option<String>,
}

// Event recording format (written to /data/debug/events.log)
pub struct EventLogEntry {
    pub timestamp_ns: u64,
    pub node_id: String,
    pub event: ConsensusEvent,
}
```

## 6. Public Interfaces

```rust
// src/main.rs — CLI subcommands added to the existing command tree

#[derive(Subcommand)]
pub enum Command {
    Start { /* existing */ },
    Keygen { /* existing */ },

    WalInspect {
        #[arg(long)] data_dir: PathBuf,
        #[arg(long, default_value = "false")] json: bool,
    },
    StateDump {
        #[arg(long)] data_dir: PathBuf,
        #[arg(long)] height: u64,
        #[arg(long, default_value = "false")] json: bool,
    },
    RepairDb {
        #[arg(long)] data_dir: PathBuf,
    },
    Replay {
        #[arg(long)] events: PathBuf,
        #[arg(long, default_value = "false")] verbose: bool,
        #[arg(long)] genesis: PathBuf,
    },
}

// src/tools/wal_inspect.rs
pub fn wal_inspect(data_dir: &Path) -> WALInspectResult;
pub fn print_wal_inspect_result(result: &WALInspectResult, json: bool);

// src/tools/state_dump.rs
pub fn state_dump(data_dir: &Path, height: u64) -> Result<StateDump, DumpError>;
pub fn print_state_dump(dump: &StateDump, json: bool);

// src/tools/repair_db.rs
pub fn repair_db(data_dir: &Path) -> Result<(), RepairError>;

// src/tools/event_replay.rs
pub fn replay_events(
    events_path: &Path,
    genesis: &GenesisConfig,
    verbose: bool,
) -> EventReplayResult;

// Event recording — called from the consensus core event loop when enabled
pub fn maybe_record_event(recorder: &Option<EventRecorder>, event: &ConsensusEvent);
pub fn open_event_recorder(data_dir: &Path) -> Option<EventRecorder>;  // None if not enabled
```

## 7. Internal Algorithms

### WAL Inspect
```
fn wal_inspect(data_dir):
    wal_path = data_dir / "wal" / "consensus.wal"
    file = open(wal_path)?
    entries = []
    crc_errors = 0
    byte_offset = 0

    loop:
        entry_start = byte_offset
        crc_read = read_u32_be(file)? else break
        len_read  = read_u32_be(file)? else break
        payload   = read_exact(file, len_read)? else break
        pad       = read_exact(file, align_pad(len_read))? else break

        if crc32(len_be(len_read) + payload) != crc_read:
            crc_errors += 1
            break  // stop at first CRC error

        entry = canonical_decode(payload)
        entries.push(WALInspectEntry { index: entries.len(), entry, byte_offset: entry_start })
        byte_offset += 4 + 4 + len_read + align_pad(len_read)

    diagnosis = diagnose(entries)
    WALInspectResult { entries, crc_errors, diagnosis }

fn diagnose(entries):
    if entries is empty: return Clean
    last = entries.last()
    match last.entry:
        CommitCompleted { .. } => Clean
        CommitStarted { height, block_hash } => CrashedDuringExecution { height, block_hash }
        _ =>
            if crc_errors > 0: Corrupt { last_valid_index: entries.len() - 1 }
            else: CrashedMidRound { height: ..., round: ..., last_action: last.entry.describe() }
```

### State Dump
```
fn state_dump(data_dir, height):
    db = open_rocksdb_read_only(data_dir / "rocksdb")?
    state_root = get "state/root/{height}" else Err(HeightNotFound)
    accounts = {}
    for (key, value) in db.iterator("state", "account/"):
        addr = extract_address_from_key(key)
        account = canonical_decode(value)
        accounts[hex(addr)] = AccountDump { balance: account.balance.to_string(), ... }
    // ... similar for contract_storage
    StateDump { height, state_root, accounts, contract_storage }
```

### Event Replay
```
fn replay_events(events_path, genesis, verbose):
    entries = read_jsonlines(events_path)?
    validator_set = ValidatorSet::from_genesis(genesis)
    mut state = ConsensusState::new_at_height_1(validator_set, node_id)
    let (inbound_tx, inbound_rx) = crossbeam::channel::bounded(1024)
    let (outbound_tx, outbound_rx) = crossbeam::channel::bounded(1024)

    // Start consensus core thread
    thread = spawn_consensus_thread(inbound_rx, outbound_tx, state)

    transitions = []
    for entry in entries where entry.node_id == target_node:
        inbound_tx.send(entry.event)
        // Collect all commands emitted
        commands = drain_outbound_rx(outbound_rx)
        if verbose:
            print transition { event, commands, state_summary }
        transitions.push(StateTransitionLog { event, commands, ... })

    drop(inbound_tx)  // close channel → thread exits
    thread.join()
    EventReplayResult { ... transitions }
```

### Event Recording
```
// Enabled by RUSTBFT_RECORD_EVENTS=true
fn maybe_record_event(recorder, event):
    if let Some(rec) = recorder:
        entry = EventLogEntry {
            timestamp_ns: unix_timestamp_ns(),
            node_id: rec.node_id.clone(),
            event: event.clone(),
        }
        // Append as JSON line to events.log
        writeln!(rec.file, "{}", serde_json::to_string(&entry))

// In consensus core event loop:
maybe_record_event(&state.recorder, &event);
```

## 8. Persistence Model

Debugging tools read data from disk (RocksDB, WAL file, events log) in read-only mode. They do not write to the database (except `repair-db` which uses RocksDB's repair utility). The event log (`/data/debug/events.log`) is append-only JSON lines written by the consensus core when recording is enabled. It is not cleaned up automatically and must be managed by the operator.

## 9. Concurrency Model

All debugging subcommands are single-threaded and run sequentially. They open databases in read-only mode where possible. The `replay` subcommand spawns a single consensus thread (using the existing production consensus code) but manages it synchronously from the main thread. No async runtime is required.

## 10. Configuration

```toml
# node.toml — no separate [tools] section
# Tools are invoked as CLI subcommands, not at runtime

# Event recording is controlled by environment variable:
# RUSTBFT_RECORD_EVENTS=true

# Event log location (when recording is enabled):
# /data/debug/events.log
```

```bash
# Useful environment variables
RUSTBFT_RECORD_EVENTS=true   # Enable event recording
RUSTBFT_RECORD_NODE_ID=node0 # Record events for a specific node (for multi-node in-process replay)
```

## 11. Observability

The debugging tools write to stdout (human-readable or `--json` mode). They do not emit Prometheus metrics. Exit code 0 on success, 1 on any error.

`wal-inspect` always prints a diagnosis summary at the end:
```
DIAGNOSIS: Crashed during block execution at height 42
  Last WAL entry: CommitStarted { height: 42, block_hash: "abc..." }
  Action needed: restart the node; WAL recovery will re-execute the block
```

## 12. Testing Strategy

- **`test_wal_inspect_clean`**: write WAL with CommitCompleted, run wal_inspect → diagnosis=Clean, entries count matches
- **`test_wal_inspect_crash_during_execution`**: write CommitStarted without Completed, run wal_inspect → diagnosis=CrashedDuringExecution with correct height
- **`test_wal_inspect_crc_error`**: corrupt one WAL entry's CRC, run wal_inspect → crc_errors=1, diagnosis=Corrupt
- **`test_wal_inspect_empty_file`**: run on empty WAL file → diagnosis=Clean, entries=[]
- **`test_wal_inspect_json_output`**: `--json` flag → valid JSON output parseable by jq
- **`test_state_dump_valid_height`**: create test DB at height 5, state_dump height=5 → all accounts present with correct balances
- **`test_state_dump_height_not_found`**: state_dump height=99 on DB with latest=5 → Err(HeightNotFound)
- **`test_state_dump_json_round_trip`**: state_dump, serialize to JSON, deserialize → same data
- **`test_repair_db_valid`**: run repair_db on a valid test DB → Ok(()), DB still readable
- **`test_replay_deterministic`**: record events from a consensus test, replay → same commands emitted in same order
- **`test_replay_verbose_output`**: replay with `--verbose` → prints each transition to stdout
- **`test_event_recording_enabled`**: set RUSTBFT_RECORD_EVENTS=true, run consensus with 5 events → events.log has 5 JSON lines
- **`test_event_recording_disabled`**: RUSTBFT_RECORD_EVENTS not set → events.log not created
- **`test_debug_bundle_script_creates_tarball`**: run collect-debug-bundle.sh against Docker cluster → tarball created with expected files (status.json, logs.json, wal.txt, metrics.txt)

## 13. Open Questions

- **`state-dump` for large state**: If the state has millions of accounts, a full dump to JSON is impractical. A future `--address-filter` flag or paginated output would help. For MVP, full dump is acceptable given the small expected state size.
- **Cross-node state comparison**: The `state-dump` tool dumps to JSON, and operators can use `diff` to compare two nodes' state. A dedicated `state-diff --node-a <path> --node-b <path>` command would be more ergonomic. Deferred post-MVP.
