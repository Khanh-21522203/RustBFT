use crate::types::{Block, Hash, SignedProposal, SignedVote, ValidatorUpdate};

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
    WriteWAL { entry: Vec<u8> },

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
