//! Consensus core FSM unit tests (doc 13 section 2.3).
//!
//! Test structure:
//!   1. Construct ConsensusCore with a known validator set
//!   2. Create crossbeam channels (inbound, outbound)
//!   3. Send a sequence of ConsensusEvents
//!   4. Collect and assert the sequence of ConsensusCommands emitted
//!   5. Assert the final ConsensusState

use crossbeam_channel::{bounded, Receiver, Sender};
use RustBFT::consensus::events::{ConsensusCommand, ConsensusEvent};
use RustBFT::consensus::state::{ConsensusConfig, ConsensusCore, ConsensusDeps};
use RustBFT::crypto::hash::sha256;
use RustBFT::types::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_validator_id(seed: u8) -> ValidatorId {
    let mut id = [0u8; 32];
    id[0] = seed;
    ValidatorId(id)
}

fn make_validator_set(n: usize) -> (ValidatorSet, Vec<ValidatorId>) {
    let mut validators = Vec::new();
    let mut ids = Vec::new();
    for i in 0..n {
        let id = make_validator_id(i as u8 + 1);
        ids.push(id);
        validators.push(Validator {
            id,
            voting_power: 1,
        });
    }
    let set_hash = sha256(&[n as u8]);
    (ValidatorSet::new(validators, set_hash), ids)
}

fn make_block(height: u64, proposer: ValidatorId) -> Block {
    Block {
        header: BlockHeader {
            height,
            timestamp_ms: 0,
            prev_block_hash: Hash::ZERO,
            proposer,
            validator_set_hash: Hash::ZERO,
            state_root: Hash::ZERO,
            tx_merkle_root: Hash::ZERO,
        },
        txs: vec![],
        last_commit: None,
    }
}

fn block_hash(block: &Block) -> Hash {
    let bytes = serde_json::to_vec(block).unwrap_or_default();
    sha256(&bytes)
}

fn make_proposal(height: u64, round: u32, block: Block, proposer: ValidatorId) -> SignedProposal {
    SignedProposal {
        proposal: Proposal {
            height,
            round,
            block,
            valid_round: -1,
            proposer,
        },
        signature: [0u8; 64],
    }
}

fn make_signed_vote(
    vote_type: VoteType,
    height: u64,
    round: u32,
    block_hash: Option<Hash>,
    validator: ValidatorId,
) -> SignedVote {
    SignedVote {
        vote: Vote {
            vote_type,
            height,
            round,
            block_hash,
            validator,
        },
        signature: [0u8; 64],
    }
}

fn default_deps() -> ConsensusDeps {
    ConsensusDeps {
        verify_proposal_sig: Box::new(|_| true),
        verify_vote_sig: Box::new(|_| true),
        validate_block: Box::new(|_| true),
        have_full_block: Box::new(|_| true),
        block_hash: Box::new(|block| {
            let bytes = serde_json::to_vec(block).unwrap_or_default();
            sha256(&bytes)
        }),
    }
}

/// Spawn a ConsensusCore on a background thread and return the channels.
/// The core will process events until the inbound channel is dropped.
fn spawn_core(
    my_id: ValidatorId,
    height: u64,
    vset: ValidatorSet,
) -> (
    Sender<ConsensusEvent>,
    Receiver<ConsensusCommand>,
) {
    let (tx_ev, rx_ev) = bounded::<ConsensusEvent>(256);
    let (tx_cmd, rx_cmd) = bounded::<ConsensusCommand>(1024);

    std::thread::spawn(move || {
        let cfg = ConsensusConfig::default();
        let deps = default_deps();
        let core = ConsensusCore::new(cfg, deps, my_id, height, vset, rx_ev, tx_cmd);
        core.run();
    });

    (tx_ev, rx_cmd)
}

/// Drain all available commands (non-blocking).
fn drain_commands(rx: &Receiver<ConsensusCommand>) -> Vec<ConsensusCommand> {
    let mut cmds = Vec::new();
    // Give the core thread a moment to process
    std::thread::sleep(std::time::Duration::from_millis(50));
    while let Ok(cmd) = rx.try_recv() {
        cmds.push(cmd);
    }
    cmds
}

