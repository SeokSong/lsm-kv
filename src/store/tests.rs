use super::*;
use crate::config::MemtableConfig;
use crate::wal::{CorruptionReason, SizeField, encode};
use tempfile::TempDir;

// 12-byte telemetry key from the design: namespace | session | car | lap | sector | seq
fn telemetry_key(lap: u16, sector: u8, seq: u16) -> Vec<u8> {
    let mut key: Vec<u8> = vec![0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05];
    key.extend_from_slice(&lap.to_be_bytes());
    key.push(sector);
    key.extend_from_slice(&seq.to_be_bytes());
    key
}

#[test]
fn open_creates_directory_and_wal() {
    let dir: TempDir = TempDir::new().unwrap();
    let store_dir: PathBuf = dir.path().join("data").join("lsm");

    let store: Store = Store::open(&store_dir, Config::default()).unwrap();

    assert_eq!(store.dir(), store_dir.as_path());
    assert!(store_dir.join(WAL_FILE_NAME).exists());
}

#[test]
fn put_then_get() {
    let dir: TempDir = TempDir::new().unwrap();
    let mut store: Store = Store::open(dir.path(), Config::default()).unwrap();
    let key: Vec<u8> = telemetry_key(12, 3, 1);

    store.put(key.clone(), vec![0xAB; 2000]).unwrap();

    assert_eq!(store.get(&key).unwrap(), Some(vec![0xAB; 2000]));
}

#[test]
fn get_missing_returns_none() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();

    assert_eq!(store.get(&telemetry_key(12, 3, 1)).unwrap(), None);
}

#[test]
fn delete_then_get_returns_none() {
    let dir: TempDir = TempDir::new().unwrap();
    let mut store: Store = Store::open(dir.path(), Config::default()).unwrap();
    let key: Vec<u8> = telemetry_key(12, 3, 1);

    store.put(key.clone(), vec![0xAB; 2000]).unwrap();
    store.delete(key.clone()).unwrap();

    assert_eq!(store.get(&key).unwrap(), None);
}

#[test]
fn overwrite_returns_newest() {
    let dir: TempDir = TempDir::new().unwrap();
    let mut store: Store = Store::open(dir.path(), Config::default()).unwrap();
    let key: Vec<u8> = telemetry_key(12, 3, 1);

    store.put(key.clone(), vec![0x01; 100]).unwrap();
    store.put(key.clone(), vec![0x02; 100]).unwrap();

    assert_eq!(store.get(&key).unwrap(), Some(vec![0x02; 100]));
}

#[test]
fn reopen_recovers_puts_and_deletes() {
    let dir: TempDir = TempDir::new().unwrap();
    let key_a: Vec<u8> = telemetry_key(12, 1, 1);
    let key_b: Vec<u8> = telemetry_key(12, 2, 1);
    let key_c: Vec<u8> = telemetry_key(12, 3, 1);

    let mut store: Store = Store::open(dir.path(), Config::default()).unwrap();
    store.put(key_a.clone(), vec![1; 50]).unwrap();
    store.put(key_b.clone(), vec![2; 50]).unwrap();
    store.put(key_a.clone(), vec![3; 50]).unwrap();
    store.delete(key_b.clone()).unwrap();
    store.put(key_c.clone(), vec![4; 50]).unwrap();
    drop(store);

    // a restart: the memtable is rebuilt from the WAL
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();

    assert_eq!(store.get(&key_a).unwrap(), Some(vec![3; 50]));
    assert_eq!(store.get(&key_b).unwrap(), None);
    assert_eq!(store.get(&key_c).unwrap(), Some(vec![4; 50]));
}

#[test]
fn reopen_after_torn_tail_keeps_working() {
    let dir: TempDir = TempDir::new().unwrap();
    let key_a: Vec<u8> = telemetry_key(12, 1, 1);
    let key_b: Vec<u8> = telemetry_key(12, 2, 1);

    let mut store: Store = Store::open(dir.path(), Config::default()).unwrap();
    store.put(key_a.clone(), vec![1; 50]).unwrap();
    drop(store);

    // crash in the middle of the next append: half a record at the end of the WAL
    let wal_path: PathBuf = dir.path().join(WAL_FILE_NAME);
    let torn_record: Vec<u8> = encode(&Record {
        key: key_b.clone(),
        op: Op::Put { value: vec![2; 50] },
    })
    .unwrap();
    let mut wal_bytes: Vec<u8> = fs::read(&wal_path).unwrap();
    wal_bytes.extend_from_slice(&torn_record[..20]);
    fs::write(&wal_path, &wal_bytes).unwrap();

    // the acked write survives, the torn one never happened
    let mut store: Store = Store::open(dir.path(), Config::default()).unwrap();
    assert_eq!(store.get(&key_a).unwrap(), Some(vec![1; 50]));
    assert_eq!(store.get(&key_b).unwrap(), None);

    // new writes after recovery are durable too
    store.put(key_b.clone(), vec![5; 50]).unwrap();
    drop(store);
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    assert_eq!(store.get(&key_a).unwrap(), Some(vec![1; 50]));
    assert_eq!(store.get(&key_b).unwrap(), Some(vec![5; 50]));
}

#[test]
fn reopen_with_corrupt_wal_fails() {
    let dir: TempDir = TempDir::new().unwrap();

    let mut store: Store = Store::open(dir.path(), Config::default()).unwrap();
    store.put(telemetry_key(12, 1, 1), vec![1; 50]).unwrap();
    store.put(telemetry_key(12, 2, 1), vec![2; 50]).unwrap();
    drop(store);

    // flip a bit in the first record's value, a valid record follows it
    let wal_path: PathBuf = dir.path().join(WAL_FILE_NAME);
    let mut wal_bytes: Vec<u8> = fs::read(&wal_path).unwrap();
    wal_bytes[30] ^= 0x01;
    fs::write(&wal_path, &wal_bytes).unwrap();

    let result = Store::open(dir.path(), Config::default());

    assert!(matches!(
        result,
        Err(WalError::Corruption {
            offset: 0,
            reason: CorruptionReason::CrcMismatch,
        })
    ));
}

#[test]
fn rejected_put_changes_nothing() {
    let dir: TempDir = TempDir::new().unwrap();
    let mut store: Store = Store::open(dir.path(), Config::default()).unwrap();
    let too_long_key: Vec<u8> = vec![0u8; 17];

    let result = store.put(too_long_key.clone(), vec![1; 50]);

    assert!(matches!(
        result,
        Err(WalError::Size {
            which_field: SizeField::Key,
            ..
        })
    ));
    // WAL first: the failed append never reached the memtable
    assert_eq!(store.get(&too_long_key).unwrap(), None);

    // and the store still accepts good writes
    store.put(telemetry_key(12, 1, 1), vec![1; 50]).unwrap();
    assert_eq!(
        store.get(&telemetry_key(12, 1, 1)).unwrap(),
        Some(vec![1; 50])
    );
}

#[test]
fn memtable_full_uses_config_threshold() {
    let dir: TempDir = TempDir::new().unwrap();
    let config: Config = Config {
        memtable: MemtableConfig {
            flush_threshold_bytes: 200,
            max_frozen: 1,
        },
        ..Config::default()
    };
    let mut store: Store = Store::open(dir.path(), config).unwrap();

    // 12 + 100 = 112 bytes, under 200
    store.put(telemetry_key(12, 1, 1), vec![1; 100]).unwrap();
    assert!(!store.memtable_full());

    // 112 + 112 = 224 bytes, at or over 200
    store.put(telemetry_key(12, 2, 1), vec![2; 100]).unwrap();
    assert!(store.memtable_full());
}
