use std::collections::BTreeMap;

use rustbft_core::{Block, Hash, ValidatorId, ValidatorSet};

use crate::events::{HotStuffCommand, HotStuffEvent};
use crate::qc::{QuorumCertificate, QuorumError, VoteAccumulator};
use crate::types::{
    HotStuffProposal, HotStuffTimeout, HotStuffVote, Phase, SignedHotStuffProposal,
    SignedHotStuffTimeout, SignedHotStuffVote, ViewNumber,
};

#[derive(Clone, Debug)]
pub struct HotStuffConfig {
    pub view_timeout_ms: u64,
    pub timeout_delta_ms: u64,
}

impl Default for HotStuffConfig {
    fn default() -> Self {
        Self {
            view_timeout_ms: 1000,
            timeout_delta_ms: 500,
        }
    }
}

pub struct HotStuffDeps {
    pub verify_proposal_sig: Box<dyn Fn(&SignedHotStuffProposal) -> bool + Send>,
    pub verify_vote_sig: Box<dyn Fn(&SignedHotStuffVote) -> bool + Send>,
    pub verify_timeout_sig: Box<dyn Fn(&SignedHotStuffTimeout) -> bool + Send>,
    pub validate_block: Box<dyn Fn(&Block) -> bool + Send>,
    pub block_hash: Box<dyn Fn(&Block) -> Hash + Send>,
    pub sign_vote: Box<dyn Fn(HotStuffVote) -> SignedHotStuffVote + Send>,
    pub sign_timeout: Box<dyn Fn(HotStuffTimeout) -> SignedHotStuffTimeout + Send>,
}

#[derive(Clone, Debug)]
pub struct HotStuffState {
    pub view: ViewNumber,
    pub locked_qc: QuorumCertificate,
    pub high_qc: QuorumCertificate,
    pub last_voted_view: Option<ViewNumber>,
}

pub struct HotStuffCore {
    cfg: HotStuffConfig,
    deps: HotStuffDeps,
    my_id: ValidatorId,
    validator_set: ValidatorSet,
    state: HotStuffState,
    vote_accumulators: BTreeMap<(ViewNumber, Phase, Hash), VoteAccumulator>,
}

impl HotStuffCore {
    pub fn new(
        cfg: HotStuffConfig,
        deps: HotStuffDeps,
        my_id: ValidatorId,
        validator_set: ValidatorSet,
        start_view: ViewNumber,
    ) -> Self {
        let genesis_qc = QuorumCertificate::genesis();
        Self {
            cfg,
            deps,
            my_id,
            validator_set,
            state: HotStuffState {
                view: start_view,
                locked_qc: genesis_qc.clone(),
                high_qc: genesis_qc,
                last_voted_view: None,
            },
            vote_accumulators: BTreeMap::new(),
        }
    }

    pub fn state(&self) -> &HotStuffState {
        &self.state
    }

