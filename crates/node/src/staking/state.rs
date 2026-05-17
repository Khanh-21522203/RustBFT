use std::collections::BTreeMap;

use rustbft_core::{Address, ValidatorId, ValidatorUpdate};
use serde::{Deserialize, Serialize};

use super::governance::{GovernanceAction, GovernanceValidatorTx};
use super::lifecycle::{UnbondingEntry, ValidatorLifecycle};
use super::metadata::{ValidatorEconomics, ValidatorMetadata};
use super::slashing::SlashEvent;
use super::tx::StakingTx;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatorRecord {
    pub validator_id: ValidatorId,
    pub operator: Address,
    pub status: ValidatorLifecycle,
    pub self_bonded: u128,
    pub delegated: u128,
    pub voting_power: u64,
    pub metadata: ValidatorMetadata,
    pub economics: ValidatorEconomics,
}

impl ValidatorRecord {
    pub fn total_stake(&self) -> u128 {
        self.self_bonded.saturating_add(self.delegated)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationRecord {
    pub delegator: Address,
    pub validator_id: ValidatorId,
    pub amount: u128,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StakingState {
    #[serde(serialize_with = "ser_validators", deserialize_with = "de_validators")]
    pub validators: BTreeMap<ValidatorId, ValidatorRecord>,
    #[serde(
        serialize_with = "ser_delegations",
        deserialize_with = "de_delegations"
    )]
    pub delegations: BTreeMap<(Address, ValidatorId), DelegationRecord>,
    pub unbonding: Vec<UnbondingEntry>,
    pub pending_slashes: Vec<SlashEvent>,
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum StakingError {
    #[error("staking transition is scaffolded but not implemented: {0}")]
    NotImplemented(&'static str),
    #[error("governance validator update has zero power")]
    ZeroPower,
}

impl StakingState {
    pub fn apply_tx(&mut self, tx: StakingTx) -> Result<Vec<ValidatorUpdate>, StakingError> {
        match tx {
            StakingTx::GovernanceValidator(tx) => self.apply_governance_validator_tx(tx),
            StakingTx::Bond(_) => {
                // Future: lock self-bonded funds and update validator lifecycle.
                Err(StakingError::NotImplemented("bond"))
            }
            StakingTx::Unbond(_) => {
                // Future: create unbonding entries with delayed withdrawal.
                Err(StakingError::NotImplemented("unbond"))
            }
            StakingTx::Delegate(_) => {
                // Future: account delegated stake toward validator voting power.
                Err(StakingError::NotImplemented("delegate"))
            }
            StakingTx::Undelegate(_) => {
                // Future: reduce delegation and start an unbonding delay.
                Err(StakingError::NotImplemented("undelegate"))
            }
        }
    }

    pub fn apply_governance_validator_tx(
        &mut self,
        tx: GovernanceValidatorTx,
    ) -> Result<Vec<ValidatorUpdate>, StakingError> {
        let update = tx.to_validator_update();
        match tx.action {
            GovernanceAction::RemoveValidator => {
                if let Some(record) = self.validators.get_mut(&update.id) {
                    record.status = ValidatorLifecycle::Unbonded;
                    record.voting_power = 0;
                }
                Ok(vec![update])
            }
            GovernanceAction::AddValidator | GovernanceAction::UpdatePower => {
                if update.new_power == 0 {
                    return Err(StakingError::ZeroPower);
                }

                self.validators
                    .entry(update.id)
                    .and_modify(|record| {
                        record.status = ValidatorLifecycle::Bonded;
                        record.voting_power = update.new_power;
                        if let Some(metadata) = tx.metadata.clone() {
                            record.metadata = metadata;
                        }
                        if let Some(economics) = tx.economics.clone() {
                            record.economics = economics;
                        }
                    })
                    .or_insert_with(|| ValidatorRecord {
                        validator_id: update.id,
                        operator: tx.authority,
                        status: ValidatorLifecycle::Bonded,
                        self_bonded: 0,
                        delegated: 0,
                        voting_power: update.new_power,
                        metadata: tx.metadata.unwrap_or_default(),
                        economics: tx.economics.unwrap_or_default(),
                    });

                Ok(vec![update])
            }
        }
    }

    pub fn end_block(&mut self, _height: u64) -> Vec<ValidatorUpdate> {
        // Future: convert staking, slashing, and reward effects into validator updates.
        Vec::new()
    }
}

fn ser_validators<S>(
    map: &BTreeMap<ValidatorId, ValidatorRecord>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeSeq;
    let mut seq = serializer.serialize_seq(Some(map.len()))?;
    for record in map.values() {
        seq.serialize_element(record)?;
    }
    seq.end()
}

fn de_validators<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<ValidatorId, ValidatorRecord>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let records: Vec<ValidatorRecord> = Deserialize::deserialize(deserializer)?;
    let mut map = BTreeMap::new();
    for record in records {
        map.insert(record.validator_id, record);
    }
    Ok(map)
}

fn ser_delegations<S>(
    map: &BTreeMap<(Address, ValidatorId), DelegationRecord>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeSeq;
    let mut seq = serializer.serialize_seq(Some(map.len()))?;
    for record in map.values() {
        seq.serialize_element(record)?;
    }
    seq.end()
}

fn de_delegations<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<(Address, ValidatorId), DelegationRecord>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let records: Vec<DelegationRecord> = Deserialize::deserialize(deserializer)?;
    let mut map = BTreeMap::new();
    for record in records {
        map.insert((record.delegator, record.validator_id), record);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use rustbft_core::{Address, ValidatorId};

    use super::super::governance::{GovernanceAction, GovernanceValidatorTx};
    use super::super::tx::{BondTx, StakingTx};
    use super::*;

    fn addr(seed: u8) -> Address {
        Address([seed; 20])
    }

    fn validator_id(seed: u8) -> ValidatorId {
        ValidatorId([seed; 32])
    }

    #[test]
    fn governance_add_validator_updates_state_and_emits_validator_update() {
        let mut state = StakingState::default();
        let validator_id = validator_id(7);

        let updates = state
            .apply_tx(StakingTx::GovernanceValidator(GovernanceValidatorTx {
                authority: addr(1),
                action: GovernanceAction::AddValidator,
                validator_id,
                new_power: 10,
                metadata: Some(ValidatorMetadata {
                    moniker: Some("validator-seven".to_string()),
                    p2p_address: Some("127.0.0.1:26656".to_string()),
                    ..Default::default()
                }),
                economics: Some(ValidatorEconomics {
                    min_self_bond: 1_000,
                    ..Default::default()
                }),
                reason: Some("bootstrap validator".to_string()),
            }))
            .unwrap();

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].id, validator_id);
        assert_eq!(updates[0].new_power, 10);

        let record = state.validators.get(&validator_id).unwrap();
        assert_eq!(record.status, ValidatorLifecycle::Bonded);
        assert_eq!(record.voting_power, 10);
        assert_eq!(record.metadata.moniker.as_deref(), Some("validator-seven"));
        assert_eq!(
            record.metadata.p2p_address.as_deref(),
            Some("127.0.0.1:26656")
        );
        assert_eq!(record.economics.min_self_bond, 1_000);
    }

