use rustbft_core::ValidatorId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidatorLifecycle {
    Candidate,
    Bonded,
    Unbonding,
    Unbonded,
    Jailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnbondingEntry {
    pub validator_id: ValidatorId,
    pub amount: u128,
    pub unlock_height: u64,
}
