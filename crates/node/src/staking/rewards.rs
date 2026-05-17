use rustbft_core::{Address, ValidatorId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewardPolicy {
    pub block_reward: u128,
    pub proposer_bonus_ppm: u32,
}

impl Default for RewardPolicy {
    fn default() -> Self {
        Self {
            block_reward: 0,
            proposer_bonus_ppm: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewardEvent {
    pub height: u64,
    pub validator_id: ValidatorId,
    pub recipient: Address,
    pub amount: u128,
}
