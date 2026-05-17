use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatorMetadata {
    pub moniker: Option<String>,
    pub website: Option<String>,
    pub metadata_url: Option<String>,
    pub security_contact: Option<String>,
    pub p2p_address: Option<String>,
    pub rpc_address: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommissionPolicy {
    pub rate_ppm: u32,
    pub max_rate_ppm: u32,
    pub max_change_rate_ppm: u32,
}

impl Default for CommissionPolicy {
    fn default() -> Self {
        Self {
            rate_ppm: 0,
            max_rate_ppm: 1_000_000,
            max_change_rate_ppm: 0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatorEconomics {
    pub min_self_bond: u128,
    pub commission: CommissionPolicy,
}
