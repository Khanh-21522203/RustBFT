use crate::consensus::events::{ConsensusCommand, ConsensusEvent, Timeout, TimeoutKind};
use crate::consensus::proposer::{select_proposer, selection_steps};
use crate::consensus::vote_set::VoteSet;
use crate::types::{
    Block, Hash, SignedProposal, SignedVote, ValidatorId, ValidatorSet, Vote, VoteType,
};
use crossbeam_channel::{Receiver, Sender};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Propose,
    Prevote,
    Precommit,
    Commit,
}

#[derive(Clone, Debug)]
pub struct ConsensusConfig {
    pub base_timeout_propose_ms: u64,
    pub base_timeout_prevote_ms: u64,
    pub base_timeout_precommit_ms: u64,
    pub timeout_delta_ms: u64,

    pub allow_empty_blocks: bool,
    pub max_reap_bytes: usize,
}

impl Default for ConsensusConfig {
    fn default() -> Self {
        Self {
            base_timeout_propose_ms: 1000,
            base_timeout_prevote_ms: 1000,
            base_timeout_precommit_ms: 1000,
            timeout_delta_ms: 500,
            allow_empty_blocks: true,
            max_reap_bytes: 512 * 1024,
        }
    }
}

/// Consensus must stay isolated from state/storage/contracts.
/// So we inject minimal deterministic checks as callbacks.
pub struct ConsensusDeps {
    /// Verify signatures of proposal/vote (P2P may do this already, but consensus can double-check).
    pub verify_proposal_sig: Box<dyn Fn(&SignedProposal) -> bool + Send>,
    pub verify_vote_sig: Box<dyn Fn(&SignedVote) -> bool + Send>,

    /// Deterministic block validation at consensus layer (lightweight).
    pub validate_block: Box<dyn Fn(&Block) -> bool + Send>,

    /// Check if we have full block data for a hash (used before locking/precommit).
    pub have_full_block: Box<dyn Fn(Hash) -> bool + Send>,

    /// Deterministic hash for a block (placeholder; later tie to canonical serialization).
    pub block_hash: Box<dyn Fn(&Block) -> Hash + Send>,
}

pub struct ConsensusCore {
    cfg: ConsensusConfig,
    deps: ConsensusDeps,

    // current height/round/step
    height: u64,
    round: u32,
    step: Step,

    // locking/valid
    locked_round: Option<u32>,
    locked_block: Option<Hash>,
    valid_round: Option<u32>,
    valid_block: Option<Hash>,

    // current proposal
    proposal: Option<SignedProposal>,

    // votes for this height
    votes: VoteSet,

    // fixed validator set for this height
    validator_set: ValidatorSet,

    // node identity
    my_id: ValidatorId,

    // channels
    rx: Receiver<ConsensusEvent>,
    tx_cmd: Sender<ConsensusCommand>,

    // if we are proposer and asked mempool for txs
    awaiting_txs: bool,
}

impl ConsensusCore {
    pub fn new(
        cfg: ConsensusConfig,
        deps: ConsensusDeps,
        my_id: ValidatorId,
        height: u64,
        validator_set: ValidatorSet,
        rx: Receiver<ConsensusEvent>,
        tx_cmd: Sender<ConsensusCommand>,
    ) -> Self {
        Self {
            cfg,
            deps,
            height,
            round: 0,
            step: Step::Propose,
            locked_round: None,
            locked_block: None,
            valid_round: None,
            valid_block: None,
            proposal: None,
            votes: VoteSet::new(height),
            validator_set,
            my_id,
            rx,
            tx_cmd,
            awaiting_txs: false,
        }
    }

    pub fn run(mut self) {
        // Enter first height/round
        self.enter_propose();

        loop {
            let ev = match self.rx.recv() {
                Ok(ev) => ev,
                Err(_) => break, // channel closed => shutdown
            };

            match ev {
                ConsensusEvent::ProposalReceived { proposal } => self.handle_proposal(proposal),
                ConsensusEvent::VoteReceived { vote } => self.handle_vote(vote),

                ConsensusEvent::TimeoutPropose { height, round } => {
                    self.handle_timeout_propose(height, round)
                }
                ConsensusEvent::TimeoutPrevote { height, round } => {
                    self.handle_timeout_prevote(height, round)
                }
                ConsensusEvent::TimeoutPrecommit { height, round } => {
                    self.handle_timeout_precommit(height, round)
                }

                ConsensusEvent::TxsAvailable { txs } => self.handle_txs_available(txs),

                ConsensusEvent::BlockExecuted { height, state_root } => {
                    self.handle_block_executed(height, state_root)
                }
            }
        }
    }

