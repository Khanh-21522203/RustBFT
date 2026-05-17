use rustbft_core::{Address, ValidatorId};
use serde::{Deserialize, Serialize};

use super::governance::GovernanceValidatorTx;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BondTx {
    pub signer: Address,
    pub validator_id: ValidatorId,
    pub amount: u128,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnbondTx {
    pub signer: Address,
    pub validator_id: ValidatorId,
    pub amount: u128,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegateTx {
    pub delegator: Address,
    pub validator_id: ValidatorId,
    pub amount: u128,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndelegateTx {
    pub delegator: Address,
    pub validator_id: ValidatorId,
    pub amount: u128,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StakingTx {
    Bond(BondTx),
    Unbond(UnbondTx),
    Delegate(DelegateTx),
    Undelegate(UndelegateTx),
    GovernanceValidator(GovernanceValidatorTx),
}
