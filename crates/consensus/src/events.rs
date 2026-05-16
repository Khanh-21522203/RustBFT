use rustbft_core::{Block, Hash, SignedProposal, SignedVote, ValidatorUpdate};

// ---- WAL entry types (moved from storage::wal) ----

/// WAL entry kinds (doc 9 section 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum WalEntryKind {
    Proposal = 0x01,
    Prevote = 0x02,
    Precommit = 0x03,
    Timeout = 0x04,
    RoundStep = 0x05,
}

impl WalEntryKind {
    pub fn from_u8(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(WalEntryKind::Proposal),
            0x02 => Some(WalEntryKind::Prevote),
            0x03 => Some(WalEntryKind::Precommit),
            0x04 => Some(WalEntryKind::Timeout),
            0x05 => Some(WalEntryKind::RoundStep),
            _ => None,
        }
    }
}

/// A single WAL entry.
/// Format on disk (per line): height(8) | round(4) | kind(1) | len(4) | data | checksum(32)
#[derive(Clone, Debug)]
pub struct WalEntry {
    pub height: u64,
    pub round: u32,
    pub kind: WalEntryKind,
    pub data: Vec<u8>,
}

// ---- Consensus events and commands ----

#[derive(Clone, Debug)]
pub enum ConsensusEvent {
    // From P2P
    ProposalReceived { proposal: SignedProposal },
    VoteReceived { vote: SignedVote },

    // From timers
    TimeoutPropose { height: u64, round: u32 },
    TimeoutPrevote { height: u64, round: u32 },
    TimeoutPrecommit { height: u64, round: u32 },

    // From state machine (after execution)
    BlockExecuted {
        height: u64,
        state_root: Hash,
        validator_updates: Vec<ValidatorUpdate>,
    },

    // From mempool (response to reap)
    TxsAvailable { txs: Vec<Vec<u8>> },
}

#[derive(Clone, Debug)]
pub enum ConsensusCommand {
    // To P2P
    BroadcastProposal { proposal: SignedProposal },
    BroadcastVote { vote: SignedVote },

    // To state machine
    ExecuteBlock { block: Block },

    // To mempool
    ReapTxs { max_bytes: usize },
    EvictTxs { tx_hashes: Vec<Hash> },

    // To storage
    PersistBlock { block: Block, state_root: Hash },
    WriteWAL { entry: WalEntry },

    // To timers
    ScheduleTimeout { timeout: Timeout },
    CancelTimeout { timeout: Timeout },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeoutKind {
    Propose,
    Prevote,
    Precommit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeout {
    pub height: u64,
    pub round: u32,
    pub kind: TimeoutKind,
    pub duration_ms: u64,
}