    fn is_current(&self, h: u64, r: u32) -> bool {
        self.height == h && self.round == r
    }

    fn proposer_for(&self, h: u64, r: u32) -> ValidatorId {
        let steps = selection_steps(h, r);
        select_proposer(&self.validator_set, steps)
    }

    fn enter_propose(&mut self) {
        self.step = Step::Propose;
        self.proposal = None;
        self.awaiting_txs = false;

        let proposer = self.proposer_for(self.height, self.round);
        if proposer == self.my_id {
            // If valid_block is set -> re-propose it with valid_round
            if let Some(vb) = self.valid_block {
                // We don't store full block by hash here; outer shell should.
                // For MVP skeleton, we just request txs to rebuild, or outer shell can inject full block later.
                // We'll request txs; when you add block cache, you can re-propose exact block.
                self.awaiting_txs = true;
                self.tx_cmd
                    .send(ConsensusCommand::ReapTxs {
                        max_bytes: self.cfg.max_reap_bytes,
                    })
                    .ok();
                // NOTE: We'll set valid_round in proposal once we actually build.
                let _ = vb;
            } else {
                self.awaiting_txs = true;
                self.tx_cmd
                    .send(ConsensusCommand::ReapTxs {
                        max_bytes: self.cfg.max_reap_bytes,
                    })
                    .ok();
            }
        }

        self.schedule_timeout(TimeoutKind::Propose);
    }

    fn enter_prevote(&mut self) {
        self.step = Step::Prevote;

        let vote_hash = self.compute_prevote();
        self.broadcast_vote(VoteType::Prevote, vote_hash);

        self.schedule_timeout(TimeoutKind::Prevote);
    }

    fn enter_precommit(&mut self) {
        self.step = Step::Precommit;

        let vote_hash = self.compute_precommit();
        self.broadcast_vote(VoteType::Precommit, vote_hash);

        self.schedule_timeout(TimeoutKind::Precommit);
    }

    fn schedule_timeout(&mut self, kind: TimeoutKind) {
        let base = match kind {
            TimeoutKind::Propose => self.cfg.base_timeout_propose_ms,
            TimeoutKind::Prevote => self.cfg.base_timeout_prevote_ms,
            TimeoutKind::Precommit => self.cfg.base_timeout_precommit_ms,
        };
        let duration_ms = base.saturating_add(self.round as u64 * self.cfg.timeout_delta_ms);

        let t = Timeout {
            height: self.height,
            round: self.round,
            kind,
            duration_ms,
        };
        self.tx_cmd
            .send(ConsensusCommand::ScheduleTimeout { timeout: t })
            .ok();
    }

    fn handle_proposal(&mut self, sp: SignedProposal) {
        if !(self.deps.verify_proposal_sig)(&sp) {
            return;
        }

        let p = &sp.proposal;
        if p.height != self.height || p.round != self.round {
            return;
        }

        // Only accept first proposal for round (can extend with equivocation evidence for proposers later)
        if self.proposal.is_some() {
            return;
        }

        // Basic block validity check (consensus-level)
        if !(self.deps.validate_block)(&p.block) {
            // invalid proposal -> ignore, timeout will drive nil prevote
            return;
        }

        self.proposal = Some(sp);

        // On receiving proposal while in Propose step, enter Prevote immediately.
        if self.step == Step::Propose {
            self.enter_prevote();
        }
    }

    fn handle_vote(&mut self, sv: SignedVote) {
        if !(self.deps.verify_vote_sig)(&sv) {
            return;
        }

        // Vote validity: height matches; validator is in set handled by VoteSet.insert_vote
        if sv.vote.height != self.height {
            return;
        }

        let _ = self.votes.insert_vote(&self.validator_set, sv);

        // After any vote, check if we can progress.
        match self.step {
            Step::Propose | Step::Prevote => {
                // if we have a polka (any), move to precommit
                if self.votes.has_polka_any(&self.validator_set, self.round) {
                    // if we aren't already in precommit, go there
                    if self.step != Step::Precommit {
                        self.enter_precommit();
                    }
                }
            }
            Step::Precommit => {
                // check commit quorum for a block
                if let Some((bh, _power)) = self.find_precommit_quorum_block(self.round) {
                    self.commit_block_hash(bh);
                }
            }
            Step::Commit => {}
        }
    }

