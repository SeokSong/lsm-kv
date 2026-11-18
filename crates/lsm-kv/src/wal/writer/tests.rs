use super::*;
use crate::wal::{Op, SizeField, decode};
use std::fs;
use tempfile::TempDir;

fn put_record() -> Record {
    let record_key: Vec<u8> = vec![
        0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05, 0x00, 0x0C, 0x03, 0x00, 0x01,
    ];

    Record {
        seq: 1,
        key: record_key,
        op: Op::Put {
            value: vec![0xAB; 2000],
        },
    }
}

fn delete_record() -> Record {
    let record_key: Vec<u8> = vec![
        0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05, 0x00, 0x0C, 0x03, 0x00, 0x01,
    ];

    Record {
        seq: 2,
        key: record_key,
        op: Op::Delete,
    }
}

// decodes every record in the file, failing the test on a torn tail or corruption
fn read_all_records(path: &Path) -> Vec<Record> {
    let buffer: Vec<u8> = fs::read(path).unwrap();
    let mut records: Vec<Record> = Vec::new();
    let mut position: usize = 0;

    while position < buffer.len() {
        let (record, bytes_used): (Record, usize) = decode(&buffer[position..]).unwrap().unwrap();
        records.push(record);
        position += bytes_used;
    }

    records
}

#[test]
fn append_creates_file_and_writes_record() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("wal.log");

    let mut writer: WalWriter = WalWriter::open(&path).unwrap();
    writer.append(&put_record()).unwrap();

    // the file holds exactly the encoded record, nothing more
    let file_bytes: Vec<u8> = fs::read(&path).unwrap();
    assert_eq!(file_bytes, encode(&put_record()).unwrap());
}

#[test]
fn append_multiple_records_in_order() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("wal.log");

    let mut writer: WalWriter = WalWriter::open(&path).unwrap();
    writer.append(&put_record()).unwrap();
    writer.append(&delete_record()).unwrap();
    writer.append(&put_record()).unwrap();

    let records: Vec<Record> = read_all_records(&path);
    assert_eq!(records, vec![put_record(), delete_record(), put_record()]);
}

#[test]
fn reopen_appends_to_end() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("wal.log");

    let mut writer: WalWriter = WalWriter::open(&path).unwrap();
    writer.append(&put_record()).unwrap();
    drop(writer);

    // reopening must not truncate or overwrite what is already there
    let mut writer: WalWriter = WalWriter::open(&path).unwrap();
    writer.append(&delete_record()).unwrap();

    let records: Vec<Record> = read_all_records(&path);
    assert_eq!(records, vec![put_record(), delete_record()]);
}

#[test]
fn append_too_big_key_writes_nothing() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("wal.log");

    let mut writer: WalWriter = WalWriter::open(&path).unwrap();

    let bad_record = Record {
        seq: 1,
        key: vec![0u8; 17],
        op: Op::Delete,
    };
    let result = writer.append(&bad_record);

    assert!(matches!(
        result,
        Err(WalError::Size {
            which_field: SizeField::Key,
            ..
        })
    ));
    assert_eq!(fs::read(&path).unwrap().len(), 0);

    // bad input is not a disk failure, so the writer still accepts good records
    writer.append(&put_record()).unwrap();
    assert_eq!(read_all_records(&path), vec![put_record()]);
}

#[test]
fn append_after_failure_is_refused() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("wal.log");

    let mut writer: WalWriter = WalWriter::open(&path).unwrap();

    // a real fsync failure can't be triggered on demand, so set the state it would leave behind
    writer.failed = true;
    let result = writer.append(&put_record());

    assert!(matches!(result, Err(WalError::Failed)));
    assert_eq!(fs::read(&path).unwrap().len(), 0);
}

#[test]
fn open_in_missing_directory_fails() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("does_not_exist").join("wal.log");

    let result = WalWriter::open(&path);

    assert!(matches!(result, Err(WalError::Io(_))));
}
