use crate::types::{ValidatorId, ValidatorSet};
use std::cmp::Ordering;
use std::collections::BTreeMap;

/// Deterministic weighted round-robin proposer selection.
///
/// Uses an incremental priority accumulator that is O(n) per selection step.
/// The `ProposerState` should be cached across rounds within the same height
/// and reset when the validator set changes.
#[derive(Clone, Debug)]
pub struct ProposerState {
    priorities: BTreeMap<ValidatorId, i128>,
    total_power: i128,
}

impl ProposerState {
    /// Initialize proposer state from a validator set (all priorities = 0).
    pub fn new(vset: &ValidatorSet) -> Self {
        let mut priorities = BTreeMap::new();
        for v in vset.validators_in_order() {
            priorities.insert(v.id, 0);
        }
        Self {
            priorities,
            total_power: vset.total_power() as i128,
        }
    }

    /// Perform one selection step: add voting power, pick best, subtract total.
    /// Returns the selected proposer for this step.
    pub fn next_proposer(&mut self, vset: &ValidatorSet) -> ValidatorId {
        // 1. Add voting_power to each validator's priority
        for v in vset.validators_in_order() {
            if let Some(p) = self.priorities.get_mut(&v.id) {
                *p += v.voting_power as i128;
            }
        }

        // 2. Pick validator with highest priority (deterministic tie-break by ValidatorId)
        let mut best: Option<(ValidatorId, i128)> = None;
        for (id, p) in self.priorities.iter() {
            match best {
                None => best = Some((*id, *p)),
                Some((best_id, best_p)) => {
                    let ord = p.cmp(&best_p).then_with(|| id.cmp(&best_id));
                    if ord == Ordering::Greater {
                        best = Some((*id, *p));
                    }
                }
            }
        }

        let (best_id, _) = best.expect("non-empty validator set");

        // 3. Subtract total_power from the selected validator
        if let Some(p) = self.priorities.get_mut(&best_id) {
            *p -= self.total_power;
        }

        best_id
    }
}

/// Stateless proposer selection: O(n * (height + round)) per call.
/// Computes the proposer for a given (height, round) by running the round-robin
/// from scratch. Each height starts fresh (proposer state reset per doc 8 section 6).
/// Height is incorporated so different heights pick different proposers at round 0.
pub fn select_proposer(vset: &ValidatorSet, height: u64, round: u32) -> ValidatorId {
    let mut state = ProposerState::new(vset);
    let total_steps = selection_steps(height, round);
    let mut last = vset.ids_in_order().next().copied().expect("non-empty");
    for _ in 0..total_steps {
        last = state.next_proposer(vset);
    }
    last
}

/// Deterministic mapping from (height, round) to selection steps.
pub fn selection_steps(height: u64, round: u32) -> u64 {
    height.saturating_add(round as u64).saturating_add(1)
}
