use crate::types::Address;

#[derive(Clone, Debug)]
pub enum DecodedTx {
    Transfer(TransferTx),
    ContractDeploy(ContractDeployTx),
    ContractCall(ContractCallTx),
    ValidatorUpdate(ValidatorUpdateTx),
}

/// TxType (1 byte)
/// 0x01 Transfer
/// 0x02 ContractDeploy
/// 0x03 ContractCall
/// 0x04 ValidatorUpdate
///
/// All integers are big-endian fixed-width.
/// All bytes fields are length-prefixed: u32 BE length + data.
#[derive(thiserror::Error, Debug)]
pub enum TxDecodeError {
    #[error("tx too short")]
    TooShort,
    #[error("unknown tx type: {0:#x}")]
    UnknownType(u8),
    #[error("invalid length")]
    InvalidLength,
    #[error("length prefix too large")]
    LengthTooLarge,
}

#[derive(Clone, Debug)]
pub struct TransferTx {
    pub from: Address,
    pub to: Address,
    pub amount: u128,
    pub nonce: u64,
    pub gas_limit: u64,
    pub signature: [u8; 64],
}

#[derive(Clone, Debug)]
pub struct ContractDeployTx {
    pub from: Address,
    pub nonce: u64,
    pub gas_limit: u64,
    pub wasm: Vec<u8>,
    pub init_input: Vec<u8>,
    pub signature: [u8; 64],
}

#[derive(Clone, Debug)]
pub struct ContractCallTx {
    pub from: Address,
    pub to: Address,
    pub nonce: u64,
    pub gas_limit: u64,
    pub value: u128,
    pub input: Vec<u8>,
    pub signature: [u8; 64],
}

/// Validator update tx: admin-authorized, processed during EndBlock.
/// action: 0x01=Add, 0x02=Remove, 0x03=UpdatePower
#[derive(Clone, Debug)]
pub struct ValidatorUpdateTx {
    pub from: Address,
    pub action: u8,            // 0x01 Add, 0x02 Remove, 0x03 UpdatePower
    pub validator_id: [u8; 32],
    pub new_power: u64,        // 0 for Remove
    pub nonce: u64,
    pub gas_limit: u64,
    pub signature: [u8; 64],
}

pub fn decode_tx(bytes: &[u8]) -> Result<DecodedTx, TxDecodeError> {
    if bytes.is_empty() {
        return Err(TxDecodeError::TooShort);
    }
    let ty = bytes[0];
    let mut b = &bytes[1..];

    let tx = match ty {
        0x01 => DecodedTx::Transfer(decode_transfer(&mut b)?),
        0x02 => DecodedTx::ContractDeploy(decode_deploy(&mut b)?),
        0x03 => DecodedTx::ContractCall(decode_call(&mut b)?),
        0x04 => DecodedTx::ValidatorUpdate(decode_validator_update(&mut b)?),
        other => return Err(TxDecodeError::UnknownType(other)),
    };

    if !b.is_empty() {
        return Err(TxDecodeError::InvalidLength);
    }
    Ok(tx)
}

fn decode_transfer(b: &mut &[u8]) -> Result<TransferTx, TxDecodeError> {
    let from = Address(take_20(b)?);
    let to = Address(take_20(b)?);
    let amount = take_u128(b)?;
    let nonce = take_u64(b)?;
    let gas_limit = take_u64(b)?;
    let signature = take_64(b)?;

    Ok(TransferTx {
        from,
        to,
        amount,
        nonce,
        gas_limit,
        signature,
    })
}

fn decode_deploy(b: &mut &[u8]) -> Result<ContractDeployTx, TxDecodeError> {
    let from = Address(take_20(b)?);
    let nonce = take_u64(b)?;
    let gas_limit = take_u64(b)?;
    let wasm = take_bytes_u32(b, 512 * 1024)?; // MVP cap: 512KB
    let init_input = take_bytes_u32(b, 256 * 1024)?; // MVP cap: 256KB
    let signature = take_64(b)?;

    Ok(ContractDeployTx {
        from,
        nonce,
        gas_limit,
        wasm,
        init_input,
        signature,
    })
}

fn decode_call(b: &mut &[u8]) -> Result<ContractCallTx, TxDecodeError> {
    let from = Address(take_20(b)?);
    let to = Address(take_20(b)?);
    let nonce = take_u64(b)?;
    let gas_limit = take_u64(b)?;
    let value = take_u128(b)?;
    let input = take_bytes_u32(b, 256 * 1024)?; // MVP cap: 256KB
    let signature = take_64(b)?;

    Ok(ContractCallTx {
        from,
        to,
        nonce,
        gas_limit,
        value,
        input,
        signature,
    })
}

fn decode_validator_update(b: &mut &[u8]) -> Result<ValidatorUpdateTx, TxDecodeError> {
    let from = Address(take_20(b)?);
    if b.is_empty() { return Err(TxDecodeError::TooShort); }
    let action = b[0];
    *b = &b[1..];
    let validator_id = take_32(b)?;
    let new_power = take_u64(b)?;
    let nonce = take_u64(b)?;
    let gas_limit = take_u64(b)?;
    let signature = take_64(b)?;

    Ok(ValidatorUpdateTx {
        from,
        action,
        validator_id,
        new_power,
        nonce,
        gas_limit,
        signature,
    })
}

// --------- canonical readers ----------

fn take_20(b: &mut &[u8]) -> Result<[u8; 20], TxDecodeError> {
    if b.len() < 20 {
        return Err(TxDecodeError::TooShort);
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&b[..20]);
    *b = &b[20..];
    Ok(out)
}

fn take_32(b: &mut &[u8]) -> Result<[u8; 32], TxDecodeError> {
    if b.len() < 32 {
        return Err(TxDecodeError::TooShort);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&b[..32]);
    *b = &b[32..];
    Ok(out)
}

fn take_64(b: &mut &[u8]) -> Result<[u8; 64], TxDecodeError> {
    if b.len() < 64 {
        return Err(TxDecodeError::TooShort);
    }
    let mut out = [0u8; 64];
    out.copy_from_slice(&b[..64]);
    *b = &b[64..];
    Ok(out)
}

fn take_u64(b: &mut &[u8]) -> Result<u64, TxDecodeError> {
    if b.len() < 8 {
        return Err(TxDecodeError::TooShort);
    }
    let mut x = [0u8; 8];
    x.copy_from_slice(&b[..8]);
    *b = &b[8..];
    Ok(u64::from_be_bytes(x))
}

fn take_u128(b: &mut &[u8]) -> Result<u128, TxDecodeError> {
    if b.len() < 16 {
        return Err(TxDecodeError::TooShort);
    }
    let mut x = [0u8; 16];
    x.copy_from_slice(&b[..16]);
    *b = &b[16..];
    Ok(u128::from_be_bytes(x))
}

fn take_bytes_u32(b: &mut &[u8], max: usize) -> Result<Vec<u8>, TxDecodeError> {
    if b.len() < 4 {
        return Err(TxDecodeError::TooShort);
    }
    let len = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
    *b = &b[4..];

    if len > max {
        return Err(TxDecodeError::LengthTooLarge);
    }
    if b.len() < len {
        return Err(TxDecodeError::TooShort);
    }

    let out = b[..len].to_vec();
    *b = &b[len..];
    Ok(out)
}