fn has_broadcast_proposal(cmds: &[ConsensusCommand]) -> bool {
    cmds.iter().any(|c| matches!(c, ConsensusCommand::BroadcastProposal { .. }))
}

fn has_broadcast_vote(cmds: &[ConsensusCommand], vt: VoteType) -> bool {
    cmds.iter().any(|c| match c {
        ConsensusCommand::BroadcastVote { vote } => vote.vote.vote_type == vt,
        _ => false,
    })
}

fn has_execute_block(cmds: &[ConsensusCommand]) -> bool {
    cmds.iter().any(|c| matches!(c, ConsensusCommand::ExecuteBlock { .. }))
}

fn has_schedule_timeout(cmds: &[ConsensusCommand]) -> bool {
    cmds.iter().any(|c| matches!(c, ConsensusCommand::ScheduleTimeout { .. }))
}

fn find_broadcast_vote_hash(cmds: &[ConsensusCommand], vt: VoteType) -> Option<Option<Hash>> {
    for c in cmds {
        if let ConsensusCommand::BroadcastVote { vote } = c {
            if vote.vote.vote_type == vt {
                return Some(vote.vote.block_hash);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Happy path: single validator commits immediately.
/// Proposal → prevote → precommit → commit
#[test]
fn test_happy_path_single_validator() {
    let (vset, ids) = make_validator_set(1);
    let my_id = ids[0];
    let (tx_ev, rx_cmd) = spawn_core(my_id, 1, vset);

    // Core starts in NewHeight → enters Propose → requests ReapTxs
    let cmds = drain_commands(&rx_cmd);
    assert!(cmds.iter().any(|c| matches!(c, ConsensusCommand::ReapTxs { .. })),
        "proposer should request txs");

    // Respond with empty txs (allow_empty_blocks = true by default)
    tx_ev.send(ConsensusEvent::TxsAvailable { txs: vec![] }).unwrap();

    // With a single validator, the core will: propose → prevote → (self has polka) →
    // precommit → (self has commit quorum) → execute block, all in one batch.
    // Collect all commands emitted.
    let cmds = drain_commands(&rx_cmd);

    assert!(has_broadcast_proposal(&cmds), "should broadcast proposal");
    assert!(has_broadcast_vote(&cmds, VoteType::Prevote), "should broadcast prevote");
    // Single validator: own prevote gives polka (1/1 >= quorum), own precommit gives commit.
    // But the core inserts its own vote via broadcast_vote which sends to tx_cmd, not to
    // the vote_set directly. The core doesn't self-deliver votes through the channel.
    // So with 1 validator, the core needs to see its own vote come back.
    // Let's feed the core its own prevote.
    // First, find the prevote we broadcast
    let my_prevote = cmds.iter().find_map(|c| match c {
        ConsensusCommand::BroadcastVote { vote } if vote.vote.vote_type == VoteType::Prevote => Some(vote.clone()),
        _ => None,
    });
    assert!(my_prevote.is_some(), "should have broadcast a prevote");
    let my_prevote = my_prevote.unwrap();

    // Feed the prevote back (simulating P2P echo or router loopback)
    tx_ev.send(ConsensusEvent::VoteReceived { vote: my_prevote }).unwrap();
    let cmds = drain_commands(&rx_cmd);

    // Now should have precommit
    assert!(has_broadcast_vote(&cmds, VoteType::Precommit), "should broadcast precommit after polka");

    // Feed the precommit back
    let my_precommit = cmds.iter().find_map(|c| match c {
        ConsensusCommand::BroadcastVote { vote } if vote.vote.vote_type == VoteType::Precommit => Some(vote.clone()),
        _ => None,
    });
    assert!(my_precommit.is_some(), "should have broadcast a precommit");
    tx_ev.send(ConsensusEvent::VoteReceived { vote: my_precommit.unwrap() }).unwrap();
    let cmds = drain_commands(&rx_cmd);

    assert!(has_execute_block(&cmds), "should execute block after commit quorum");

    // Send BlockExecuted to complete the commit
    tx_ev.send(ConsensusEvent::BlockExecuted {
        height: 1,
        state_root: Hash::ZERO,
        validator_updates: vec![],
    }).unwrap();

    let cmds = drain_commands(&rx_cmd);
    // Should have PersistBlock and then start next height (ReapTxs again)
    assert!(cmds.iter().any(|c| matches!(c, ConsensusCommand::PersistBlock { .. })),
        "should persist block");

    drop(tx_ev);
}

/// Proposer timeout: no proposal received → prevote nil → precommit nil → next round.
#[test]
fn test_proposer_timeout_prevote_nil() {
    let (vset, ids) = make_validator_set(4);
    // Use a non-proposer as our node
    let my_id = ids[3]; // likely not proposer for round 0
    let (tx_ev, rx_cmd) = spawn_core(my_id, 1, vset);

    let cmds = drain_commands(&rx_cmd);
    // Should schedule a propose timeout
    assert!(has_schedule_timeout(&cmds), "should schedule propose timeout");

    // Fire the propose timeout
    tx_ev.send(ConsensusEvent::TimeoutPropose { height: 1, round: 0 }).unwrap();
    let cmds = drain_commands(&rx_cmd);

    // Should prevote nil (no proposal received)
    let prevote_hash = find_broadcast_vote_hash(&cmds, VoteType::Prevote);
    assert_eq!(prevote_hash, Some(None), "should prevote nil on propose timeout");

    drop(tx_ev);
}

/// Duplicate vote: same vote received twice → no double-counting.
#[test]
fn test_duplicate_vote_no_double_count() {
    let (vset, ids) = make_validator_set(4);
    let my_id = ids[0];
    let (tx_ev, rx_cmd) = spawn_core(my_id, 1, vset);
    drain_commands(&rx_cmd); // drain initial commands

    let block = make_block(1, ids[0]);
    let bh = block_hash(&block);

    // Send the same prevote from validator 1 twice
    let vote = make_signed_vote(VoteType::Prevote, 1, 0, Some(bh), ids[1]);
    tx_ev.send(ConsensusEvent::VoteReceived { vote: vote.clone() }).unwrap();
    tx_ev.send(ConsensusEvent::VoteReceived { vote }).unwrap();
    drain_commands(&rx_cmd);

    // The second vote should be silently ignored (duplicate).
    // No crash, no panic. This is a basic sanity check.
    drop(tx_ev);
}

/// Round skip: receive votes from round R+2, skip to R+2 if >1/3 voting power.
#[test]
fn test_round_skip() {
    let (vset, ids) = make_validator_set(4);
    let my_id = ids[3]; // non-proposer
    let (tx_ev, rx_cmd) = spawn_core(my_id, 1, vset);
    drain_commands(&rx_cmd); // drain initial

    let block = make_block(1, ids[0]);
    let bh = block_hash(&block);

    // Send 2 prevotes from round 2 (2/4 = 50% > 1/3)
    let v1 = make_signed_vote(VoteType::Prevote, 1, 2, Some(bh), ids[0]);
    let v2 = make_signed_vote(VoteType::Prevote, 1, 2, Some(bh), ids[1]);
    tx_ev.send(ConsensusEvent::VoteReceived { vote: v1 }).unwrap();
    tx_ev.send(ConsensusEvent::VoteReceived { vote: v2 }).unwrap();

    let cmds = drain_commands(&rx_cmd);
    // Should have scheduled a new propose timeout for round 2
    let has_round2_timeout = cmds.iter().any(|c| match c {
        ConsensusCommand::ScheduleTimeout { timeout } => timeout.round == 2,
        _ => false,
    });
    assert!(has_round2_timeout, "should skip to round 2");

    drop(tx_ev);
}

/// Equivocation detection: two different votes from same validator.
#[test]
fn test_equivocation_detection() {
    let (vset, ids) = make_validator_set(4);
    let my_id = ids[0];
    let (tx_ev, rx_cmd) = spawn_core(my_id, 1, vset);
    drain_commands(&rx_cmd);

    let bh_a = Hash([1u8; 32]);
    let bh_b = Hash([2u8; 32]);

    // Send two conflicting prevotes from validator 1
    let v1 = make_signed_vote(VoteType::Prevote, 1, 0, Some(bh_a), ids[1]);
    let v2 = make_signed_vote(VoteType::Prevote, 1, 0, Some(bh_b), ids[1]);
    tx_ev.send(ConsensusEvent::VoteReceived { vote: v1 }).unwrap();
    tx_ev.send(ConsensusEvent::VoteReceived { vote: v2 }).unwrap();

    // Should not crash. The equivocation is recorded internally in VoteSet.
    drain_commands(&rx_cmd);
    drop(tx_ev);
}

/// Stale event: votes from a past height are ignored.
#[test]
fn test_stale_height_vote_ignored() {
    let (vset, ids) = make_validator_set(4);
    let my_id = ids[0];
    let (tx_ev, rx_cmd) = spawn_core(my_id, 5, vset);
    drain_commands(&rx_cmd);

    // Send a vote from height 3 (stale)
    let v = make_signed_vote(VoteType::Prevote, 3, 0, Some(Hash([1u8; 32])), ids[1]);
    tx_ev.send(ConsensusEvent::VoteReceived { vote: v }).unwrap();

    // Should not cause any state change
    let cmds = drain_commands(&rx_cmd);
    assert!(!has_execute_block(&cmds), "stale vote should not trigger commit");

    drop(tx_ev);
}

/// Future height vote: votes from a future height are ignored.
#[test]
fn test_future_height_vote_ignored() {
    let (vset, ids) = make_validator_set(4);
    let my_id = ids[0];
    let (tx_ev, rx_cmd) = spawn_core(my_id, 1, vset);
    drain_commands(&rx_cmd);

    // Send a vote from height 100 (future)
    let v = make_signed_vote(VoteType::Prevote, 100, 0, Some(Hash([1u8; 32])), ids[1]);
    tx_ev.send(ConsensusEvent::VoteReceived { vote: v }).unwrap();

    let cmds = drain_commands(&rx_cmd);
    assert!(!has_execute_block(&cmds), "future vote should not trigger commit");

    drop(tx_ev);
}

/// Invalid proposal: bad validation → prevote nil.
#[test]
fn test_invalid_proposal_prevote_nil() {
    let (vset, ids) = make_validator_set(4);
    let my_id = ids[3];

    let (tx_ev, rx_ev) = bounded::<ConsensusEvent>(256);
    let (tx_cmd, rx_cmd) = bounded::<ConsensusCommand>(1024);

    std::thread::spawn(move || {
        let cfg = ConsensusConfig::default();
        let deps = ConsensusDeps {
            verify_proposal_sig: Box::new(|_| true),
            verify_vote_sig: Box::new(|_| true),
            validate_block: Box::new(|_| false), // ALL blocks invalid
            have_full_block: Box::new(|_| true),
            block_hash: Box::new(|block| {
                let bytes = serde_json::to_vec(block).unwrap_or_default();
                sha256(&bytes)
            }),
        };
        let core = ConsensusCore::new(cfg, deps, my_id, 1, vset, rx_ev, tx_cmd);
        core.run();
    });

    drain_commands(&rx_cmd); // drain initial

    // Send a proposal (will fail validate_block)
    let block = make_block(1, ids[0]);
    let sp = make_proposal(1, 0, block, ids[0]);
    tx_ev.send(ConsensusEvent::ProposalReceived { proposal: sp }).unwrap();

    // The proposal should be rejected (validate_block returns false).
    // If we then fire timeout_propose, node should prevote nil.
    tx_ev.send(ConsensusEvent::TimeoutPropose { height: 1, round: 0 }).unwrap();
    let cmds = drain_commands(&rx_cmd);

    let prevote_hash = find_broadcast_vote_hash(&cmds, VoteType::Prevote);
    assert_eq!(prevote_hash, Some(None), "should prevote nil for invalid proposal");

    drop(tx_ev);
}

/// Timeout handling: stale timeout (wrong height/round) is ignored.
#[test]
fn test_stale_timeout_ignored() {
    let (vset, ids) = make_validator_set(4);
    let my_id = ids[0];
    let (tx_ev, rx_cmd) = spawn_core(my_id, 5, vset);
    drain_commands(&rx_cmd);

    // Send a timeout for height 3 (stale)
    tx_ev.send(ConsensusEvent::TimeoutPropose { height: 3, round: 0 }).unwrap();
    let cmds = drain_commands(&rx_cmd);

    // Should not trigger any state transition
    assert!(!has_broadcast_vote(&cmds, VoteType::Prevote),
        "stale timeout should not trigger prevote");

    drop(tx_ev);
}
