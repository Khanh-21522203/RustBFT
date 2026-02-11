use crate::types::{
    block::{Block, BlockHeader},
    hash::Hash,
    proposal::{Proposal, SignedProposal},
    validator::ValidatorId,
    vote::{SignedVote, Vote, VoteType},
};

#[derive(thiserror::Error, Debug)]
pub enum CodecError {
    #[error("unexpected eof")]
    Eof,
    #[error("invalid data: {0}")]
    Invalid(&'static str),
}

pub struct Encoder {
    buf: Vec<u8>,
}
impl Encoder {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn put_u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    pub fn put_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    pub fn put_u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    pub fn put_bytes32(&mut self, v: &[u8; 32]) {
        self.buf.extend_from_slice(v);
    }
    pub fn put_bytes64(&mut self, v: &[u8; 64]) {
        self.buf.extend_from_slice(v);
    }
    pub fn put_vec(&mut self, data: &[u8]) {
        self.put_u32(data.len() as u32);
        self.buf.extend_from_slice(data);
    }
    pub fn put_opt_hash(&mut self, h: &Option<Hash>) {
        match h {
            None => self.put_u32(0),
            Some(Hash(b)) => {
                self.put_u32(32);
                self.buf.extend_from_slice(b);
            }
        }
    }
}

pub struct Decoder<'a> {
    data: &'a [u8],
    pos: usize,
}
impl<'a> Decoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], CodecError> {
        if self.pos + n > self.data.len() {
            return Err(CodecError::Eof);
        }
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub fn get_u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.take(1)?[0])
    }
    pub fn get_u32(&mut self) -> Result<u32, CodecError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn get_u64(&mut self) -> Result<u64, CodecError> {
        let b = self.take(8)?;
        Ok(u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }
    pub fn get_bytes32(&mut self) -> Result<[u8; 32], CodecError> {
        let b = self.take(32)?;
        let mut out = [0u8; 32];
        out.copy_from_slice(b);
        Ok(out)
    }
    pub fn get_bytes64(&mut self) -> Result<[u8; 64], CodecError> {
        let b = self.take(64)?;
        let mut out = [0u8; 64];
        out.copy_from_slice(b);
        Ok(out)
    }
    pub fn get_vec(&mut self) -> Result<Vec<u8>, CodecError> {
        let n = self.get_u32()? as usize;
        let b = self.take(n)?;
        Ok(b.to_vec())
    }
    pub fn get_opt_hash(&mut self) -> Result<Option<Hash>, CodecError> {
        let n = self.get_u32()? as usize;
        if n == 0 {
            return Ok(None);
        }
        if n != 32 {
            return Err(CodecError::Invalid("opt hash length != 32"));
        }
        Ok(Some(Hash(self.get_bytes32()?)))
    }
}

// ---- VoteType ----
fn encode_vote_type(t: VoteType) -> u8 {
    match t {
        VoteType::Prevote => 1,
        VoteType::Precommit => 2,
    }
}
fn decode_vote_type(b: u8) -> Result<VoteType, CodecError> {
    match b {
        1 => Ok(VoteType::Prevote),
        2 => Ok(VoteType::Precommit),
        _ => Err(CodecError::Invalid("unknown VoteType")),
    }
}

// ---- Block ----
pub fn encode_block(b: &Block) -> Vec<u8> {
    let mut e = Encoder::new();
    encode_block_header(&mut e, &b.header);

    e.put_u32(b.txs.len() as u32);
    for tx in &b.txs {
        e.put_vec(tx);
    }
    e.into_bytes()
}

pub fn decode_block(data: &[u8]) -> Result<Block, CodecError> {
    let mut d = Decoder::new(data);
    let header = decode_block_header(&mut d)?;

    let n = d.get_u32()? as usize;
    let mut txs = Vec::with_capacity(n);
    for _ in 0..n {
        txs.push(d.get_vec()?);
    }
    Ok(Block { header, txs })
}

fn encode_block_header(e: &mut Encoder, h: &BlockHeader) {
    e.put_u64(h.height);
    e.put_u64(h.timestamp_ms);
    e.put_bytes32(&(h.prev_block_hash.0));
    e.put_bytes32(&(h.proposer.0));
    e.put_bytes32(&(h.validator_set_hash.0));
    e.put_bytes32(&(h.state_root.0));
    e.put_bytes32(&(h.tx_merkle_root.0));
}

fn decode_block_header(d: &mut Decoder<'_>) -> Result<BlockHeader, CodecError> {
    Ok(BlockHeader {
        height: d.get_u64()?,
        timestamp_ms: d.get_u64()?,
        prev_block_hash: Hash(d.get_bytes32()?),
        proposer: ValidatorId(d.get_bytes32()?),
        validator_set_hash: Hash(d.get_bytes32()?),
        state_root: Hash(d.get_bytes32()?),
        tx_merkle_root: Hash(d.get_bytes32()?),
    })
}

// ---- Proposal ----
pub fn encode_signed_proposal(sp: &SignedProposal) -> Vec<u8> {
    let p = &sp.proposal;
    let mut e = Encoder::new();
    e.put_u64(p.height);
    e.put_u32(p.round);

    let block_bytes = encode_block(&p.block);
    e.put_vec(&block_bytes);

    e.put_u32(p.valid_round as u32); // sentinel: -1 encoded as u32::MAX
    if p.valid_round < 0 {
        // represent -1 exactly
        // overwrite last 4 bytes with 0xFFFF_FFFF
        let len = e.buf.len();
        e.buf[len - 4..len].copy_from_slice(&u32::MAX.to_be_bytes());
    }

    e.put_bytes32(&(p.proposer.0));
    e.put_bytes64(&sp.signature);
    e.into_bytes()
}

pub fn decode_signed_proposal(data: &[u8]) -> Result<SignedProposal, CodecError> {
    let mut d = Decoder::new(data);
    let height = d.get_u64()?;
    let round = d.get_u32()?;
    let block_bytes = d.get_vec()?;
    let block = decode_block(&block_bytes)?;

    let vr_u32 = d.get_u32()?;
    let valid_round = if vr_u32 == u32::MAX { -1 } else { vr_u32 as i32 };

    let proposer = ValidatorId(d.get_bytes32()?);
    let sig = d.get_bytes64()?;

    Ok(SignedProposal {
        proposal: Proposal {
            height,
            round,
            block,
            valid_round,
            proposer,
        },
        signature: sig,
    })
}

// ---- Vote ----
pub fn encode_signed_vote(sv: &SignedVote) -> Vec<u8> {
    let v = &sv.vote;
    let mut e = Encoder::new();
    e.put_u8(encode_vote_type(v.vote_type));
    e.put_u64(v.height);
    e.put_u32(v.round);
    e.put_opt_hash(&v.block_hash);
    e.put_bytes32(&(v.validator.0));
    e.put_bytes64(&sv.signature);
    e.into_bytes()
}

pub fn decode_signed_vote(data: &[u8]) -> Result<SignedVote, CodecError> {
    let mut d = Decoder::new(data);
    let vt = decode_vote_type(d.get_u8()?)?;
    let height = d.get_u64()?;
    let round = d.get_u32()?;
    let bh = d.get_opt_hash()?;
    let val = ValidatorId(d.get_bytes32()?);
    let sig = d.get_bytes64()?;

    Ok(SignedVote {
        vote: Vote {
            vote_type: vt,
            height,
            round,
            block_hash: bh,
            validator: val,
        },
        signature: sig,
    })
}
