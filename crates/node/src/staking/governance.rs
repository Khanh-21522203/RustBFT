use rustbft_core::{Address, ValidatorId, ValidatorUpdate};
use serde::{Deserialize, Serialize};

use super::metadata::{ValidatorEconomics, ValidatorMetadata};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GovernanceAction {
    AddValidator,
    RemoveValidator,
    UpdatePower,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GovernanceValidatorTx {
    pub authority: Address,
    pub action: GovernanceAction,
    pub validator_id: ValidatorId,
    pub new_power: u64,
    pub metadata: Option<ValidatorMetadata>,
    pub economics: Option<ValidatorEconomics>,
    pub reason: Option<String>,
}

impl GovernanceValidatorTx {
    pub fn to_validator_update(&self) -> ValidatorUpdate {
        let new_power = match self.action {
            GovernanceAction::AddValidator | GovernanceAction::UpdatePower => self.new_power,
            GovernanceAction::RemoveValidator => 0,
        };

        ValidatorUpdate {
            id: self.validator_id,
            new_power,
        }
    }
}
