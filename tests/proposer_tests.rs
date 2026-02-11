//! Proposer selection unit tests (doc 13 section 2.2).

use RustBFT::consensus::proposer::{select_proposer, selection_steps, ProposerState};
use RustBFT::crypto::hash::sha256;
use RustBFT::types::{Validator, ValidatorId, ValidatorSet};

fn make_id(seed: u8) -> ValidatorId {
    let mut id = [0u8; 32];
    id[0] = seed;
    ValidatorId(id)
}

fn make_vset(powers: &[(u8, u64)]) -> ValidatorSet {
    let validators: Vec<Validator> = powers
        .iter()
        .map(|(seed, power)| Validator {
            id: make_id(*seed),
            voting_power: *power,
        })
        .collect();
    let hash = sha256(&[powers.len() as u8]);
    ValidatorSet::new(validators, hash)
}

/// Property: For any validator set and any (height, round),
/// proposer_selection returns exactly one validator that is in the set.
#[test]
fn test_proposer_always_in_set() {
    let vset = make_vset(&[(1, 100), (2, 200), (3, 50)]);
    for h in 0..50 {
        for r in 0..10 {
            let proposer = select_proposer(&vset, h, r);
            assert!(vset.contains(&proposer),
                "proposer at h={} r={} must be in validator set", h, r);
        }
    }
}

/// Property: Same inputs → same output (determinism).
#[test]
fn test_proposer_deterministic() {
    let vset = make_vset(&[(1, 100), (2, 200), (3, 50)]);
    for h in 0..20 {
        for r in 0..5 {
            let a = select_proposer(&vset, h, r);
            let b = select_proposer(&vset, h, r);
            assert_eq!(a, b, "proposer must be deterministic at h={} r={}", h, r);
        }
    }
}

/// Different heights should (eventually) produce different proposers for round 0.
#[test]
fn test_proposer_rotates_across_heights() {
    let vset = make_vset(&[(1, 1), (2, 1), (3, 1)]);
    let mut proposers = std::collections::BTreeSet::new();
    for h in 0..30 {
        proposers.insert(select_proposer(&vset, h, 0));
    }
    assert!(proposers.len() > 1, "proposer should rotate across heights");
}

/// Higher voting power should be selected more often.
#[test]
fn test_proposer_weighted() {
    let vset = make_vset(&[(1, 1), (2, 9)]);
    let id2 = make_id(2);
    let mut count_id2 = 0;
    let total = 100;
    for h in 0..total {
        if select_proposer(&vset, h, 0) == id2 {
            count_id2 += 1;
        }
    }
    // Validator 2 has 9/10 of total power, should be selected ~90% of the time
    assert!(count_id2 > 70, "validator with 9x power should be selected frequently, got {}", count_id2);
}

/// selection_steps is consistent with the formula.
#[test]
fn test_selection_steps_formula() {
    assert_eq!(selection_steps(0, 0), 1);
    assert_eq!(selection_steps(10, 5), 16);
    assert_eq!(selection_steps(100, 0), 101);
}

/// ProposerState incremental API produces same result as stateless select_proposer.
#[test]
fn test_proposer_state_matches_stateless() {
    let vset = make_vset(&[(1, 100), (2, 200), (3, 50)]);
    for h in 0..10 {
        for r in 0..5 {
            let stateless = select_proposer(&vset, h, r);

            // Reproduce with ProposerState
            let mut state = ProposerState::new(&vset);
            let steps = selection_steps(h, r);
            let mut last = vset.ids_in_order().next().copied().unwrap();
            for _ in 0..steps {
                last = state.next_proposer(&vset);
            }
            assert_eq!(stateless, last,
                "stateless and stateful must agree at h={} r={}", h, r);
        }
    }
}
