use crate::types::{Hash, SignedVote, ValidatorId};
use serde::{Deserialize, Serialize};

/// Commit info: >2/3 precommit signatures for the previous block (doc 8 section 8).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitInfo {
    pub height: u64,
    pub round: u32,
    pub block_hash: Hash,
    pub signatures: Vec<SignedVote>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub txs: Vec<Vec<u8>>,
    pub last_commit: Option<CommitInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockHeader {
    pub height: u64,
    pub timestamp_ms: u64, // advisory only
    pub prev_block_hash: Hash,
    pub proposer: ValidatorId,
    pub validator_set_hash: Hash,
    pub state_root: Hash,
    pub tx_merkle_root: Hash,
}
