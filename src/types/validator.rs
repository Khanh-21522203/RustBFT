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

    pub fn ids_in_order(&self) -> impl Iterator<Item = &ValidatorId> {
        self.validators.keys()
    }

    pub fn validators_in_order(&self) -> impl Iterator<Item = &Validator> {
        self.validators.values()
    }
}
