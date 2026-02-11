use crate::types::{ValidatorId, ValidatorSet};
use std::cmp::Ordering;
use std::collections::BTreeMap;

/// Deterministic weighted round-robin.
/// IMPORTANT: priority is recomputed from genesis / last committed validator set by "steps".
/// For (height, round), we model "steps = height * K + round" with K large enough,
/// but to stay exact, caller should pass a deterministic `selection_steps` counter.
pub fn select_proposer(vset: &ValidatorSet, selection_steps: u64) -> ValidatorId {
    // priority accumulator per validator
    // recompute from 0 deterministically each call
    let mut priority: BTreeMap<ValidatorId, i128> = BTreeMap::new();
    for id in vset.ids_in_order() {
        priority.insert(*id, 0);
    }

    let total = vset.total_power() as i128;

    for _ in 0..selection_steps {
        // add voting power
        for v in vset.validators_in_order() {
            let e = priority.get_mut(&v.id).expect("exists");
            *e += v.voting_power as i128;
        }

        // pick max priority (deterministic tie-break by ValidatorId order)
        let mut best: Option<(ValidatorId, i128)> = None;
        for (id, p) in priority.iter() {
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

        let (best_id, _) = best.expect("non-empty vset");
        let e = priority.get_mut(&best_id).expect("exists");
        *e -= total;
    }

    // After `selection_steps` iterations, the last selected proposer is:
    // To obtain it, do one more selection step and return that best.
    // This matches “selection step” semantics.
    // Implement by performing one extra step and returning best.

    // add voting power
    for v in vset.validators_in_order() {
        let e = priority.get_mut(&v.id).expect("exists");
        *e += v.voting_power as i128;
    }
    let mut best: Option<(ValidatorId, i128)> = None;
    for (id, p) in priority.iter() {
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
    best.expect("non-empty").0
}

/// Deterministic mapping from (height, round) to selection steps.
/// You can revise once you define genesis height and whether height starts at 1.
pub fn selection_steps(height: u64, round: u32) -> u64 {
    // Simple bijection: steps = height * 1_000_000 + round
    // (assumes round won't exceed 1_000_000 in practice)
    height.saturating_mul(1_000_000).saturating_add(round as u64)
}