    #[test]
    fn governance_remove_validator_marks_unbonded_and_emits_zero_power() {
        let mut state = StakingState::default();
        let validator_id = validator_id(8);

        state
            .apply_tx(StakingTx::GovernanceValidator(GovernanceValidatorTx {
                authority: addr(1),
                action: GovernanceAction::AddValidator,
                validator_id,
                new_power: 10,
                metadata: None,
                economics: None,
                reason: None,
            }))
            .unwrap();

        let updates = state
            .apply_tx(StakingTx::GovernanceValidator(GovernanceValidatorTx {
                authority: addr(1),
                action: GovernanceAction::RemoveValidator,
                validator_id,
                new_power: 10,
                metadata: None,
                economics: None,
                reason: Some("operator requested removal".to_string()),
            }))
            .unwrap();

        assert_eq!(updates[0].id, validator_id);
        assert_eq!(updates[0].new_power, 0);
        assert_eq!(
            state.validators.get(&validator_id).unwrap().status,
            ValidatorLifecycle::Unbonded
        );
    }

    #[test]
    fn bond_tx_is_scaffolded_not_implemented() {
        let mut state = StakingState::default();
        let err = state
            .apply_tx(StakingTx::Bond(BondTx {
                signer: addr(2),
                validator_id: validator_id(9),
                amount: 100,
            }))
            .unwrap_err();

        assert_eq!(err, StakingError::NotImplemented("bond"));
    }
}
