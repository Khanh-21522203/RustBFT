use crate::types::Address;

#[derive(Clone, Debug)]
pub enum DecodedTx {
    Transfer(TransferTx),
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

#[derive(thiserror::Error, Debug)]
pub enum TxDecodeError {
    #[error("tx too short")]
    TooShort,
    #[error("unknown tx type")]
    UnknownType,
    #[error("invalid length")]
    InvalidLength,
}

pub fn decode_tx(bytes: &[u8]) -> Result<DecodedTx, TxDecodeError> {
    if bytes.len() < 1 {
        return Err(TxDecodeError::TooShort);
    }

    match bytes[0] {
        0x01 => decode_transfer(&bytes[1..]).map(DecodedTx::Transfer),
        _ => Err(TxDecodeError::UnknownType),
    }
}

fn decode_transfer(mut b: &[u8]) -> Result<TransferTx, TxDecodeError> {
    let from = take_20(&mut b)?;
    let to = take_20(&mut b)?;
    let amount = take_u128(&mut b)?;
    let nonce = take_u64(&mut b)?;
    let gas_limit = take_u64(&mut b)?;
    let sig = take_64(&mut b)?;

    if !b.is_empty() {
        return Err(TxDecodeError::InvalidLength);
    }

    Ok(TransferTx {
        from: Address(from),
        to: Address(to),
        amount,
        nonce,
        gas_limit,
        signature: sig,
    })
}

fn take_20(b: &mut &[u8]) -> Result<[u8; 20], TxDecodeError> {
    if b.len() < 20 {
        return Err(TxDecodeError::TooShort);
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&b[..20]);
    *b = &b[20..];
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