    pub fn process_event(&mut self, event: HotStuffEvent) -> Vec<HotStuffCommand> {
        match event {
            HotStuffEvent::ProposalReceived { proposal } => self.handle_proposal(proposal),
            HotStuffEvent::VoteReceived { vote } => self.handle_vote(vote),
            HotStuffEvent::TimeoutReceived { timeout } => self.handle_timeout(timeout),
            HotStuffEvent::TimeoutCertificateReceived { certificate } => {
                if certificate.view >= self.state.view {
                    self.state.view = certificate.view.saturating_add(1);
                    self.update_high_qc(certificate.high_qc);
                    self.schedule_timeout()
                } else {
                    Vec::new()
                }
            }
            HotStuffEvent::ViewTimeout { view } => {
                if view == self.state.view {
                    let timeout = (self.deps.sign_timeout)(HotStuffTimeout {
                        view,
                        validator: self.my_id,
                        high_qc: self.state.high_qc.clone(),
                    });
                    vec![HotStuffCommand::BroadcastTimeout { timeout }]
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn handle_proposal(&mut self, signed: SignedHotStuffProposal) -> Vec<HotStuffCommand> {
        if !(self.deps.verify_proposal_sig)(&signed) {
            return Vec::new();
        }

        let proposal = &signed.proposal;
        if proposal.view < self.state.view {
            return Vec::new();
        }
        if !self.validator_set.contains(&proposal.proposer) {
            return Vec::new();
        }
        if !(self.deps.validate_block)(&proposal.block) {
            return Vec::new();
        }
        if proposal.justify_qc.has_quorum(&self.validator_set) {
            self.update_high_qc(proposal.justify_qc.clone());
        }
        if !self.safe_to_vote(proposal) {
            return Vec::new();
        }
        if self
            .state
            .last_voted_view
            .is_some_and(|v| v >= proposal.view)
        {
            return Vec::new();
        }

        self.state.view = proposal.view;
        self.state.last_voted_view = Some(proposal.view);

        let block_hash = (self.deps.block_hash)(&proposal.block);
        let vote = (self.deps.sign_vote)(HotStuffVote {
            view: proposal.view,
            phase: proposal.phase,
            block_hash,
            validator: self.my_id,
        });

        vec![HotStuffCommand::BroadcastVote { vote }]
    }

    fn handle_vote(&mut self, signed: SignedHotStuffVote) -> Vec<HotStuffCommand> {
        if !(self.deps.verify_vote_sig)(&signed) {
            return Vec::new();
        }

        let vote = &signed.vote;
        let key = (vote.view, vote.phase, vote.block_hash);
        let acc = self
            .vote_accumulators
            .entry(key)
            .or_insert_with(|| VoteAccumulator::new(vote.view, vote.phase, vote.block_hash));

        match acc.insert(&self.validator_set, signed) {
            Ok(()) | Err(QuorumError::Duplicate) => {}
            Err(_) => return Vec::new(),
        }

        if let Ok(qc) = acc.build_qc(&self.validator_set) {
            self.update_high_qc(qc.clone());
            if qc.phase == Phase::Commit {
                // Later wiring should resolve block_hash to a block and execute it.
                return Vec::new();
            }
        }

        Vec::new()
    }

    fn handle_timeout(&mut self, signed: SignedHotStuffTimeout) -> Vec<HotStuffCommand> {
        if !(self.deps.verify_timeout_sig)(&signed) {
            return Vec::new();
        }
        if !self.validator_set.contains(&signed.timeout.validator) {
            return Vec::new();
        }
        if signed.timeout.high_qc.has_quorum(&self.validator_set) {
            self.update_high_qc(signed.timeout.high_qc.clone());
        }
        Vec::new()
    }

    fn safe_to_vote(&self, proposal: &HotStuffProposal) -> bool {
        proposal.justify_qc.view >= self.state.locked_qc.view
            || proposal.parent_block_hash == self.state.locked_qc.block_hash
    }

    fn update_high_qc(&mut self, qc: QuorumCertificate) {
        if qc.view > self.state.high_qc.view {
            if qc.phase == Phase::PreCommit
                || qc.phase == Phase::Commit
                || qc.phase == Phase::Decide
            {
                self.state.locked_qc = qc.clone();
            }
            self.state.high_qc = qc;
        }
    }

    fn schedule_timeout(&self) -> Vec<HotStuffCommand> {
        let duration_ms = self
            .cfg
            .view_timeout_ms
            .saturating_add(self.state.view.saturating_mul(self.cfg.timeout_delta_ms));
        vec![HotStuffCommand::ScheduleViewTimeout {
            view: self.state.view,
            duration_ms,
        }]
    }
}

#[cfg(test)]
mod tests {
    use rustbft_core::{BlockHeader, Validator, crypto::sha256};

    use super::*;

    fn validator_id(seed: u8) -> ValidatorId {
        let mut id = [0u8; 32];
        id[0] = seed;
        ValidatorId(id)
    }

    fn validator_set(n: u8) -> (ValidatorSet, Vec<ValidatorId>) {
        let ids: Vec<_> = (1..=n).map(validator_id).collect();
        let validators = ids
            .iter()
            .map(|id| Validator {
                id: *id,
                voting_power: 1,
            })
            .collect();
        (ValidatorSet::new(validators, sha256(&[n])), ids)
    }

    fn block(proposer: ValidatorId) -> Block {
        Block {
            header: BlockHeader {
                height: 1,
                timestamp_ms: 0,
                prev_block_hash: Hash::ZERO,
                proposer,
                validator_set_hash: Hash::ZERO,
                state_root: Hash::ZERO,
                tx_merkle_root: Hash::ZERO,
            },
            txs: Vec::new(),
            last_commit: None,
        }
    }

    fn deps() -> HotStuffDeps {
        HotStuffDeps {
            verify_proposal_sig: Box::new(|_| true),
            verify_vote_sig: Box::new(|_| true),
            verify_timeout_sig: Box::new(|_| true),
            validate_block: Box::new(|_| true),
            block_hash: Box::new(|block| sha256(&serde_json_bytes(block))),
            sign_vote: Box::new(|vote| SignedHotStuffVote {
                vote,
                signature: [0u8; 64],
            }),
            sign_timeout: Box::new(|timeout| SignedHotStuffTimeout {
                timeout,
                signature: [0u8; 64],
            }),
        }
    }

    fn serde_json_bytes(block: &Block) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&block.header.height.to_be_bytes());
        out.extend_from_slice(&block.header.proposer.0);
        out
    }

    #[test]
    fn proposal_produces_vote_when_safe() {
        let (vset, ids) = validator_set(4);
        let my_id = ids[0];
        let b = block(ids[1]);
        let bh = sha256(&serde_json_bytes(&b));
        let mut core = HotStuffCore::new(HotStuffConfig::default(), deps(), my_id, vset, 1);

        let proposal = SignedHotStuffProposal {
            proposal: HotStuffProposal {
                view: 1,
                phase: Phase::Prepare,
                block: b,
                parent_block_hash: Hash::ZERO,
                justify_qc: QuorumCertificate::genesis(),
                proposer: ids[1],
            },
            signature: [0u8; 64],
        };

        let commands = core.process_event(HotStuffEvent::ProposalReceived { proposal });
        assert_eq!(commands.len(), 1);
        match &commands[0] {
            HotStuffCommand::BroadcastVote { vote } => {
                assert_eq!(vote.vote.view, 1);
                assert_eq!(vote.vote.phase, Phase::Prepare);
                assert_eq!(vote.vote.block_hash, bh);
                assert_eq!(vote.vote.validator, my_id);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn vote_accumulator_builds_qc_at_two_thirds_plus_one() {
        let (vset, ids) = validator_set(4);
        let block_hash = Hash([9u8; 32]);
        let mut acc = VoteAccumulator::new(2, Phase::Prepare, block_hash);

        for id in ids.iter().take(3) {
            acc.insert(
                &vset,
                SignedHotStuffVote {
                    vote: HotStuffVote {
                        view: 2,
                        phase: Phase::Prepare,
                        block_hash,
                        validator: *id,
                    },
                    signature: [0u8; 64],
                },
            )
            .unwrap();
        }

        let qc = acc.build_qc(&vset).unwrap();
        assert!(qc.has_quorum(&vset));
        assert_eq!(qc.signatures.len(), 3);
    }
}
