use bytes::{BufMut, BytesMut};
use chacha20poly1305::{aead::KeyInit, ChaCha20Poly1305, Key, Nonce};
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

/// Post-handshake encrypted framer.
///
/// After handshake the TcpStream is split into OwnedReadHalf / OwnedWriteHalf
/// for concurrent reader/writer tasks in manager.rs. Therefore this struct only
/// serves as a container to transfer (stream, max_size, crypto) from handshake
/// to the manager; the actual AEAD read/write is done inline on the split halves.
pub struct EncryptedFramer {
    pub stream: TcpStream,
    pub max_size: usize,
    pub crypto: CryptoState,
}
