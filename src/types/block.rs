use crate::types::{Hash, ValidatorId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub txs: Vec<Vec<u8>>, // placeholder Transaction bytes; mempool/state will define later
    // last_commit: CommitInfo (để sau, khi có types cho commit signatures)
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