    fn handle_timeout_propose(&mut self, h: u64, r: u32) {
        if !self.is_current(h, r) {
            return;
        }
        if self.step != Step::Propose {
            return;
        }
        // No proposal => enter prevote (prevote nil likely)
        self.enter_prevote();
    }

    fn handle_timeout_prevote(&mut self, h: u64, r: u32) {
        if !self.is_current(h, r) {
            return;
        }
        if self.step != Step::Prevote {
            return;
        }
        // If timeout_prevote fires without polka => precommit nil
        self.enter_precommit();
    }

    fn handle_timeout_precommit(&mut self, h: u64, r: u32) {
        if !self.is_current(h, r) {
            return;
        }
        if self.step != Step::Precommit {
            return;
        }
        // No commit => next round
        self.round = self.round.saturating_add(1);
        self.step = Step::Propose;
        // DO NOT reset locked/valid across rounds
        self.enter_propose();
    }

    fn handle_txs_available(&mut self, txs: Vec<Vec<u8>>) {
        if !self.awaiting_txs {
            return;
        }
        let proposer = self.proposer_for(self.height, self.round);
        if proposer != self.my_id {
            return;
        }

        // Build a block deterministically from tx ordering (proposer-defined).
        // Note: prev_block_hash/state_root/tx_merkle_root will be filled later by state/storage layer.
        let block = Block {
            header: crate::types::BlockHeader {
                height: self.height,
                timestamp_ms: 0, // outer shell can inject advisory time
                prev_block_hash: Hash::ZERO,
                proposer: self.my_id,
                validator_set_hash: self.validator_set.set_hash,
                state_root: Hash::ZERO,
                tx_merkle_root: Hash::ZERO,
            },
            txs,
        };

        if !self.cfg.allow_empty_blocks && block.txs.is_empty() {
            // If empty blocks disabled, just let timeouts advance rounds.
            self.awaiting_txs = false;
            return;
        }

        let valid_round = self.valid_round.map(|r| r as i32).unwrap_or(-1);

        // Signature is produced by outer shell in real impl. Here placeholder zero.
        let sp = SignedProposal {
            proposal: crate::types::Proposal {
                height: self.height,
                round: self.round,
                block,
                valid_round,
                proposer: self.my_id,
            },
            signature: [0u8; 64],
        };

        self.awaiting_txs = false;

        // store it as our proposal (so we can prevote it)
        self.proposal = Some(sp.clone());

        self.tx_cmd
            .send(ConsensusCommand::BroadcastProposal { proposal: sp })
            .ok();

        // As proposer, after proposing, enter prevote
        if self.step == Step::Propose {
            self.enter_prevote();
        }
    }

    fn handle_block_executed(&mut self, height: u64, state_root: Hash) {
        if self.step != Step::Commit {
            return;
        }
        if height != self.height {
            return;
        }

        // Persist command (outer shell will do actual storage)
        // We need the committed block; for skeleton, we take it from proposal
        // (in real impl, committed block comes from block cache).
        if let Some(sp) = &self.proposal {
            self.tx_cmd
                .send(ConsensusCommand::PersistBlock {
                    block: sp.proposal.block.clone(),
                    state_root,
                })
                .ok();
        }

        // Advance height
        self.height = self.height.saturating_add(1);
        self.round = 0;
        self.step = Step::Propose;

        // Reset locking/valid for new height
        self.locked_round = None;
        self.locked_block = None;
        self.valid_round = None;
        self.valid_block = None;
        self.proposal = None;
        self.votes = VoteSet::new(self.height);

        // validator set updates apply here later (H+1) once you wire BlockExecuted with updates.

        self.enter_propose();
    }

    // ---------------------------
    // Voting logic from the docs
    // ---------------------------

