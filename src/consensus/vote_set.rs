use crate::types::{Evidence, Hash, SignedVote, ValidatorId, ValidatorSet, VoteType};
use std::collections::BTreeMap;

#[derive(thiserror::Error, Debug)]
pub enum VoteSetError {
    #[error("vote from non-validator")]
    NonValidator,
    #[error("height mismatch")]
    HeightMismatch,
    #[error("unexpected vote type for current step")]
    UnexpectedVoteType,
    #[error("duplicate vote (same block hash)")]
    Duplicate,
    #[error("equivocation detected")]
    Equivocation,
}

#[derive(Clone, Debug)]
pub struct VoteSet {
    height: u64,
    // key: (round, vote_type, validator)
    votes: BTreeMap<(u32, VoteType, ValidatorId), SignedVote>,
    evidence: Vec<Evidence>,
}

impl VoteSet {
    pub fn new(height: u64) -> Self {
        Self {
            height,
            votes: BTreeMap::new(),
            evidence: Vec::new(),
        }
    }

    pub fn height(&self) -> u64 {
        self.height
    }

    pub fn evidence(&self) -> &[Evidence] {
        &self.evidence
    }

    pub fn insert_vote(&mut self, vset: &ValidatorSet, signed: SignedVote) -> Result<(), VoteSetError> {
        let vote = &signed.vote;

        if vote.height != self.height {
            return Err(VoteSetError::HeightMismatch);
        }
        if !vset.contains(&vote.validator) {
            return Err(VoteSetError::NonValidator);
        }

        let key = (vote.round, vote.vote_type, vote.validator);
        if let Some(existing) = self.votes.get(&key) {
            // same (h,r,type,validator)
            if existing.vote.block_hash == vote.block_hash {
                return Err(VoteSetError::Duplicate);
            }
            // equivocation
            self.evidence.push(Evidence {
                vote_a: existing.clone(),
                vote_b: signed,
            });
            return Err(VoteSetError::Equivocation);
        }

        self.votes.insert(key, signed);
        Ok(())
    }

    pub fn quorum_threshold(total_power: u64) -> u64 {
        // (2/3)*total + 1 using integer math
        (total_power.saturating_mul(2) / 3).saturating_add(1)
    }

    pub fn round_votes(&self, round: u32, vote_type: VoteType) -> impl Iterator<Item = &SignedVote> {
        self.votes
            .range((round, vote_type, ValidatorId([0u8; 32]))..=(round, vote_type, ValidatorId([0xFFu8; 32])))
            .map(|(_, v)| v)
    }

    pub fn sum_power_for(&self, vset: &ValidatorSet, round: u32, vote_type: VoteType, block_hash: Option<Hash>) -> u64 {
        let mut sum = 0u64;
        for sv in self.round_votes(round, vote_type) {
            if sv.vote.block_hash == block_hash {
                sum = sum.saturating_add(vset.voting_power(&sv.vote.validator));
            }
        }
        sum
    }

    pub fn has_polka_for(&self, vset: &ValidatorSet, round: u32, block_hash: Option<Hash>) -> bool {
        let thr = Self::quorum_threshold(vset.total_power());
        self.sum_power_for(vset, round, VoteType::Prevote, block_hash) >= thr
    }

    pub fn has_polka_any(&self, vset: &ValidatorSet, round: u32) -> bool {
        let thr = Self::quorum_threshold(vset.total_power());
        let mut sum = 0u64;
        for sv in self.round_votes(round, VoteType::Prevote) {
            sum = sum.saturating_add(vset.voting_power(&sv.vote.validator));
        }
        sum >= thr
    }

    pub fn has_commit_quorum(&self, vset: &ValidatorSet, round: u32, block_hash: Hash) -> bool {
        let thr = Self::quorum_threshold(vset.total_power());
        self.sum_power_for(vset, round, VoteType::Precommit, Some(block_hash)) >= thr
    }
}
