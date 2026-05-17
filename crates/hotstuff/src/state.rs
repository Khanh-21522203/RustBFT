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
    pub max_reap_bytes: usize,
}

impl Default for HotStuffConfig {
    fn default() -> Self {
        Self {
            view_timeout_ms: 1000,
            timeout_delta_ms: 500,
            max_reap_bytes: 512 * 1024,
        }
    }
}

pub struct HotStuffDeps {
    pub verify_proposal_sig: Box<dyn Fn(&SignedHotStuffProposal) -> bool + Send>,
    pub verify_vote_sig: Box<dyn Fn(&SignedHotStuffVote) -> bool + Send>,
    pub verify_timeout_sig: Box<dyn Fn(&SignedHotStuffTimeout) -> bool + Send>,
    pub validate_block: Box<dyn Fn(&Block) -> bool + Send>,
    pub block_hash: Box<dyn Fn(&Block) -> Hash + Send>,
    pub sign_proposal: Box<dyn Fn(HotStuffProposal) -> SignedHotStuffProposal + Send>,
    pub sign_vote: Box<dyn Fn(HotStuffVote) -> SignedHotStuffVote + Send>,
    pub sign_timeout: Box<dyn Fn(HotStuffTimeout) -> SignedHotStuffTimeout + Send>,
}

#[derive(Clone, Debug)]
pub struct HotStuffState {
    pub view: ViewNumber,
    pub locked_qc: QuorumCertificate,
    pub high_qc: QuorumCertificate,
    pub last_voted_view: Option<ViewNumber>,
    pub last_executed: Option<Hash>,
}

pub struct HotStuffCore {
    cfg: HotStuffConfig,
    deps: HotStuffDeps,
    my_id: ValidatorId,
    validator_set: ValidatorSet,
    state: HotStuffState,
    vote_accumulators: BTreeMap<(ViewNumber, Phase, Hash), VoteAccumulator>,
    timeout_accumulators: BTreeMap<ViewNumber, BTreeMap<ValidatorId, SignedHotStuffTimeout>>,
    blocks: BTreeMap<Hash, BlockNode>,
}

