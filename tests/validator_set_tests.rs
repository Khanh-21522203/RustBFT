//! Validator set unit tests (doc 13 section 2.2 + doc 8 safety rules).

use RustBFT::crypto::hash::sha256;
use RustBFT::types::*;

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

#[test]
fn test_basic_validator_set() {
    let vset = make_vset(&[(1, 100), (2, 200), (3, 50)]);
    assert_eq!(vset.total_power(), 350);
    assert_eq!(vset.len(), 3);
    assert!(vset.contains(&make_id(1)));
    assert!(!vset.contains(&make_id(99)));
    assert_eq!(vset.voting_power(&make_id(2)), 200);
    assert_eq!(vset.voting_power(&make_id(99)), 0);
}

#[test]
fn test_compute_hash_deterministic() {
    let vset = make_vset(&[(1, 100), (2, 200)]);
    let h1 = vset.compute_hash();
    let h2 = vset.compute_hash();
    assert_eq!(h1, h2, "compute_hash must be deterministic");
}

#[test]
fn test_apply_updates_add_validator() {
    let vset = make_vset(&[(1, 100), (2, 100), (3, 100)]);
    let policy = ValidatorSetPolicy::default();

    let updates = vec![ValidatorUpdate {
        id: make_id(4),
        new_power: 100,
    }];

    let new_vset = vset.apply_updates(&updates, &policy).unwrap();
    assert_eq!(new_vset.len(), 4);
    assert!(new_vset.contains(&make_id(4)));
    assert_eq!(new_vset.voting_power(&make_id(4)), 100);
}

#[test]
fn test_apply_updates_remove_validator() {
    let vset = make_vset(&[(1, 100), (2, 100), (3, 100)]);
    let policy = ValidatorSetPolicy::default();

    let updates = vec![ValidatorUpdate {
        id: make_id(3),
        new_power: 0,
    }];

    let new_vset = vset.apply_updates(&updates, &policy).unwrap();
    assert_eq!(new_vset.len(), 2);
    assert!(!new_vset.contains(&make_id(3)));
}

#[test]
fn test_apply_updates_change_power() {
    let vset = make_vset(&[(1, 100), (2, 100), (3, 100)]);
    let policy = ValidatorSetPolicy::default();

    let updates = vec![ValidatorUpdate {
        id: make_id(2),
        new_power: 50,
    }];

    let new_vset = vset.apply_updates(&updates, &policy).unwrap();
    assert_eq!(new_vset.voting_power(&make_id(2)), 50);
    assert_eq!(new_vset.total_power(), 250);
}

#[test]
fn test_apply_updates_reject_too_large_change() {
    let vset = make_vset(&[(1, 100), (2, 100), (3, 100)]);
    let policy = ValidatorSetPolicy::default(); // max 1/3 change

    // Adding 200 power when total is 300 → delta=200, max_allowed=100
    let updates = vec![ValidatorUpdate {
        id: make_id(4),
        new_power: 200,
    }];

    let result = vset.apply_updates(&updates, &policy);
    assert!(result.is_err(), "should reject power change > 1/3");
    match result.unwrap_err() {
        ValidatorSetError::PowerChangeTooLarge { .. } => {}
        e => panic!("unexpected error: {:?}", e),
    }
}

#[test]
fn test_apply_updates_reject_below_min_validators() {
    let vset = make_vset(&[(1, 100)]);
    let policy = ValidatorSetPolicy {
        min_validators: 1,
        ..Default::default()
    };

    // Remove the only validator
    let updates = vec![ValidatorUpdate {
        id: make_id(1),
        new_power: 0,
    }];

    let result = vset.apply_updates(&updates, &policy);
    assert!(result.is_err(), "should reject dropping below min validators");
}

#[test]
fn test_apply_updates_reject_remove_nonexistent() {
    let vset = make_vset(&[(1, 100), (2, 100)]);
    let policy = ValidatorSetPolicy::default();

    let updates = vec![ValidatorUpdate {
        id: make_id(99),
        new_power: 0,
    }];

    let result = vset.apply_updates(&updates, &policy);
    assert!(result.is_err(), "should reject removing non-existent validator");
}

#[test]
fn test_apply_updates_reject_zero_total_power() {
    let vset = make_vset(&[(1, 100)]);
    let mut policy = ValidatorSetPolicy::default();
    policy.min_validators = 0; // allow 0 validators
    policy.max_power_change_num = 1;
    policy.max_power_change_den = 1; // allow 100% change

    let updates = vec![ValidatorUpdate {
        id: make_id(1),
        new_power: 0,
    }];

    let result = vset.apply_updates(&updates, &policy);
    assert!(result.is_err(), "should reject zero total power");
}

#[test]
fn test_set_hash_updated_after_apply() {
    let vset = make_vset(&[(1, 100), (2, 100), (3, 100)]);
    let policy = ValidatorSetPolicy::default();

    let updates = vec![ValidatorUpdate {
        id: make_id(2),
        new_power: 50,
    }];

    let new_vset = vset.apply_updates(&updates, &policy).unwrap();
    assert_ne!(vset.set_hash, new_vset.set_hash, "set_hash should change after update");
    assert_eq!(new_vset.set_hash, new_vset.compute_hash(), "set_hash should match compute_hash");
}
