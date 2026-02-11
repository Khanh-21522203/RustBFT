use crate::types::{Hash, ValidatorId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum VoteType {
    Prevote,
    Precommit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vote {
    pub vote_type: VoteType,
    pub height: u64,
    pub round: u32,
    pub block_hash: Option<Hash>, // None = nil
    pub validator: ValidatorId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedVote {
    pub vote: Vote,
    #[serde(with = "serde_bytes")]
    pub signature: [u8; 64], // ed25519 signature bytes
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Evidence {
    pub vote_a: SignedVote,
    pub vote_b: SignedVote,
}