#[derive(Clone, Debug)]
struct BlockNode {
    block: Block,
    parent: Hash,
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
                last_executed: None,
            },
            vote_accumulators: BTreeMap::new(),
            timeout_accumulators: BTreeMap::new(),
            blocks: BTreeMap::new(),
        }
    }

    pub fn state(&self) -> &HotStuffState {
        &self.state
    }

    pub fn process_event(&mut self, event: HotStuffEvent) -> Vec<HotStuffCommand> {
        match event {
            HotStuffEvent::BlockReady { block } => self.handle_block_ready(block),
            HotStuffEvent::ProposalReceived { proposal } => self.handle_proposal(proposal),
            HotStuffEvent::VoteReceived { vote } => self.handle_vote(vote),
            HotStuffEvent::TimeoutReceived { timeout } => self.handle_timeout(timeout),
            HotStuffEvent::TimeoutCertificateReceived { certificate } => {
                if self.validate_timeout_certificate(&certificate)
                    && certificate.view >= self.state.view
                {
                    self.state.view = certificate.view.saturating_add(1);
                    self.update_high_qc(certificate.high_qc);
                    let mut commands = self.schedule_timeout();
                    commands.extend(self.maybe_request_block());
                    commands
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

    fn handle_block_ready(&mut self, block: Block) -> Vec<HotStuffCommand> {
        if self.leader_for(self.state.view) != self.my_id {
            return Vec::new();
        }

        let parent_block_hash = block.header.prev_block_hash;
        let proposal = HotStuffProposal {
            view: self.state.view,
            phase: Phase::Prepare,
            block,
            parent_block_hash,
            justify_qc: self.state.high_qc.clone(),
            proposer: self.my_id,
        };
        vec![HotStuffCommand::BroadcastProposal {
            proposal: (self.deps.sign_proposal)(proposal),
        }]
    }

    fn handle_proposal(&mut self, signed: SignedHotStuffProposal) -> Vec<HotStuffCommand> {
        if !(self.deps.verify_proposal_sig)(&signed) {
            return Vec::new();
        }

        let proposal = &signed.proposal;
        if proposal.view < self.state.view {
            return Vec::new();
        }
        if proposal.proposer != self.leader_for(proposal.view) {
            return Vec::new();
        }
        if !self.validator_set.contains(&proposal.proposer) {
            return Vec::new();
        }
        if !(self.deps.validate_block)(&proposal.block) {
            return Vec::new();
        }
        if !self.validate_qc(&proposal.justify_qc) {
            return Vec::new();
        }
        self.update_high_qc(proposal.justify_qc.clone());
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
        self.blocks.insert(
            block_hash,
            BlockNode {
                block: proposal.block.clone(),
                parent: proposal.parent_block_hash,
            },
        );
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
            let mut commands = self.advance_after_qc(qc);
            commands.extend(self.schedule_timeout());
            commands
        } else {
            Vec::new()
        }
    }

    fn advance_after_qc(&mut self, qc: QuorumCertificate) -> Vec<HotStuffCommand> {
        let mut commands = Vec::new();
        if qc.phase == Phase::Decide {
            if self.state.last_executed != Some(qc.block_hash) {
                if let Some(node) = self.blocks.get(&qc.block_hash) {
                    self.state.last_executed = Some(qc.block_hash);
                    commands.push(HotStuffCommand::ExecuteBlock {
                        block: node.block.clone(),
                    });
                }
            }
            self.state.view = self.state.view.max(qc.view.saturating_add(1));
            commands.extend(self.maybe_request_block());
            return commands;
        }

        self.state.view = self.state.view.max(qc.view.saturating_add(1));

        if self.leader_for(self.state.view) == self.my_id {
            if let Some(phase) = next_phase(qc.phase) {
                if let Some(node) = self.blocks.get(&qc.block_hash) {
                    let proposal = HotStuffProposal {
                        view: self.state.view,
                        phase,
                        block: node.block.clone(),
                        parent_block_hash: node.parent,
                        justify_qc: qc,
                        proposer: self.my_id,
                    };
                    commands.push(HotStuffCommand::BroadcastProposal {
                        proposal: (self.deps.sign_proposal)(proposal),
                    });
                }
            } else {
                commands.extend(self.maybe_request_block());
            }
        }

        commands
    }

    fn handle_timeout(&mut self, signed: SignedHotStuffTimeout) -> Vec<HotStuffCommand> {
        if !(self.deps.verify_timeout_sig)(&signed) {
            return Vec::new();
        }
        if !self.validator_set.contains(&signed.timeout.validator) {
            return Vec::new();
        }
        if self.validate_qc(&signed.timeout.high_qc) {
            self.update_high_qc(signed.timeout.high_qc.clone());
        }

        let timeout_view = signed.timeout.view;
        let validator = signed.timeout.validator;
        let acc = self.timeout_accumulators.entry(timeout_view).or_default();
        acc.entry(validator).or_insert(signed);

        let power = acc
            .keys()
            .map(|id| self.validator_set.voting_power(id))
            .fold(0u64, u64::saturating_add);
        if power < QuorumCertificate::quorum_threshold(self.validator_set.total_power()) {
            return Vec::new();
        }

        let high_qc = acc
            .values()
            .map(|t| t.timeout.high_qc.clone())
            .max_by_key(|qc| qc.view)
            .unwrap_or_else(|| self.state.high_qc.clone());
        let timeouts = acc.values().cloned().collect();
        self.update_high_qc(high_qc.clone());
        self.state.view = self.state.view.max(timeout_view.saturating_add(1));

        let certificate = crate::types::TimeoutCertificate {
            view: timeout_view,
            high_qc,
            timeouts,
        };
        let mut commands = vec![HotStuffCommand::BroadcastTimeoutCertificate { certificate }];
        commands.extend(self.schedule_timeout());
        commands.extend(self.maybe_request_block());
        commands
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

    fn maybe_request_block(&self) -> Vec<HotStuffCommand> {
        if self.leader_for(self.state.view) == self.my_id {
            vec![HotStuffCommand::ReapTxs {
                max_bytes: self.cfg.max_reap_bytes,
            }]
        } else {
            Vec::new()
        }
    }

    fn leader_for(&self, view: ViewNumber) -> ValidatorId {
        let ids: Vec<_> = self.validator_set.ids_in_order().copied().collect();
        ids[(view as usize) % ids.len()]
    }

    fn validate_timeout_certificate(&self, certificate: &crate::types::TimeoutCertificate) -> bool {
        let mut seen = std::collections::BTreeSet::new();
        let mut power = 0u64;
        for signed in &certificate.timeouts {
            let timeout = &signed.timeout;
            if timeout.view != certificate.view {
                return false;
            }
            if !(self.deps.verify_timeout_sig)(signed) {
                return false;
            }
            if !self.validator_set.contains(&timeout.validator) {
                return false;
            }
            if !self.validate_qc(&timeout.high_qc) {
                return false;
            }
            if seen.insert(timeout.validator) {
                power = power.saturating_add(self.validator_set.voting_power(&timeout.validator));
            }
        }
        power >= QuorumCertificate::quorum_threshold(self.validator_set.total_power())
    }

    fn validate_qc(&self, qc: &QuorumCertificate) -> bool {
        qc.validate(&self.validator_set).is_ok()
            && qc
                .signatures
                .iter()
                .all(|vote| (self.deps.verify_vote_sig)(vote))
    }
}

fn next_phase(phase: Phase) -> Option<Phase> {
    match phase {
        Phase::Prepare => Some(Phase::PreCommit),
        Phase::PreCommit => Some(Phase::Commit),
        Phase::Commit => Some(Phase::Decide),
        Phase::Decide => None,
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
            sign_proposal: Box::new(|proposal| SignedHotStuffProposal {
                proposal,
                signature: [0u8; 64],
            }),
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

    fn proposal(
        view: ViewNumber,
        phase: Phase,
        block: Block,
        proposer: ValidatorId,
        justify_qc: QuorumCertificate,
    ) -> SignedHotStuffProposal {
        SignedHotStuffProposal {
            proposal: HotStuffProposal {
                view,
                phase,
                parent_block_hash: block.header.prev_block_hash,
                block,
                justify_qc,
                proposer,
            },
            signature: [0u8; 64],
        }
    }

    fn signed_vote(
        view: ViewNumber,
        phase: Phase,
        block_hash: Hash,
        validator: ValidatorId,
    ) -> SignedHotStuffVote {
        SignedHotStuffVote {
            vote: HotStuffVote {
                view,
                phase,
                block_hash,
                validator,
            },
            signature: [0u8; 64],
        }
    }

    #[test]
    fn proposal_produces_vote_when_safe() {
        let (vset, ids) = validator_set(4);
        let my_id = ids[0];
        let b = block(ids[1]);
        let bh = sha256(&serde_json_bytes(&b));
        let mut core = HotStuffCore::new(HotStuffConfig::default(), deps(), my_id, vset, 1);

        let proposal = proposal(1, Phase::Prepare, b, ids[1], QuorumCertificate::genesis());

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

    #[test]
    fn proposal_from_non_leader_is_rejected() {
        let (vset, ids) = validator_set(4);
        let my_id = ids[0];
        let b = block(ids[0]);
        let mut core = HotStuffCore::new(HotStuffConfig::default(), deps(), my_id, vset, 1);

        // View 1 leader is ids[1], so ids[0] must not be accepted as proposer.
        let commands = core.process_event(HotStuffEvent::ProposalReceived {
            proposal: proposal(1, Phase::Prepare, b, ids[0], QuorumCertificate::genesis()),
        });

        assert!(commands.is_empty());
    }

    #[test]
    fn prepare_qc_advances_view_and_next_leader_broadcasts_precommit_proposal() {
        let (vset, ids) = validator_set(4);
        let my_id = ids[2]; // view 2 leader
        let b = block(ids[1]);
        let bh = sha256(&serde_json_bytes(&b));
        let mut core = HotStuffCore::new(HotStuffConfig::default(), deps(), my_id, vset, 1);

        core.process_event(HotStuffEvent::ProposalReceived {
            proposal: proposal(1, Phase::Prepare, b, ids[1], QuorumCertificate::genesis()),
        });

        let mut saw_proposal = false;
        for id in ids.iter().take(3) {
            let commands = core.process_event(HotStuffEvent::VoteReceived {
                vote: signed_vote(1, Phase::Prepare, bh, *id),
            });
            saw_proposal |= commands.iter().any(|cmd| match cmd {
                HotStuffCommand::BroadcastProposal { proposal } => {
                    proposal.proposal.view == 2 && proposal.proposal.phase == Phase::PreCommit
                }
                _ => false,
            });
        }

        assert!(saw_proposal);
        assert_eq!(core.state().high_qc.view, 1);
        assert_eq!(core.state().view, 2);
    }

    #[test]
    fn decide_qc_executes_known_block_once() {
        let (vset, ids) = validator_set(4);
        let my_id = ids[0];
        let b = block(ids[1]);
        let bh = sha256(&serde_json_bytes(&b));
        let mut core = HotStuffCore::new(HotStuffConfig::default(), deps(), my_id, vset, 1);

        core.process_event(HotStuffEvent::ProposalReceived {
            proposal: proposal(1, Phase::Prepare, b, ids[1], QuorumCertificate::genesis()),
        });

        let mut execute_count = 0;
        for id in ids.iter().take(3) {
            let commands = core.process_event(HotStuffEvent::VoteReceived {
                vote: signed_vote(4, Phase::Decide, bh, *id),
            });
            execute_count += commands
                .iter()
                .filter(|cmd| matches!(cmd, HotStuffCommand::ExecuteBlock { .. }))
                .count();
        }

        let duplicate_commands = core.process_event(HotStuffEvent::VoteReceived {
            vote: signed_vote(4, Phase::Decide, bh, ids[0]),
        });

        assert_eq!(execute_count, 1);
        assert!(
            !duplicate_commands
                .iter()
                .any(|cmd| matches!(cmd, HotStuffCommand::ExecuteBlock { .. }))
        );
    }

    #[test]
    fn timeout_quorum_builds_timeout_certificate() {
        let (vset, ids) = validator_set(4);
        let my_id = ids[2]; // view 2 leader after timeout from view 1
        let mut core = HotStuffCore::new(HotStuffConfig::default(), deps(), my_id, vset, 1);

        let mut saw_tc = false;
        for id in ids.iter().take(3) {
            let commands = core.process_event(HotStuffEvent::TimeoutReceived {
                timeout: SignedHotStuffTimeout {
                    timeout: HotStuffTimeout {
                        view: 1,
                        validator: *id,
                        high_qc: QuorumCertificate::genesis(),
                    },
                    signature: [0u8; 64],
                },
            });
            saw_tc |= commands.iter().any(|cmd| {
                matches!(cmd, HotStuffCommand::BroadcastTimeoutCertificate { certificate } if certificate.view == 1)
            });
        }

        assert!(saw_tc);
        assert_eq!(core.state().view, 2);
    }
}