    fn compute_prevote(&self) -> Option<Hash> {
        let sp = match &self.proposal {
            None => return None, // nil
            Some(sp) => sp,
        };
        let p = &sp.proposal;
        let block = &p.block;

        if !(self.deps.validate_block)(block) {
            return None;
        }

        let bh = (self.deps.block_hash)(block);

        // If not locked => prevote block
        if self.locked_block.is_none() {
            return Some(bh);
        }

        // If locked on same block => prevote it
        if self.locked_block == Some(bh) {
            return Some(bh);
        }

        // If proposal.valid_round >= locked_round and has polka at that valid_round for this block => prevote
        if p.valid_round >= 0 {
            let vr = p.valid_round as u32;
            if let Some(lr) = self.locked_round {
                if vr >= lr && self.votes.has_polka_for(&self.validator_set, vr, Some(bh)) {
                    return Some(bh);
                }
            } else {
                // locked_round None shouldn't happen if locked_block Some, but keep safe
                if self.votes.has_polka_for(&self.validator_set, vr, Some(bh)) {
                    return Some(bh);
                }
            }
        }

        None
    }

    fn compute_precommit(&mut self) -> Option<Hash> {
        // If has polka for a specific block B at (height, round)
        if let Some((bh, _power)) = self.find_prevote_polka_block(self.round) {
            self.valid_block = Some(bh);
            self.valid_round = Some(self.round);

            // If have full block and valid => lock and precommit B, else nil
            if (self.deps.have_full_block)(bh) {
                // We don't have direct block by hash here; assume proposal block matches
                if let Some(sp) = &self.proposal {
                    let ph = (self.deps.block_hash)(&sp.proposal.block);
                    if ph == bh && (self.deps.validate_block)(&sp.proposal.block) {
                        self.locked_block = Some(bh);
                        self.locked_round = Some(self.round);
                        return Some(bh);
                    }
                }
            }
            return None;
        }

        // If polka for nil => unlock and precommit nil
        if self.votes.has_polka_for(&self.validator_set, self.round, None) {
            self.locked_block = None;
            self.locked_round = None;
            return None;
        }

        // Timeout path: nil
        None
    }

    fn broadcast_vote(&mut self, vote_type: VoteType, block_hash: Option<Hash>) {
        let v = Vote {
            vote_type,
            height: self.height,
            round: self.round,
            block_hash,
            validator: self.my_id,
        };
        let sv = SignedVote {
            vote: v,
            signature: [0u8; 64], // outer shell will sign
        };
        self.tx_cmd
            .send(ConsensusCommand::BroadcastVote { vote: sv })
            .ok();
    }

    // ---------------------------
    // Quorum helpers
    // ---------------------------

    fn find_prevote_polka_block(&self, round: u32) -> Option<(Hash, u64)> {
        // Determine if any specific block hash has quorum of prevotes
        // We aggregate by hash deterministically using BTreeMap.
        use std::collections::BTreeMap;

        let thr = VoteSet::quorum_threshold(self.validator_set.total_power());
        let mut agg: BTreeMap<Hash, u64> = BTreeMap::new();

        for sv in self.votes.round_votes(round, VoteType::Prevote) {
            if let Some(h) = sv.vote.block_hash {
                let e = agg.entry(h).or_insert(0);
                *e = e.saturating_add(self.validator_set.voting_power(&sv.vote.validator));
            }
        }

        for (h, p) in agg {
            if p >= thr {
                return Some((h, p));
            }
        }
        None
    }

    fn find_precommit_quorum_block(&self, round: u32) -> Option<(Hash, u64)> {
        use std::collections::BTreeMap;

        let thr = VoteSet::quorum_threshold(self.validator_set.total_power());
        let mut agg: BTreeMap<Hash, u64> = BTreeMap::new();

        for sv in self.votes.round_votes(round, VoteType::Precommit) {
            if let Some(h) = sv.vote.block_hash {
                let e = agg.entry(h).or_insert(0);
                *e = e.saturating_add(self.validator_set.voting_power(&sv.vote.validator));
            }
        }

        for (h, p) in agg {
            if p >= thr {
                return Some((h, p));
            }
        }
        None
    }

    fn commit_block_hash(&mut self, bh: Hash) {
        // Must have >2/3 precommits for bh at this round (checked before calling)
        self.step = Step::Commit;

        // Emit ExecuteBlock command; state machine will reply BlockExecuted
        // We need the block; take from current proposal as skeleton.
        if let Some(sp) = &self.proposal {
            let ph = (self.deps.block_hash)(&sp.proposal.block);
            if ph == bh {
                self.tx_cmd
                    .send(ConsensusCommand::ExecuteBlock {
                        block: sp.proposal.block.clone(),
                    })
                    .ok();
            } else {
                // In real impl: fetch from block store/cache by hash
            }
        }
    }
}
