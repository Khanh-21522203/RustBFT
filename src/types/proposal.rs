use crate::types::{Block, ValidatorId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub height: u64,
    pub round: u32,
    pub block: Block,
    pub valid_round: i32, // -1 if none
    pub proposer: ValidatorId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedProposal {
    pub proposal: Proposal,
    #[serde(with = "serde_bytes")]
    pub signature: [u8; 64],
}
