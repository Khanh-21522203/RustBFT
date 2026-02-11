use bytes::{BufMut, BytesMut};
use chacha20poly1305::{aead::{Aead, KeyInit}, ChaCha20Poly1305, Key, Nonce};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::p2p::msg::MsgType;

#[derive(thiserror::Error, Debug)]
pub enum NetCodecError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("message too large")]
    TooLarge,
    #[error("unknown msg type")]
    UnknownType,
    #[error("aead failed")]
    AeadFailed,
    #[error("protocol error: {0}")]
    Protocol(&'static str),
}

#[derive(Clone)]
pub struct CryptoState {
    pub aead: ChaCha20Poly1305,
    pub send_ctr: u64,
    pub recv_ctr: u64,
    pub dir_send: u8,
    pub dir_recv: u8,
}

impl CryptoState {
    pub fn new(key_bytes32: [u8; 32]) -> Self {
        let key = Key::from_slice(&key_bytes32);
        Self {
            aead: ChaCha20Poly1305::new(key),
            send_ctr: 0,
            recv_ctr: 0,
            dir_send: 0xA1,
            dir_recv: 0xB2,
        }
    }
    fn nonce(dir: u8, ctr: u64) -> Nonce {
        let mut n = [0u8; 12];
        n[0] = dir;
        n[1..9].copy_from_slice(&ctr.to_be_bytes());
        Nonce::from_slice(&n).to_owned()
    }
}

pub struct PlainFramer {
    pub stream: TcpStream,
    pub max_size: usize,
}
impl PlainFramer {
    pub fn new(stream: TcpStream, max_size: usize) -> Self {
        Self { stream, max_size }
    }

    pub async fn write_plain(&mut self, ty: MsgType, payload: &[u8]) -> Result<(), NetCodecError> {
        let len = 1usize + payload.len();
        if len > self.max_size { return Err(NetCodecError::TooLarge); }
        let mut buf = BytesMut::with_capacity(4 + len);
        buf.put_u32(len as u32);
        buf.put_u8(ty as u8);
        buf.extend_from_slice(payload);
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    pub async fn read_plain(&mut self) -> Result<(MsgType, Vec<u8>), NetCodecError> {
        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > self.max_size { return Err(NetCodecError::TooLarge); }
        let mut data = vec![0u8; len];
        self.stream.read_exact(&mut data).await?;
        if data.is_empty() { return Err(NetCodecError::Protocol("empty frame")); }
        let ty = MsgType::from_u8(data[0]).ok_or(NetCodecError::UnknownType)?;
        Ok((ty, data[1..].to_vec()))
    }

    pub fn into_encrypted(self, crypto: CryptoState) -> EncryptedFramer {
        EncryptedFramer { stream: self.stream, max_size: self.max_size, crypto }
    }
}

pub struct EncryptedFramer {
    pub stream: TcpStream,
    pub max_size: usize,
    pub crypto: CryptoState,
}

impl EncryptedFramer {
    pub async fn write(&mut self, ty: MsgType, payload: &[u8]) -> Result<(), NetCodecError> {
        let mut pt = Vec::with_capacity(1 + payload.len());
        pt.push(ty as u8);
        pt.extend_from_slice(payload);
        if pt.len() > self.max_size { return Err(NetCodecError::TooLarge); }

        let nonce = CryptoState::nonce(self.crypto.dir_send, self.crypto.send_ctr);
        self.crypto.send_ctr = self.crypto.send_ctr.wrapping_add(1);
        let ct = self.crypto.aead.encrypt(&nonce, pt.as_ref()).map_err(|_| NetCodecError::AeadFailed)?;
        if ct.len() > self.max_size { return Err(NetCodecError::TooLarge); }

        let mut buf = BytesMut::with_capacity(4 + ct.len());
        buf.put_u32(ct.len() as u32);
        buf.extend_from_slice(&ct);
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    pub async fn read(&mut self) -> Result<(MsgType, Vec<u8>), NetCodecError> {
        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > self.max_size { return Err(NetCodecError::TooLarge); }

        let mut ct = vec![0u8; len];
        self.stream.read_exact(&mut ct).await?;

        let nonce = CryptoState::nonce(self.crypto.dir_recv, self.crypto.recv_ctr);
        self.crypto.recv_ctr = self.crypto.recv_ctr.wrapping_add(1);
        let pt = self.crypto.aead.decrypt(&nonce, ct.as_ref()).map_err(|_| NetCodecError::AeadFailed)?;
        if pt.is_empty() { return Err(NetCodecError::Protocol("empty plaintext")); }

        let ty = MsgType::from_u8(pt[0]).ok_or(NetCodecError::UnknownType)?;
        Ok((ty, pt[1..].to_vec()))
    }
}
