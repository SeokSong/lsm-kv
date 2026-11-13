use super::*;
use crate::wal::{CorruptionReason, Op, WalWriter, encode};
use std::path::PathBuf;
use tempfile::TempDir;

// small records keep the every-truncation test fast, each torn cut does a real fsync
fn small_put(id: u8) -> Record {
    Record {
        key: vec![0x01, 0x00, 0x00, id],
        op: Op::Put {
            value: vec![id; 20],
        },
    }
}

fn small_delete(id: u8) -> Record {
    Record {
        key: vec![0x01, 0x00, 0x00, id],
        op: Op::Delete,
    }
}

fn write_records(path: &Path, records: &[Record]) {
    let mut writer: WalWriter = WalWriter::open(path).unwrap();
    for record in records {
        writer.append(record).unwrap();
    }
}

#[test]
fn replay_missing_file_returns_empty() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("wal.log");

    let records: Vec<Record> = replay(&path).unwrap();

    assert!(records.is_empty());
    // replay doesn't create the file, WalWriter::open does
    assert!(!path.exists());
}

#[test]
fn replay_empty_file_returns_empty() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("wal.log");
    fs::write(&path, []).unwrap();

    let records: Vec<Record> = replay(&path).unwrap();

    assert!(records.is_empty());
}

#[test]
fn replay_returns_records_in_order() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("wal.log");
    let written: Vec<Record> = vec![small_put(1), small_delete(2), small_put(3)];
    write_records(&path, &written);

    let records: Vec<Record> = replay(&path).unwrap();

    assert_eq!(records, written);
}

#[test]
fn replay_every_truncation() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("wal.log");
    let written: Vec<Record> = vec![small_put(1), small_delete(2), small_put(3)];

    // full WAL bytes, plus where each record ends
    let mut full_wal: Vec<u8> = Vec::new();
    let mut record_ends: Vec<usize> = Vec::new();
    for record in &written {
        full_wal.extend_from_slice(&encode(record).unwrap());
        record_ends.push(full_wal.len());
    }

    // every possible crash point, including the empty file and the full file
    for cut in 0..=full_wal.len() {
        fs::write(&path, &full_wal[..cut]).unwrap();

        let records: Vec<Record> = replay(&path).unwrap();

        // only records that ended at or before the cut come back
        let complete: usize = record_ends.iter().filter(|&&end| end <= cut).count();
        assert_eq!(records, written[..complete], "cut at {cut} bytes");

        // the file is cut back to the end of the last complete record
        let valid_len: usize = if complete == 0 {
            0
        } else {
            record_ends[complete - 1]
        };
        assert_eq!(
            fs::metadata(&path).unwrap().len() as usize,
            valid_len,
            "cut at {cut} bytes"
        );
    }
}

#[test]
fn append_after_torn_tail() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("wal.log");
    write_records(&path, &[small_put(1), small_put(2)]);

    // crash in the middle of a third append: only its first 5 bytes made it
    let torn_record: Vec<u8> = encode(&small_put(3)).unwrap();
    let mut wal_bytes: Vec<u8> = fs::read(&path).unwrap();
    wal_bytes.extend_from_slice(&torn_record[..5]);
    fs::write(&path, &wal_bytes).unwrap();

    // startup order: replay first, then open the writer
    let records: Vec<Record> = replay(&path).unwrap();
    assert_eq!(records, vec![small_put(1), small_put(2)]);

    write_records(&path, &[small_put(4)]);

    // without the truncate, record 4 would land after the torn bytes and look like corruption
    let records: Vec<Record> = replay(&path).unwrap();
    assert_eq!(records, vec![small_put(1), small_put(2), small_put(4)]);
}

#[test]
fn replay_corruption_reports_offset() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("wal.log");
    write_records(&path, &[small_put(1), small_put(2), small_put(3)]);

    // flip a bit in the value of the second record
    let first_len: usize = encode(&small_put(1)).unwrap().len();
    let mut wal_bytes: Vec<u8> = fs::read(&path).unwrap();
    wal_bytes[first_len + 20] ^= 0x01;
    fs::write(&path, &wal_bytes).unwrap();

    let result = replay(&path);

    assert!(matches!(
        result,
        Err(WalError::Corruption {
            offset,
            reason: CorruptionReason::CrcMismatch,
        }) if offset == first_len as u64
    ));

    // corruption is not a torn tail, the file is left untouched
    assert_eq!(fs::read(&path).unwrap(), wal_bytes);
}
