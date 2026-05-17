use rustbft_core::{Block, Hash};

use crate::types::{
    SignedHotStuffProposal, SignedHotStuffTimeout, SignedHotStuffVote, TimeoutCertificate,
    ViewNumber,
};

#[derive(Clone, Debug)]
pub enum HotStuffEvent {
    BlockReady { block: Block },
    ProposalReceived { proposal: SignedHotStuffProposal },
    VoteReceived { vote: SignedHotStuffVote },
    TimeoutReceived { timeout: SignedHotStuffTimeout },
    TimeoutCertificateReceived { certificate: TimeoutCertificate },
    ViewTimeout { view: ViewNumber },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotStuffCommand {
    ReapTxs { max_bytes: usize },
    BroadcastProposal { proposal: SignedHotStuffProposal },
    BroadcastVote { vote: SignedHotStuffVote },
    BroadcastTimeout { timeout: SignedHotStuffTimeout },
    BroadcastTimeoutCertificate { certificate: TimeoutCertificate },
    ExecuteBlock { block: Block },
    PersistBlock { block: Block, state_root: Hash },
    ScheduleViewTimeout { view: ViewNumber, duration_ms: u64 },
}
