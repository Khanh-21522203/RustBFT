use crate::crypto::hash::sha256;
use crate::types::Hash;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ValidatorId(pub [u8; 32]);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Validator {
    pub id: ValidatorId,
    pub voting_power: u64,
}

/// A single validator update request (produced by EndBlock).
/// power = 0 means remove.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidatorUpdate {
    pub id: ValidatorId,
    pub new_power: u64,
}

/// Safety rules for validator set changes (doc 8 section 4).
#[derive(Clone, Debug)]
pub struct ValidatorSetPolicy {
    /// Maximum fraction of total power that can change in one block (numerator/denominator).
    /// Default: 1/3 (max_power_change_num=1, max_power_change_den=3).
    pub max_power_change_num: u64,
    pub max_power_change_den: u64,
    /// Minimum number of validators.
    pub min_validators: usize,
    /// Maximum number of validators.
    pub max_validators: usize,
}

impl Default for ValidatorSetPolicy {
    fn default() -> Self {
        Self {
            max_power_change_num: 1,
            max_power_change_den: 3,
            min_validators: 1,
            max_validators: 150,
        }
    }
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum ValidatorSetError {
    #[error("would drop below minimum validators ({min})")]
    TooFewValidators { min: usize },
    #[error("would exceed maximum validators ({max})")]
    TooManyValidators { max: usize },
    #[error("power change {delta} exceeds max allowed {max_allowed}")]
    PowerChangeTooLarge { delta: u64, max_allowed: u64 },
    #[error("cannot set power to zero for non-existent validator")]
    RemoveNonExistent,
    #[error("total power would be zero")]
    ZeroTotalPower,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidatorSet {
    // Deterministic iteration order, no HashMap.
    validators: BTreeMap<ValidatorId, Validator>,
    total_power: u64,
    // Hash of validator set used in BlockHeader
    pub set_hash: Hash,
}

impl ValidatorSet {
    pub fn new(validators: Vec<Validator>, set_hash: Hash) -> Self {
        let mut map = BTreeMap::new();
        let mut total = 0u64;
        for v in validators {
            total = total.saturating_add(v.voting_power);
            map.insert(v.id, v);
        }
        Self { validators: map, total_power: total, set_hash }
    }

    pub fn total_power(&self) -> u64 {
        self.total_power
    }

    pub fn voting_power(&self, id: &ValidatorId) -> u64 {
        self.validators.get(id).map(|v| v.voting_power).unwrap_or(0)
    }

    pub fn contains(&self, id: &ValidatorId) -> bool {
        self.validators.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.validators.len()
    }

    pub fn is_empty(&self) -> bool {
        self.validators.is_empty()
    }

    pub fn ids_in_order(&self) -> impl Iterator<Item = &ValidatorId> {
        self.validators.keys()
    }

    pub fn validators_in_order(&self) -> impl Iterator<Item = &Validator> {
        self.validators.values()
    }

    pub fn get(&self, id: &ValidatorId) -> Option<&Validator> {
        self.validators.get(id)
    }

    /// Compute a deterministic hash of the validator set.
    pub fn compute_hash(&self) -> Hash {
        let mut bytes = Vec::new();
        for (id, v) in &self.validators {
            bytes.extend_from_slice(&id.0);
            bytes.extend_from_slice(&v.voting_power.to_be_bytes());
        }
        sha256(&bytes)
    }

    /// Apply a batch of validator updates, enforcing safety rules (doc 8 section 4).
    /// Returns the new ValidatorSet or an error if safety rules are violated.
    /// Updates take effect at H+1 (caller is responsible for timing).
    pub fn apply_updates(
        &self,
        updates: &[ValidatorUpdate],
        policy: &ValidatorSetPolicy,
    ) -> Result<ValidatorSet, ValidatorSetError> {
        let mut new_map = self.validators.clone();
        let mut power_delta: u64 = 0;

        for u in updates {
            let old_power = new_map.get(&u.id).map(|v| v.voting_power).unwrap_or(0);

            if u.new_power == 0 {
                // Remove validator
                if old_power == 0 {
                    return Err(ValidatorSetError::RemoveNonExistent);
                }
                new_map.remove(&u.id);
                power_delta = power_delta.saturating_add(old_power);
            } else if old_power == 0 {
                // Add new validator
                new_map.insert(u.id, Validator { id: u.id, voting_power: u.new_power });
                power_delta = power_delta.saturating_add(u.new_power);
            } else {
                // Update existing validator power
                let diff = if u.new_power > old_power {
                    u.new_power - old_power
                } else {
                    old_power - u.new_power
                };
                power_delta = power_delta.saturating_add(diff);
                new_map.insert(u.id, Validator { id: u.id, voting_power: u.new_power });
            }
        }

        // Safety check: minimum validators
        if new_map.len() < policy.min_validators {
            return Err(ValidatorSetError::TooFewValidators { min: policy.min_validators });
        }

        // Safety check: maximum validators
        if new_map.len() > policy.max_validators {
            return Err(ValidatorSetError::TooManyValidators { max: policy.max_validators });
        }

        // Safety check: total power > 0
        let new_total: u64 = new_map.values().map(|v| v.voting_power).sum();
        if new_total == 0 {
            return Err(ValidatorSetError::ZeroTotalPower);
        }

        // Safety check: max power change per block
        let max_allowed = self.total_power
            .saturating_mul(policy.max_power_change_num)
            / policy.max_power_change_den;
        if power_delta > max_allowed {
            return Err(ValidatorSetError::PowerChangeTooLarge {
                delta: power_delta,
                max_allowed,
            });
        }

        let mut new_set = ValidatorSet {
            validators: new_map,
            total_power: new_total,
            set_hash: Hash::ZERO,
        };
        new_set.set_hash = new_set.compute_hash();
        Ok(new_set)
    }
}
