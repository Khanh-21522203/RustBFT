use rustbft_core::{Hash, ValidatorId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlashReason {
    Equivocation,
    Downtime,
    InvalidStateTransition,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashEvent {
    pub validator_id: ValidatorId,
    pub reason: SlashReason,
    pub evidence_hash: Hash,
    pub slash_fraction_ppm: u32,
}
