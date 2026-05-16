use rustbft_core::{Block, Hash, ValidatorId};
use serde::{Deserialize, Serialize};

pub type ViewNumber = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Phase {
    Prepare,
    PreCommit,
    Commit,
    Decide,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotStuffProposal {
    pub view: ViewNumber,
    pub phase: Phase,
    pub block: Block,
    pub parent_block_hash: Hash,
    pub justify_qc: crate::qc::QuorumCertificate,
    pub proposer: ValidatorId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedHotStuffProposal {
    pub proposal: HotStuffProposal,
    #[serde(with = "serde_bytes")]
    pub signature: [u8; 64],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotStuffVote {
    pub view: ViewNumber,
    pub phase: Phase,
    pub block_hash: Hash,
    pub validator: ValidatorId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedHotStuffVote {
    pub vote: HotStuffVote,
    #[serde(with = "serde_bytes")]
    pub signature: [u8; 64],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotStuffTimeout {
    pub view: ViewNumber,
    pub validator: ValidatorId,
    pub high_qc: crate::qc::QuorumCertificate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedHotStuffTimeout {
    pub timeout: HotStuffTimeout,
    #[serde(with = "serde_bytes")]
    pub signature: [u8; 64],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeoutCertificate {
    pub view: ViewNumber,
    pub high_qc: crate::qc::QuorumCertificate,
    pub timeouts: Vec<SignedHotStuffTimeout>,
}
