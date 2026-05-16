use std::collections::BTreeMap;

use rustbft_core::{Hash, ValidatorId, ValidatorSet};
use serde::{Deserialize, Serialize};

use crate::types::{Phase, SignedHotStuffVote, ViewNumber};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuorumCertificate {
    pub view: ViewNumber,
    pub phase: Phase,
    pub block_hash: Hash,
    pub signatures: Vec<SignedHotStuffVote>,
}

impl QuorumCertificate {
    pub fn genesis() -> Self {
        Self {
            view: 0,
            phase: Phase::Prepare,
            block_hash: Hash::ZERO,
            signatures: Vec::new(),
        }
    }

    pub fn quorum_threshold(total_power: u64) -> u64 {
        total_power.saturating_mul(2) / 3 + 1
    }

    pub fn voting_power(&self, validator_set: &ValidatorSet) -> u64 {
        let mut seen = std::collections::BTreeSet::new();
        let mut power = 0u64;
        for signed in &self.signatures {
            let vote = &signed.vote;
            if vote.view != self.view
                || vote.phase != self.phase
                || vote.block_hash != self.block_hash
            {
                continue;
            }
            if seen.insert(vote.validator) {
                power = power.saturating_add(validator_set.voting_power(&vote.validator));
            }
        }
        power
    }

    pub fn has_quorum(&self, validator_set: &ValidatorSet) -> bool {
        self.voting_power(validator_set) >= Self::quorum_threshold(validator_set.total_power())
    }
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum QuorumError {
    #[error("vote from non-validator")]
    NonValidator,
    #[error("vote does not match accumulator target")]
    TargetMismatch,
    #[error("duplicate vote")]
    Duplicate,
    #[error("conflicting vote from validator")]
    ConflictingVote,
    #[error("quorum has not been reached")]
    NoQuorum,
}

#[derive(Clone, Debug)]
pub struct VoteAccumulator {
    view: ViewNumber,
    phase: Phase,
    block_hash: Hash,
    votes: BTreeMap<ValidatorId, SignedHotStuffVote>,
}

impl VoteAccumulator {
    pub fn new(view: ViewNumber, phase: Phase, block_hash: Hash) -> Self {
        Self {
            view,
            phase,
            block_hash,
            votes: BTreeMap::new(),
        }
    }

    pub fn insert(
        &mut self,
        validator_set: &ValidatorSet,
        vote: SignedHotStuffVote,
    ) -> Result<(), QuorumError> {
        if !validator_set.contains(&vote.vote.validator) {
            return Err(QuorumError::NonValidator);
        }
        if vote.vote.view != self.view
            || vote.vote.phase != self.phase
            || vote.vote.block_hash != self.block_hash
        {
            return Err(QuorumError::TargetMismatch);
        }

        if let Some(existing) = self.votes.get(&vote.vote.validator) {
            if existing.vote == vote.vote {
                return Err(QuorumError::Duplicate);
            }
            return Err(QuorumError::ConflictingVote);
        }

        self.votes.insert(vote.vote.validator, vote);
        Ok(())
    }

    pub fn power(&self, validator_set: &ValidatorSet) -> u64 {
        self.votes
            .keys()
            .map(|id| validator_set.voting_power(id))
            .fold(0u64, u64::saturating_add)
    }

    pub fn has_quorum(&self, validator_set: &ValidatorSet) -> bool {
        self.power(validator_set)
            >= QuorumCertificate::quorum_threshold(validator_set.total_power())
    }

    pub fn build_qc(&self, validator_set: &ValidatorSet) -> Result<QuorumCertificate, QuorumError> {
        if !self.has_quorum(validator_set) {
            return Err(QuorumError::NoQuorum);
        }
        Ok(QuorumCertificate {
            view: self.view,
            phase: self.phase,
            block_hash: self.block_hash,
            signatures: self.votes.values().cloned().collect(),
        })
    }
}
