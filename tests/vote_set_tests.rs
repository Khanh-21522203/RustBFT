//! VoteSet unit tests (doc 13 section 2.2).

use RustBFT::consensus::vote_set::VoteSet;
use RustBFT::crypto::hash::sha256;
use RustBFT::types::*;

fn make_id(seed: u8) -> ValidatorId {
    let mut id = [0u8; 32];
    id[0] = seed;
    ValidatorId(id)
}

fn make_vset(n: usize) -> (ValidatorSet, Vec<ValidatorId>) {
    let mut validators = Vec::new();
    let mut ids = Vec::new();
    for i in 0..n {
        let id = make_id(i as u8 + 1);
        ids.push(id);
        validators.push(Validator { id, voting_power: 1 });
    }
    (ValidatorSet::new(validators, sha256(&[n as u8])), ids)
}

fn make_vote(vt: VoteType, h: u64, r: u32, bh: Option<Hash>, v: ValidatorId) -> SignedVote {
    SignedVote {
        vote: Vote { vote_type: vt, height: h, round: r, block_hash: bh, validator: v },
        signature: [0u8; 64],
    }
}

#[test]
fn test_insert_and_count() {
    let (vset, ids) = make_vset(4);
    let mut vs = VoteSet::new(1);
    let bh = Hash([1u8; 32]);

    vs.insert_vote(&vset, make_vote(VoteType::Prevote, 1, 0, Some(bh), ids[0])).unwrap();
    vs.insert_vote(&vset, make_vote(VoteType::Prevote, 1, 0, Some(bh), ids[1])).unwrap();

    assert_eq!(vs.sum_power_for(&vset, 0, VoteType::Prevote, Some(bh)), 2);
}

#[test]
fn test_duplicate_vote_rejected() {
    let (vset, ids) = make_vset(4);
    let mut vs = VoteSet::new(1);
    let bh = Hash([1u8; 32]);

    let v = make_vote(VoteType::Prevote, 1, 0, Some(bh), ids[0]);
    vs.insert_vote(&vset, v.clone()).unwrap();
    let err = vs.insert_vote(&vset, v).unwrap_err();
    assert!(err.to_string().contains("duplicate"), "should reject duplicate vote");
}

#[test]
fn test_equivocation_detected() {
    let (vset, ids) = make_vset(4);
    let mut vs = VoteSet::new(1);

    let v1 = make_vote(VoteType::Prevote, 1, 0, Some(Hash([1u8; 32])), ids[0]);
    let v2 = make_vote(VoteType::Prevote, 1, 0, Some(Hash([2u8; 32])), ids[0]);

    vs.insert_vote(&vset, v1).unwrap();
    let err = vs.insert_vote(&vset, v2).unwrap_err();
    assert!(err.to_string().contains("equivocation"), "should detect equivocation");
    assert_eq!(vs.evidence().len(), 1, "should record evidence");
}

#[test]
fn test_height_mismatch_rejected() {
    let (vset, ids) = make_vset(4);
    let mut vs = VoteSet::new(1);
    let v = make_vote(VoteType::Prevote, 2, 0, Some(Hash([1u8; 32])), ids[0]);
    let err = vs.insert_vote(&vset, v).unwrap_err();
    assert!(err.to_string().contains("height"), "should reject height mismatch");
}

#[test]
fn test_non_validator_rejected() {
    let (vset, _ids) = make_vset(4);
    let mut vs = VoteSet::new(1);
    let outsider = make_id(99);
    let v = make_vote(VoteType::Prevote, 1, 0, Some(Hash([1u8; 32])), outsider);
    let err = vs.insert_vote(&vset, v).unwrap_err();
    assert!(err.to_string().contains("non-validator"), "should reject non-validator");
}

#[test]
fn test_quorum_threshold() {
    // 2/3 * 4 + 1 = 3
    assert_eq!(VoteSet::quorum_threshold(4), 3);
    // 2/3 * 3 + 1 = 3
    assert_eq!(VoteSet::quorum_threshold(3), 3);
    // 2/3 * 100 + 1 = 67
    assert_eq!(VoteSet::quorum_threshold(100), 67);
    // 2/3 * 1 + 1 = 1
    assert_eq!(VoteSet::quorum_threshold(1), 1);
}

#[test]
fn test_has_polka_for() {
    let (vset, ids) = make_vset(4);
    let mut vs = VoteSet::new(1);
    let bh = Hash([1u8; 32]);

    // 2 prevotes: not enough (need 3 for quorum of 4)
    vs.insert_vote(&vset, make_vote(VoteType::Prevote, 1, 0, Some(bh), ids[0])).unwrap();
    vs.insert_vote(&vset, make_vote(VoteType::Prevote, 1, 0, Some(bh), ids[1])).unwrap();
    assert!(!vs.has_polka_for(&vset, 0, Some(bh)));

    // 3 prevotes: quorum reached
    vs.insert_vote(&vset, make_vote(VoteType::Prevote, 1, 0, Some(bh), ids[2])).unwrap();
    assert!(vs.has_polka_for(&vset, 0, Some(bh)));
}

#[test]
fn test_has_polka_nil() {
    let (vset, ids) = make_vset(4);
    let mut vs = VoteSet::new(1);

    vs.insert_vote(&vset, make_vote(VoteType::Prevote, 1, 0, None, ids[0])).unwrap();
    vs.insert_vote(&vset, make_vote(VoteType::Prevote, 1, 0, None, ids[1])).unwrap();
    assert!(!vs.has_polka_for(&vset, 0, None));

    vs.insert_vote(&vset, make_vote(VoteType::Prevote, 1, 0, None, ids[2])).unwrap();
    assert!(vs.has_polka_for(&vset, 0, None));
}

#[test]
fn test_has_commit_quorum() {
    let (vset, ids) = make_vset(4);
    let mut vs = VoteSet::new(1);
    let bh = Hash([1u8; 32]);

    vs.insert_vote(&vset, make_vote(VoteType::Precommit, 1, 0, Some(bh), ids[0])).unwrap();
    vs.insert_vote(&vset, make_vote(VoteType::Precommit, 1, 0, Some(bh), ids[1])).unwrap();
    assert!(!vs.has_commit_quorum(&vset, 0, bh));

    vs.insert_vote(&vset, make_vote(VoteType::Precommit, 1, 0, Some(bh), ids[2])).unwrap();
    assert!(vs.has_commit_quorum(&vset, 0, bh));
}

#[test]
fn test_round_votes_isolation() {
    let (vset, ids) = make_vset(4);
    let mut vs = VoteSet::new(1);
    let bh = Hash([1u8; 32]);

    // Insert votes in round 0 and round 1
    vs.insert_vote(&vset, make_vote(VoteType::Prevote, 1, 0, Some(bh), ids[0])).unwrap();
    vs.insert_vote(&vset, make_vote(VoteType::Prevote, 1, 1, Some(bh), ids[1])).unwrap();

    // Round 0 should have 1 prevote
    assert_eq!(vs.sum_power_for(&vset, 0, VoteType::Prevote, Some(bh)), 1);
    // Round 1 should have 1 prevote
    assert_eq!(vs.sum_power_for(&vset, 1, VoteType::Prevote, Some(bh)), 1);
}
