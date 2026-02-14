use crate::consensus::events::{ConsensusCommand, ConsensusEvent, Timeout, TimeoutKind};
use crate::consensus::proposer::select_proposer;
use crate::consensus::vote_set::VoteSet;
use crate::storage::wal::{WalEntry, WalEntryKind};
use crate::types::{
    Block, Hash, SignedProposal, SignedVote, ValidatorId, ValidatorSet, ValidatorSetPolicy,
    ValidatorUpdate, Vote, VoteType,
};
use crossbeam_channel::{Receiver, Sender};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    NewHeight,
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

    // policy for validator set updates (doc 8)
    validator_set_policy: ValidatorSetPolicy,

    // node identity
    my_id: ValidatorId,

    // channels
    rx: Receiver<ConsensusEvent>,
    tx_cmd: Sender<ConsensusCommand>,

    // if we are proposer and asked mempool for txs
    awaiting_txs: bool,

    // track whether we already voted this round (invariants I1, I2)
    prevoted_this_round: bool,
    precommitted_this_round: bool,
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
            step: Step::NewHeight,
            locked_round: None,
            locked_block: None,
            valid_round: None,
            valid_block: None,
            proposal: None,
            votes: VoteSet::new(height),
            validator_set,
            validator_set_policy: ValidatorSetPolicy::default(),
            my_id,
            rx,
            tx_cmd,
            awaiting_txs: false,
            prevoted_this_round: false,
            precommitted_this_round: false,
        }
    }

    pub fn run(mut self) {
        // Enter first height/round
        self.enter_new_height();

        loop {
            // 1. Blocking receive
            let ev = match self.rx.recv() {
                Ok(ev) => ev,
                Err(_) => break, // channel closed => shutdown
            };

            self.process_event(ev);

            // 2. Batch processing: drain any additional events available without blocking
            while let Ok(ev) = self.rx.try_recv() {
                self.process_event(ev);
            }
        }
    }

    fn process_event(&mut self, ev: ConsensusEvent) {
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

            ConsensusEvent::BlockExecuted { height, state_root, validator_updates } => {
                self.handle_block_executed(height, state_root, validator_updates)
            }
        }
    }

    fn is_current(&self, h: u64, r: u32) -> bool {
        self.height == h && self.round == r
    }

    fn proposer_for(&self, h: u64, r: u32) -> ValidatorId {
        select_proposer(&self.validator_set, h, r)
    }

    // -------------------------------------------------------
    // State transitions
    // -------------------------------------------------------

    fn enter_new_height(&mut self) {
        self.step = Step::NewHeight;
        self.round = 0;
        self.locked_round = None;
        self.locked_block = None;
        self.valid_round = None;
        self.valid_block = None;
        self.proposal = None;
        self.votes = VoteSet::new(self.height);
        self.prevoted_this_round = false;
        self.precommitted_this_round = false;

        // Immediately enter Propose for round 0
        self.enter_propose();
    }

    fn enter_round(&mut self, round: u32) {
        self.round = round;
        self.proposal = None;
        self.awaiting_txs = false;
        self.prevoted_this_round = false;
        self.precommitted_this_round = false;
        // DO NOT reset locked/valid across rounds
        self.enter_propose();
    }

    fn enter_propose(&mut self) {
        self.step = Step::Propose;
        self.awaiting_txs = false;

        let proposer = self.proposer_for(self.height, self.round);
        if proposer == self.my_id {
            if self.valid_block.is_some() {
                // Re-propose valid_block; for MVP we request txs to rebuild
                self.awaiting_txs = true;
                self.tx_cmd
                    .send(ConsensusCommand::ReapTxs {
                        max_bytes: self.cfg.max_reap_bytes,
                    })
                    .ok();
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

        if !self.prevoted_this_round {
            self.prevoted_this_round = true;
            let vote_hash = self.compute_prevote();
            self.broadcast_vote(VoteType::Prevote, vote_hash);
        }

        // Check if we already have enough prevotes to move to precommit
        self.check_prevote_quorums();
    }

    fn enter_precommit(&mut self) {
        self.step = Step::Precommit;

        if !self.precommitted_this_round {
            self.precommitted_this_round = true;
            let vote_hash = self.compute_precommit();
            self.broadcast_vote(VoteType::Precommit, vote_hash);
        }

        // Check if we already have enough precommits to commit
        self.check_commit_quorum();
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

    // -------------------------------------------------------
    // Event handlers
    // -------------------------------------------------------

    fn handle_proposal(&mut self, sp: SignedProposal) {
        if !(self.deps.verify_proposal_sig)(&sp) {
            return;
        }

        let p = &sp.proposal;
        if p.height != self.height || p.round != self.round {
            return;
        }

        // Only accept first proposal for round
        if self.proposal.is_some() {
            return;
        }

        // Basic block validity check (consensus-level)
        if !(self.deps.validate_block)(&p.block) {
            return;
        }

        self.proposal = Some(sp.clone());

        // WAL: record received proposal for crash recovery
        self.wal_write(WalEntryKind::Proposal, &serde_json::to_vec(&sp).unwrap_or_default());

        // On receiving proposal while in Propose step, enter Prevote immediately.
        if self.step == Step::Propose {
            self.enter_prevote();
        }

        // If we're in Prevote and already have a polka for this proposal, re-check
        if self.step == Step::Prevote {
            self.check_prevote_quorums();
        }
    }

    fn handle_vote(&mut self, sv: SignedVote) {
        if !(self.deps.verify_vote_sig)(&sv) {
            return;
        }

        // Stale event handling per doc 12 section 10
        if sv.vote.height < self.height {
            return; // already committed
        }
        if sv.vote.height > self.height {
            return; // future height, ignore for now (block sync would handle)
        }

        // Check for round skip (doc 12 section 5.2):
        // If >1/3 votes at a higher round, skip to that round
        let vote_round = sv.vote.round;

        match self.votes.insert_vote(&self.validator_set, sv) {
            Ok(()) => {}
            Err(crate::consensus::vote_set::VoteSetError::Duplicate) => return,
            Err(crate::consensus::vote_set::VoteSetError::Equivocation) => {
                // Log equivocation evidence (doc 12 section 10)
                let evidence = self.votes.evidence();
                if let Some(ev) = evidence.last() {
                    eprintln!(
                        "EQUIVOCATION DETECTED: validator {:?} at height={} round={}",
                        ev.vote_a.vote.validator, ev.vote_a.vote.height, ev.vote_a.vote.round
                    );
                }
                return;
            }
            Err(crate::consensus::vote_set::VoteSetError::HeightMismatch) => return,
            Err(crate::consensus::vote_set::VoteSetError::NonValidator) => return,
            Err(crate::consensus::vote_set::VoteSetError::UnexpectedVoteType) => return,
        }

        // Round skip check: if vote is from a higher round
        if vote_round > self.round {
            self.check_round_skip(vote_round);
        }

        // Cross-state commit check (doc 12 section 5.1):
        // If at any point we have >2/3 precommits for a block, commit it
        self.check_any_round_commit();

        // After any vote, check if we can progress in current round
        match self.step {
            Step::Propose => {
                // Votes buffered; if polka formed, we'll catch it when entering prevote
            }
            Step::Prevote => {
                self.check_prevote_quorums();
            }
            Step::Precommit => {
                self.check_commit_quorum();
            }
            Step::Commit | Step::NewHeight => {}
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
        self.enter_round(self.round.saturating_add(1));
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
            last_commit: None, // MVP: commit info will be populated by outer shell
        };

        if !self.cfg.allow_empty_blocks && block.txs.is_empty() {
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

    fn handle_block_executed(
        &mut self,
        height: u64,
        state_root: Hash,
        validator_updates: Vec<ValidatorUpdate>,
    ) {
        if self.step != Step::Commit {
            return;
        }
        if height != self.height {
            return;
        }

        // Persist command (outer shell will do actual storage)
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

        // Apply validator set updates at H+1 (doc 8 section 3)
        if !validator_updates.is_empty() {
            match self.validator_set.apply_updates(&validator_updates, &self.validator_set_policy) {
                Ok(new_set) => {
                    self.validator_set = new_set;
                }
                Err(e) => {
                    // Safety rules violated; keep old validator set.
                    // In production, this should be logged/alerted.
                    eprintln!("validator set update rejected: {}", e);
                }
            }
        }

        self.enter_new_height();
    }

    // -------------------------------------------------------
    // Quorum checks
    // -------------------------------------------------------

    /// Cross-state commit: check ALL rounds for a precommit quorum (doc 12 section 5.1)
    fn check_any_round_commit(&mut self) {
        if self.step == Step::Commit || self.step == Step::NewHeight {
            return;
        }

        // Check all rounds we have votes for
        for round in 0..=self.round.saturating_add(10) {
            if let Some((bh, _)) = self.find_precommit_quorum_block(round) {
                self.commit_block_hash(bh, round);
                return;
            }
        }
    }

    /// Check if >1/3 of voting power has votes at a higher round (doc 12 section 5.2)
    fn check_round_skip(&mut self, target_round: u32) {
        if target_round <= self.round {
            return;
        }
        if self.step == Step::Commit {
            return;
        }

        // Count total voting power at target_round (prevotes + precommits)
        let mut power = 0u64;
        let mut seen = std::collections::BTreeSet::new();

        for sv in self.votes.round_votes(target_round, VoteType::Prevote) {
            if seen.insert(sv.vote.validator) {
                power = power.saturating_add(self.validator_set.voting_power(&sv.vote.validator));
            }
        }
        for sv in self.votes.round_votes(target_round, VoteType::Precommit) {
            if seen.insert(sv.vote.validator) {
                power = power.saturating_add(self.validator_set.voting_power(&sv.vote.validator));
            }
        }

        // >1/3 threshold
        let skip_threshold = self.validator_set.total_power() / 3 + 1;
        if power >= skip_threshold {
            self.enter_round(target_round);
        }
    }

    fn check_prevote_quorums(&mut self) {
        if self.step != Step::Prevote {
            return;
        }

        // Check for polka for a specific block
        if let Some((_bh, _)) = self.find_prevote_polka_block(self.round) {
            self.enter_precommit();
            return;
        }

        // Check for polka for nil
        if self.votes.has_polka_for(&self.validator_set, self.round, None) {
            self.enter_precommit();
            return;
        }

        // Check for polka any (>2/3 total prevotes) -> schedule timeout_prevote
        if self.votes.has_polka_any(&self.validator_set, self.round) {
            self.schedule_timeout(TimeoutKind::Prevote);
        }
    }

    fn check_commit_quorum(&mut self) {
        if self.step != Step::Precommit {
            return;
        }

        if let Some((bh, _)) = self.find_precommit_quorum_block(self.round) {
            self.commit_block_hash(bh, self.round);
            return;
        }

        // Check for >2/3 precommits total (any value) -> schedule timeout_precommit
        let thr = VoteSet::quorum_threshold(self.validator_set.total_power());
        let mut total = 0u64;
        for sv in self.votes.round_votes(self.round, VoteType::Precommit) {
            total = total.saturating_add(self.validator_set.voting_power(&sv.vote.validator));
        }
        if total >= thr {
            self.schedule_timeout(TimeoutKind::Precommit);
        }
    }

    // -------------------------------------------------------
    // Voting logic from the docs
    // -------------------------------------------------------

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

        // WAL: record our vote before broadcasting
        let wal_kind = match vote_type {
            VoteType::Prevote => WalEntryKind::Prevote,
            VoteType::Precommit => WalEntryKind::Precommit,
        };
        self.wal_write(wal_kind, &serde_json::to_vec(&sv).unwrap_or_default());

        self.tx_cmd
            .send(ConsensusCommand::BroadcastVote { vote: sv })
            .ok();
    }

    /// Write a WAL entry for crash recovery (doc 9 section 5).
    fn wal_write(&self, kind: WalEntryKind, data: &[u8]) {
        self.tx_cmd
            .send(ConsensusCommand::WriteWAL {
                entry: WalEntry {
                    height: self.height,
                    round: self.round,
                    kind,
                    data: data.to_vec(),
                },
            })
            .ok();
    }

    // -------------------------------------------------------
    // Quorum helpers
    // -------------------------------------------------------

    fn find_prevote_polka_block(&self, round: u32) -> Option<(Hash, u64)> {
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

    fn commit_block_hash(&mut self, bh: Hash, round: u32) {
        self.step = Step::Commit;
        self.round = round;

        // Cancel all pending timeouts for this height
        self.tx_cmd
            .send(ConsensusCommand::CancelTimeout {
                timeout: Timeout {
                    height: self.height,
                    round: self.round,
                    kind: TimeoutKind::Propose,
                    duration_ms: 0,
                },
            })
            .ok();

        // Emit ExecuteBlock command; state machine will reply BlockExecuted
        if let Some(sp) = &self.proposal {
            let ph = (self.deps.block_hash)(&sp.proposal.block);
            if ph == bh {
                self.tx_cmd
                    .send(ConsensusCommand::ExecuteBlock {
                        block: sp.proposal.block.clone(),
                    })
                    .ok();
            }
            // In real impl: fetch from block store/cache by hash if proposal doesn't match
        }
    }
}
