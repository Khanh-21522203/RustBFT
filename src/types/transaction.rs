use crate::types::{Address, Hash};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxKind {
    Transfer,
    ContractDeploy,
    ContractCall,
    ValidatorUpdate,
}

/// MVP signature bytes.
/// You can replace this with your own signature type from crypto::ed25519.
pub type SignatureBytes = [u8; 64];

#[derive(Clone, Debug)]
pub enum Transaction {
    Transfer(TransferTx),
    ContractDeploy(ContractDeployTx),
    ContractCall(ContractCallTx),
    ValidatorUpdate(ValidatorUpdateTx),
}

impl Transaction {
    pub fn gas_limit(&self) -> u64 {
        match self {
            Transaction::Transfer(t) => t.gas_limit,
            Transaction::ContractDeploy(t) => t.gas_limit,
            Transaction::ContractCall(t) => t.gas_limit,
            Transaction::ValidatorUpdate(t) => t.gas_limit,
        }
    }

    pub fn sender(&self) -> Address {
        match self {
            Transaction::Transfer(t) => t.from,
            Transaction::ContractDeploy(t) => t.from,
            Transaction::ContractCall(t) => t.from,
            Transaction::ValidatorUpdate(t) => t.from,
        }
    }

    pub fn kind(&self) -> TxKind {
        match self {
            Transaction::Transfer(_) => TxKind::Transfer,
            Transaction::ContractDeploy(_) => TxKind::ContractDeploy,
            Transaction::ContractCall(_) => TxKind::ContractCall,
            Transaction::ValidatorUpdate(_) => TxKind::ValidatorUpdate,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TransferTx {
    pub from: Address,
    pub to: Address,
    pub amount: u128,
    pub nonce: u64,
    pub gas_limit: u64,
    pub signature: SignatureBytes,
}

#[derive(Clone, Debug)]
pub struct ContractDeployTx {
    pub from: Address,
    pub code: Vec<u8>,
    pub init_args: Vec<u8>,
    pub nonce: u64,
    pub gas_limit: u64,
    pub signature: SignatureBytes,
}

#[derive(Clone, Debug)]
pub struct ContractCallTx {
    pub from: Address,
    pub to: Address,
    pub input: Vec<u8>,
    pub amount: u128,
    pub nonce: u64,
    pub gas_limit: u64,
    pub signature: SignatureBytes,
}

#[derive(Clone, Debug)]
pub enum ValidatorAction {
    Add,
    Remove,
    UpdatePower,
}

/// This tx is executed during EndBlock (special tx). It remains in block even if it fails.
#[derive(Clone, Debug)]
pub struct ValidatorUpdateTx {
    pub from: Address,
    pub action: ValidatorAction,
    pub validator_id: [u8; 32],
    pub new_power: Option<u64>,
    pub nonce: u64,
    pub gas_limit: u64,
    pub signature: SignatureBytes,
}

/// Receipt is produced for every tx deterministically (even failures).
#[derive(Clone, Debug)]
pub struct Receipt {
    pub success: bool,
    pub gas_used: u64,
    pub error: Option<String>,
    pub logs: Vec<Vec<u8>>,
}

/// Deterministic hash placeholder. Replace with canonical encoding + sha256.
pub fn tx_hash(_tx: &Transaction) -> Hash {
    Hash([0u8; 32])
}
