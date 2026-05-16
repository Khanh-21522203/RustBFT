use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use rustbft_core::crypto::sha256;

// WalEntry and WalEntryKind are now defined in rustbft-consensus
pub use rustbft_consensus::{WalEntry, WalEntryKind};

/// Encode a WalEntry to bytes (binary format).
pub fn encode_wal_entry(entry: &WalEntry) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&entry.height.to_be_bytes());
    buf.extend_from_slice(&entry.round.to_be_bytes());
    buf.push(entry.kind as u8);
    buf.extend_from_slice(&(entry.data.len() as u32).to_be_bytes());
    buf.extend_from_slice(&entry.data);
    // Checksum over everything before it
    let checksum = sha256(&buf);
    buf.extend_from_slice(&checksum.0);
    buf
}

/// Decode a WalEntry from bytes.
pub fn decode_wal_entry(bytes: &[u8]) -> Result<WalEntry, WalError> {
    if bytes.len() < 8 + 4 + 1 + 4 + 32 {
        return Err(WalError::CorruptEntry);
    }
    let height = u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    let round = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let kind = WalEntryKind::from_u8(bytes[12]).ok_or(WalError::CorruptEntry)?;
    let data_len = u32::from_be_bytes([bytes[13], bytes[14], bytes[15], bytes[16]]) as usize;

    let expected_total = 8 + 4 + 1 + 4 + data_len + 32;
    if bytes.len() != expected_total {
        return Err(WalError::CorruptEntry);
    }

    let data = bytes[17..17 + data_len].to_vec();

    // Verify checksum
    let payload = &bytes[..17 + data_len];
    let expected_checksum = sha256(payload);
    let mut actual = [0u8; 32];
    actual.copy_from_slice(&bytes[17 + data_len..]);
    if actual != expected_checksum.0 {
        return Err(WalError::ChecksumMismatch);
    }

    Ok(WalEntry { height, round, kind, data })
}

#[derive(thiserror::Error, Debug)]
pub enum WalError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("corrupt WAL entry")]
    CorruptEntry,
    #[error("checksum mismatch")]
    ChecksumMismatch,
}

/// Write-Ahead Log for consensus crash recovery (doc 9 section 5).
/// Each entry is written as a hex-encoded line for simplicity.
/// On recovery, replay entries from the WAL to reconstruct consensus state.
pub struct WAL {
    path: PathBuf,
    file: File,
}

impl WAL {
    /// Open or create a WAL file.
    pub fn open(path: &Path) -> Result<Self, WalError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
        })
    }

    /// Append an entry to the WAL. Flushes immediately for durability.
    pub fn write_entry(&mut self, entry: &WalEntry) -> Result<(), WalError> {
        let encoded = encode_wal_entry(entry);
        let hex_line = hex_encode(&encoded);
        writeln!(self.file, "{}", hex_line)?;
        self.file.flush()?;
        Ok(())
    }

    /// Read all entries from the WAL (for crash recovery).
    pub fn read_all(path: &Path) -> Result<Vec<WalEntry>, WalError> {
        let file = match File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(WalError::Io(e)),
        };

        let reader = BufReader::new(file);
        let mut entries = Vec::new();

        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match hex_decode(trimmed) {
                Some(bytes) => match decode_wal_entry(&bytes) {
                    Ok(entry) => entries.push(entry),
                    Err(_) => {
                        // Corrupt entry at end of WAL (crash during write) - stop here
                        break;
                    }
                },
                None => break, // corrupt hex
            }
        }

        Ok(entries)
    }

    /// Truncate the WAL (after a successful commit, old entries can be removed).
    pub fn truncate(&mut self) -> Result<(), WalError> {
        // Close and reopen with truncation
        self.file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)?;
        Ok(())
    }

    /// Truncate and keep only entries at or above the given height.
    pub fn truncate_below(&mut self, min_height: u64) -> Result<(), WalError> {
        let entries = WAL::read_all(&self.path)?;
        self.truncate()?;
        for entry in entries {
            if entry.height >= min_height {
                self.write_entry(&entry)?;
            }
        }
        Ok(())
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for i in (0..s.len()).step_by(2) {
        let byte = u8::from_str_radix(&s[i..i + 2], 16).ok()?;
        out.push(byte);
    }
    Some(out)
}
