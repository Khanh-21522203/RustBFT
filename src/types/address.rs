use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Address(pub [u8; 20]);

impl Address {
    pub const ZERO: Address = Address([0u8; 20]);
}