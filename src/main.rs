use crossbeam_channel::bounded;
use RustBFT::consensus::{ConsensusConfig, ConsensusCore, ConsensusDeps, ConsensusEvent};
use RustBFT::types::{Hash, Validator, ValidatorId, ValidatorSet};

fn main() {
    let (tx_ev, rx_ev) = bounded::<ConsensusEvent>(1024);
    let (tx_cmd, rx_cmd) = bounded(1024);

    // dummy validator set (4 validators)
    let v1 = ValidatorId([1u8; 32]);
    let v2 = ValidatorId([2u8; 32]);
    let v3 = ValidatorId([3u8; 32]);
    let v4 = ValidatorId([4u8; 32]);

    let vset = ValidatorSet::new(
        vec![
            Validator { id: v1, voting_power: 1 },
            Validator { id: v2, voting_power: 1 },
            Validator { id: v3, voting_power: 1 },
            Validator { id: v4, voting_power: 1 },
        ],
        Hash([9u8; 32]),
    );

    let deps = ConsensusDeps {
        verify_proposal_sig: Box::new(|_p| true),
        verify_vote_sig: Box::new(|_v| true),
        validate_block: Box::new(|_b| true),
        have_full_block: Box::new(|_h| true),
        block_hash: Box::new(|b| {
            // placeholder deterministic hash: hash(height)
            let mut out = [0u8; 32];
            out[0..8].copy_from_slice(&b.header.height.to_le_bytes());
            Hash(out)
        }),
    };

    let core = ConsensusCore::new(
        ConsensusConfig::default(),
        deps,
        v1,      // pretend we're validator 1
        1,       // start height=1
        vset,
        rx_ev,
        tx_cmd,
    );

    std::thread::spawn(move || core.run());

    // Print commands emitted by consensus (demo)
    loop {
        match rx_cmd.recv() {
            Ok(cmd) => println!("CMD: {:?}", cmd),
            Err(_) => break,
        }
    }

    drop(tx_ev);
}
