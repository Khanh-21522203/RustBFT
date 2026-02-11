//! WAL unit tests (doc 13 section 2.2 - storage).

use RustBFT::storage::wal::{WAL, WalEntry, WalEntryKind};
use std::path::Path;

fn temp_wal_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("rustbft_test_wal");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{}.wal", name))
}

#[test]
fn test_wal_write_and_read() {
    let path = temp_wal_path("write_read");
    let _ = std::fs::remove_file(&path);

    let mut wal = WAL::open(&path).unwrap();

    let entry1 = WalEntry {
        height: 1,
        round: 0,
        kind: WalEntryKind::Proposal,
        data: vec![1, 2, 3, 4],
    };
    let entry2 = WalEntry {
        height: 1,
        round: 0,
        kind: WalEntryKind::Prevote,
        data: vec![5, 6, 7],
    };

    wal.write_entry(&entry1).unwrap();
    wal.write_entry(&entry2).unwrap();
    drop(wal);

    let entries = WAL::read_all(&path).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].height, 1);
    assert_eq!(entries[0].kind, WalEntryKind::Proposal);
    assert_eq!(entries[0].data, vec![1, 2, 3, 4]);
    assert_eq!(entries[1].kind, WalEntryKind::Prevote);
    assert_eq!(entries[1].data, vec![5, 6, 7]);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_wal_truncate() {
    let path = temp_wal_path("truncate");
    let _ = std::fs::remove_file(&path);

    let mut wal = WAL::open(&path).unwrap();
    wal.write_entry(&WalEntry {
        height: 1, round: 0, kind: WalEntryKind::Proposal, data: vec![1],
    }).unwrap();
    wal.write_entry(&WalEntry {
        height: 2, round: 0, kind: WalEntryKind::Proposal, data: vec![2],
    }).unwrap();

    wal.truncate().unwrap();

    let entries = WAL::read_all(&path).unwrap();
    assert_eq!(entries.len(), 0, "truncate should clear all entries");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_wal_truncate_below() {
    let path = temp_wal_path("truncate_below");
    let _ = std::fs::remove_file(&path);

    let mut wal = WAL::open(&path).unwrap();
    for h in 1..=5 {
        wal.write_entry(&WalEntry {
            height: h, round: 0, kind: WalEntryKind::Proposal, data: vec![h as u8],
        }).unwrap();
    }

    wal.truncate_below(3).unwrap();

    let entries = WAL::read_all(&path).unwrap();
    assert_eq!(entries.len(), 3, "should keep entries at height >= 3");
    assert_eq!(entries[0].height, 3);
    assert_eq!(entries[1].height, 4);
    assert_eq!(entries[2].height, 5);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_wal_read_nonexistent() {
    let path = Path::new("/tmp/rustbft_test_wal_nonexistent_12345.wal");
    let entries = WAL::read_all(path).unwrap();
    assert_eq!(entries.len(), 0, "reading nonexistent WAL should return empty");
}

#[test]
fn test_wal_entry_encode_decode_roundtrip() {
    let entry = WalEntry {
        height: 42,
        round: 3,
        kind: WalEntryKind::Precommit,
        data: vec![10, 20, 30, 40, 50],
    };

    let encoded = entry.encode();
    let decoded = WalEntry::decode(&encoded).unwrap();

    assert_eq!(decoded.height, 42);
    assert_eq!(decoded.round, 3);
    assert_eq!(decoded.kind, WalEntryKind::Precommit);
    assert_eq!(decoded.data, vec![10, 20, 30, 40, 50]);
}

#[test]
fn test_wal_entry_corrupt_checksum() {
    let entry = WalEntry {
        height: 1, round: 0, kind: WalEntryKind::Proposal, data: vec![1, 2, 3],
    };
    let mut encoded = entry.encode();
    // Corrupt the last byte (checksum)
    let len = encoded.len();
    encoded[len - 1] ^= 0xFF;

    let result = WalEntry::decode(&encoded);
    assert!(result.is_err(), "corrupt checksum should fail decode");
}

#[test]
fn test_wal_entry_kind_roundtrip() {
    let kinds = [
        WalEntryKind::Proposal,
        WalEntryKind::Prevote,
        WalEntryKind::Precommit,
        WalEntryKind::Timeout,
        WalEntryKind::RoundStep,
    ];
    for kind in &kinds {
        let byte = *kind as u8;
        let decoded = WalEntryKind::from_u8(byte).unwrap();
        assert_eq!(*kind, decoded);
    }
    assert!(WalEntryKind::from_u8(0xFF).is_none());
}
