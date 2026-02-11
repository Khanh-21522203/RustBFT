use crate::types::{SignedProposal, SignedVote};
use crate::types::serialization::{
    decode_signed_proposal, decode_signed_vote, encode_signed_proposal, encode_signed_vote, CodecError,
};

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsgType {
    Handshake = 0x01,
    HandshakeAck = 0x02,
    Proposal = 0x03,
    Prevote = 0x04,
    Precommit = 0x05,
    Transaction = 0x06,
    Evidence = 0x07,
    BlockRequest = 0x08,
    BlockResponse = 0x09,
    Ping = 0x0A,
    Pong = 0x0B,
    Disconnect = 0x0C,
}

impl MsgType {
    pub fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            0x01 => MsgType::Handshake,
            0x02 => MsgType::HandshakeAck,
            0x03 => MsgType::Proposal,
            0x04 => MsgType::Prevote,
            0x05 => MsgType::Precommit,
            0x06 => MsgType::Transaction,
            0x07 => MsgType::Evidence,
            0x08 => MsgType::BlockRequest,
            0x09 => MsgType::BlockResponse,
            0x0A => MsgType::Ping,
            0x0B => MsgType::Pong,
            0x0C => MsgType::Disconnect,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub enum NetworkMessage {
    Proposal(SignedProposal),
    Prevote(SignedVote),
    Precommit(SignedVote),
    Transaction(Vec<u8>),
    Evidence(Vec<u8>),
    Ping(u64),
    Pong(u64),
    Disconnect(u8),
    BlockRequest(Vec<u8>),
    BlockResponse(Vec<u8>),
}

pub fn encode_message(msg: &NetworkMessage) -> (MsgType, Vec<u8>) {
    match msg {
        NetworkMessage::Proposal(p) => (MsgType::Proposal, encode_signed_proposal(p)),
        NetworkMessage::Prevote(v) => (MsgType::Prevote, encode_signed_vote(v)),
        NetworkMessage::Precommit(v) => (MsgType::Precommit, encode_signed_vote(v)),
        NetworkMessage::Transaction(t) => (MsgType::Transaction, t.clone()),
        NetworkMessage::Evidence(e) => (MsgType::Evidence, e.clone()),
        NetworkMessage::Ping(n) => (MsgType::Ping, n.to_be_bytes().to_vec()),
        NetworkMessage::Pong(n) => (MsgType::Pong, n.to_be_bytes().to_vec()),
        NetworkMessage::Disconnect(c) => (MsgType::Disconnect, vec![*c]),
        NetworkMessage::BlockRequest(b) => (MsgType::BlockRequest, b.clone()),
        NetworkMessage::BlockResponse(b) => (MsgType::BlockResponse, b.clone()),
    }
}

pub fn decode_message(ty: MsgType, payload: &[u8]) -> Result<NetworkMessage, CodecError> {
    match ty {
        MsgType::Proposal => Ok(NetworkMessage::Proposal(decode_signed_proposal(payload)?)),
        MsgType::Prevote => Ok(NetworkMessage::Prevote(decode_signed_vote(payload)?)),
        MsgType::Precommit => Ok(NetworkMessage::Precommit(decode_signed_vote(payload)?)),
        MsgType::Transaction => Ok(NetworkMessage::Transaction(payload.to_vec())),
        MsgType::Evidence => Ok(NetworkMessage::Evidence(payload.to_vec())),
        MsgType::Ping => {
            if payload.len() != 8 { return Err(CodecError::Invalid("ping len")); }
            Ok(NetworkMessage::Ping(u64::from_be_bytes(payload.try_into().unwrap())))
        }
        MsgType::Pong => {
            if payload.len() != 8 { return Err(CodecError::Invalid("pong len")); }
            Ok(NetworkMessage::Pong(u64::from_be_bytes(payload.try_into().unwrap())))
        }
        MsgType::Disconnect => Ok(NetworkMessage::Disconnect(payload.get(0).copied().unwrap_or(0))),
        MsgType::BlockRequest => Ok(NetworkMessage::BlockRequest(payload.to_vec())),
        MsgType::BlockResponse => Ok(NetworkMessage::BlockResponse(payload.to_vec())),
        MsgType::Handshake | MsgType::HandshakeAck => Err(CodecError::Invalid("handshake handled separately")),
    }
}
