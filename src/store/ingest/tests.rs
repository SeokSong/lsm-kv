use super::*;
use crate::config::{Config, MemtableConfig, SstableConfig};
use crate::store::{Store, StoreError};
use crate::wal::{SizeField, WalError};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

// 12-byte telemetry key: namespace | session (4) | car (2) | lap (2) | sector | seq (2)
fn key(namespace: u8, session: u32, car: u16, lap: u16, sector: u8) -> Vec<u8> {
    let mut key: Vec<u8> = vec![namespace];
    key.extend_from_slice(&session.to_be_bytes());
    key.extend_from_slice(&car.to_be_bytes());
    key.extend_from_slice(&lap.to_be_bytes());
    key.push(sector);
    key.extend_from_slice(&1u16.to_be_bytes());
    key
}

// every chunk of a session in key order: cars x laps x 3 sectors
fn session_keys(namespace: u8, session: u32, cars: u16, laps: u16) -> Vec<Vec<u8>> {
    let mut keys: Vec<Vec<u8>> = Vec::new();
    for car in 1..=cars {
        for lap in 1..=laps {
            for sector in 1..=3 {
                keys.push(key(namespace, session, car, lap, sector));
            }
        }
    }
    keys
}

fn value_for(key: &[u8]) -> Vec<u8> {
    vec![key[8] ^ key[9], 100]
}

fn ingest_keys(store: &Store, keys: &[Vec<u8>]) -> Result<IngestInfo, StoreError> {
    let mut ingest: Ingest<'_> = store.begin_ingest()?;
    for key in keys {
        ingest.put(key.clone(), value_for(key))?;
    }
    ingest.commit()
}

fn assert_readable(store: &Store, keys: &[Vec<u8>]) {
    for key in keys {
        assert_eq!(store.get(key).unwrap(), Some(value_for(key)), "key {key:?}");
    }
}

fn assert_missing(store: &Store, keys: &[Vec<u8>]) {
    for key in keys {
        assert_eq!(store.get(key).unwrap(), None, "key {key:?}");
    }
}

// .sst and .tmp files in the store directory
fn sst_files(dir: &Path) -> usize {
    fs::read_dir(dir)
        .unwrap()
        .filter(|entry| {
            let name: String = entry.as_ref().unwrap().file_name().to_string_lossy().into();
            name.ends_with(".sst") || name.ends_with(".tmp")
        })
        .count()
}

fn manifest_of(store: &Store) -> Manifest {
    store.inner.manifest.lock().unwrap().clone()
}

#[test]
fn ingest_then_get_every_key() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    let keys: Vec<Vec<u8>> = session_keys(0x01, 9141, 20, 10);

    let info: IngestInfo = ingest_keys(&store, &keys).unwrap();

    assert_eq!(info.entries, 600);
    // one namespace, one session, far under the size target: one file
    assert_eq!(info.files, 1);
    assert_eq!(store.sstable_count(), 1);
    assert_eq!(store.stats().ingests, 1);
    assert_readable(&store, &keys);
    // nothing went through the WAL or the memtable
    assert!(store.inner.state.lock().unwrap().active.is_empty());
}

#[test]
fn one_seq_for_the_whole_ingest() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();

    store.put(key(0x01, 1, 1, 1, 1), vec![1]).unwrap();
    let info: IngestInfo = ingest_keys(&store, &session_keys(0x01, 2, 2, 2)).unwrap();
    store.put(key(0x01, 3, 1, 1, 1), vec![3]).unwrap();

    assert_eq!(info.seq, 2);
    assert_eq!(store.last_seq(), 3);
    let manifest: Manifest = manifest_of(&store);
    assert_eq!(manifest.sstables.len(), 1);
    assert_eq!(manifest.sstables[0].min_seq, 2);
    assert_eq!(manifest.sstables[0].max_seq, 2);
    assert!(manifest.last_seq >= 2);
}

#[test]
fn splits_by_namespace_and_session() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    let mut keys: Vec<Vec<u8>> = session_keys(0x01, 100, 2, 3);
    keys.extend(session_keys(0x01, 200, 2, 3));
    keys.extend(session_keys(0x02, 100, 2, 3));

    let info: IngestInfo = ingest_keys(&store, &keys).unwrap();

    assert_eq!(info.files, 3);
    for meta in &manifest_of(&store).sstables {
        // every file holds one namespace + session
        assert_eq!(meta.first_key[..5], meta.last_key[..5]);
    }
    assert_readable(&store, &keys);
}

#[test]
fn splits_at_the_target_file_size() {
    let dir: TempDir = TempDir::new().unwrap();
    let config: Config = Config {
        sstable: SstableConfig {
            target_file_bytes: 1_000,
            ..SstableConfig::default()
        },
        ..Config::default()
    };
    let store: Store = Store::open(dir.path(), config).unwrap();
    // 60 entries of 12-byte keys + 200-byte values below
    let keys: Vec<Vec<u8>> = session_keys(0x01, 1, 1, 20);

    let mut ingest: Ingest<'_> = store.begin_ingest().unwrap();
    for key in &keys {
        ingest.put(key.clone(), vec![7; 200]).unwrap();
    }
    let info: IngestInfo = ingest.commit().unwrap();

    // 212 bytes per entry, a new file after 5 entries reach 1,000 bytes
    assert_eq!(info.files, 12);
    let manifest: Manifest = manifest_of(&store);
    for pair in manifest.sstables.windows(2) {
        assert!(
            pair[0].last_key < pair[1].first_key,
            "files must not overlap"
        );
    }
    for key in &keys {
        assert_eq!(store.get(key).unwrap(), Some(vec![7; 200]));
    }
}

#[test]
fn split_prefix_len_zero_keeps_one_file() {
    let dir: TempDir = TempDir::new().unwrap();
    let config: Config = Config {
        sstable: SstableConfig {
            split_prefix_len: 0,
            ..SstableConfig::default()
        },
        ..Config::default()
    };
    let store: Store = Store::open(dir.path(), config).unwrap();
    let mut keys: Vec<Vec<u8>> = session_keys(0x01, 1, 1, 2);
    keys.extend(session_keys(0x02, 1, 1, 2));

    assert_eq!(ingest_keys(&store, &keys).unwrap().files, 1);
}

#[test]
fn ingest_survives_reopen_and_seqs_continue() {
    let dir: TempDir = TempDir::new().unwrap();
    let keys: Vec<Vec<u8>> = session_keys(0x01, 9141, 3, 4);
    let seq: u64 = {
        let store: Store = Store::open(dir.path(), Config::default()).unwrap();
        ingest_keys(&store, &keys).unwrap().seq
    };

    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    assert_readable(&store, &keys);
    store.put(key(0x01, 1, 1, 1, 1), vec![1]).unwrap();
    assert!(store.last_seq() > seq);
}

#[test]
fn overlap_with_the_active_memtable_is_refused() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    let keys: Vec<Vec<u8>> = session_keys(0x01, 5, 2, 2);
    // inside the session's range, not one of its keys
    store.put(key(0x01, 5, 1, 1, 9), vec![9]).unwrap();

    let result: Result<IngestInfo, StoreError> = ingest_keys(&store, &keys);

    assert!(matches!(
        result,
        Err(StoreError::Ingest(IngestError::Overlaps))
    ));
    assert_missing(&store, &keys);
    assert_eq!(sst_files(dir.path()), 0);
    assert_eq!(store.get(&key(0x01, 5, 1, 1, 9)).unwrap(), Some(vec![9]));
}

#[test]
fn overlap_with_a_frozen_memtable_is_refused() {
    let dir: TempDir = TempDir::new().unwrap();
    let config: Config = Config {
        memtable: MemtableConfig {
            flush_threshold_bytes: 100,
            max_frozen: 2,
        },
        ..Config::default()
    };
    // no flush thread: the frozen memtable stays frozen
    let store: Store = Store::open_with(dir.path(), config, false).unwrap();
    store.put(key(0x01, 5, 1, 1, 9), vec![9; 90]).unwrap();
    store.put(key(0x01, 6, 1, 1, 1), vec![1; 90]).unwrap();
    assert_eq!(store.inner.state.lock().unwrap().frozen.len(), 1);

    let result: Result<IngestInfo, StoreError> = ingest_keys(&store, &session_keys(0x01, 5, 2, 2));

    assert!(matches!(
        result,
        Err(StoreError::Ingest(IngestError::Overlaps))
    ));
    assert_eq!(sst_files(dir.path()), 0);
}

#[test]
fn importing_the_same_session_twice_is_refused() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    let keys: Vec<Vec<u8>> = session_keys(0x01, 9141, 2, 3);
    ingest_keys(&store, &keys).unwrap();

    let result: Result<IngestInfo, StoreError> = ingest_keys(&store, &keys);

    assert!(matches!(
        result,
        Err(StoreError::Ingest(IngestError::Overlaps))
    ));
    assert_eq!(store.sstable_count(), 1);
    assert_eq!(sst_files(dir.path()), 1);
    assert_readable(&store, &keys);
}

#[test]
fn data_next_to_the_range_is_not_an_overlap() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    // sessions 4 and 6 around session 5, in the memtable and in an SSTable
    store.put(key(0x01, 4, 9, 9, 3), vec![4]).unwrap();
    ingest_keys(&store, &session_keys(0x01, 6, 1, 1)).unwrap();

    let keys: Vec<Vec<u8>> = session_keys(0x01, 5, 2, 2);
    ingest_keys(&store, &keys).unwrap();

    assert_readable(&store, &keys);
    assert_eq!(store.get(&key(0x01, 4, 9, 9, 3)).unwrap(), Some(vec![4]));
}

#[test]
fn empty_ingest_is_refused() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();

    let result: Result<IngestInfo, StoreError> = store.begin_ingest().unwrap().commit();

    assert!(matches!(
        result,
        Err(StoreError::Ingest(IngestError::Empty))
    ));
    assert_eq!(store.stats().ingests, 0);
}

#[test]
fn out_of_order_key_aborts_the_ingest() {
    let dir: TempDir = TempDir::new().unwrap();
    let config: Config = Config {
        sstable: SstableConfig {
            target_file_bytes: 300,
            ..SstableConfig::default()
        },
        ..Config::default()
    };
    let store: Store = Store::open(dir.path(), config).unwrap();
    let keys: Vec<Vec<u8>> = session_keys(0x01, 1, 1, 10);

    let mut ingest: Ingest<'_> = store.begin_ingest().unwrap();
    for key in &keys {
        ingest.put(key.clone(), vec![1; 100]).unwrap();
    }
    // several files are written by now, then a key goes backwards
    assert!(sst_files(dir.path()) > 1);
    let result: Result<(), StoreError> = ingest.put(keys[0].clone(), vec![1]);
    assert!(matches!(
        result,
        Err(StoreError::Ingest(IngestError::OutOfOrder))
    ));

    // the same key twice is out of order too, and every later call is refused
    assert!(matches!(
        ingest.put(vec![0xFF; 12], vec![1]),
        Err(StoreError::Ingest(IngestError::Aborted))
    ));
    assert!(matches!(
        ingest.commit(),
        Err(StoreError::Ingest(IngestError::Aborted))
    ));
    assert_eq!(sst_files(dir.path()), 0);
    assert_missing(&store, &keys);
}

#[test]
fn duplicate_key_aborts_the_ingest() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();

    let mut ingest: Ingest<'_> = store.begin_ingest().unwrap();
    ingest.put(key(0x01, 1, 1, 1, 1), vec![1]).unwrap();
    assert!(matches!(
        ingest.put(key(0x01, 1, 1, 1, 1), vec![2]),
        Err(StoreError::Ingest(IngestError::OutOfOrder))
    ));
}

#[test]
fn oversized_key_or_value_aborts_the_ingest() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();

    let mut ingest: Ingest<'_> = store.begin_ingest().unwrap();
    ingest.put(key(0x01, 1, 1, 1, 1), vec![1]).unwrap();
    let result: Result<(), StoreError> = ingest.put(vec![0xFF; 17], vec![1]);
    assert!(matches!(
        result,
        Err(StoreError::Wal(WalError::Size {
            which_field: SizeField::Key,
            ..
        }))
    ));
    drop(ingest);

    let mut ingest: Ingest<'_> = store.begin_ingest().unwrap();
    let result: Result<(), StoreError> = ingest.put(key(0x01, 1, 1, 1, 1), vec![0; 65_537]);
    assert!(matches!(
        result,
        Err(StoreError::Wal(WalError::Size {
            which_field: SizeField::Value,
            ..
        }))
    ));
    drop(ingest);
    assert_eq!(sst_files(dir.path()), 0);
}

#[test]
fn dropping_an_ingest_leaves_nothing() {
    let dir: TempDir = TempDir::new().unwrap();
    let config: Config = Config {
        sstable: SstableConfig {
            target_file_bytes: 300,
            ..SstableConfig::default()
        },
        ..Config::default()
    };
    let store: Store = Store::open(dir.path(), config).unwrap();
    let keys: Vec<Vec<u8>> = session_keys(0x01, 1, 1, 10);

    let mut ingest: Ingest<'_> = store.begin_ingest().unwrap();
    for key in &keys {
        ingest.put(key.clone(), vec![1; 100]).unwrap();
    }
    assert!(sst_files(dir.path()) > 1);
    drop(ingest);

    assert_eq!(sst_files(dir.path()), 0);
    assert_eq!(store.sstable_count(), 0);
    assert!(manifest_of(&store).sstables.is_empty());
}

// a crash before commit: drop never runs. the files are unlisted, the next open deletes them
#[test]
fn crash_before_commit_leaves_no_trace_after_reopen() {
    let dir: TempDir = TempDir::new().unwrap();
    let config: Config = Config {
        sstable: SstableConfig {
            target_file_bytes: 300,
            ..SstableConfig::default()
        },
        ..Config::default()
    };
    let keys: Vec<Vec<u8>> = session_keys(0x01, 1, 1, 10);
    {
        let store: Store = Store::open(dir.path(), config.clone()).unwrap();
        let mut ingest: Ingest<'_> = store.begin_ingest().unwrap();
        for key in &keys {
            ingest.put(key.clone(), vec![1; 100]).unwrap();
        }
        std::mem::forget(ingest);
        // finished files and the half-written one (.tmp)
        assert!(sst_files(dir.path()) > 1);
    }

    let store: Store = Store::open(dir.path(), config).unwrap();
    assert_eq!(sst_files(dir.path()), 0);
    for key in &keys {
        assert_eq!(store.get(key).unwrap(), None);
    }
    // the same session can be ingested again
    ingest_keys(&store, &keys).unwrap();
    assert_readable(&store, &keys);
}

#[test]
fn puts_keep_going_during_an_ingest() {
    let dir: TempDir = TempDir::new().unwrap();
    let config: Config = Config {
        memtable: MemtableConfig {
            flush_threshold_bytes: 500,
            max_frozen: 1,
        },
        ..Config::default()
    };
    let store: Store = Store::open(dir.path(), config).unwrap();
    let keys: Vec<Vec<u8>> = session_keys(0x01, 9141, 4, 5);

    let mut ingest: Ingest<'_> = store.begin_ingest().unwrap();
    for (i, key) in keys.iter().enumerate() {
        ingest.put(key.clone(), value_for(key)).unwrap();
        // live data for another session, with swaps and flushes in between
        store
            .put(self::key(0x01, 1, 1, i as u16, 1), vec![5; 100])
            .unwrap();
    }
    let info: IngestInfo = ingest.commit().unwrap();

    assert_readable(&store, &keys);
    for i in 0..keys.len() {
        assert_eq!(
            store.get(&self::key(0x01, 1, 1, i as u16, 1)).unwrap(),
            Some(vec![5; 100])
        );
    }
    // every put after begin_ingest got a later seq
    assert_eq!(store.last_seq(), info.seq + keys.len() as u64);
}

#[test]
fn second_of_two_overlapping_ingests_is_refused() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    let keys: Vec<Vec<u8>> = session_keys(0x01, 9141, 2, 2);

    let mut first: Ingest<'_> = store.begin_ingest().unwrap();
    let mut second: Ingest<'_> = store.begin_ingest().unwrap();
    for key in &keys {
        first.put(key.clone(), value_for(key)).unwrap();
        second.put(key.clone(), vec![0]).unwrap();
    }
    first.commit().unwrap();

    assert!(matches!(
        second.commit(),
        Err(StoreError::Ingest(IngestError::Overlaps))
    ));
    assert_readable(&store, &keys);
    assert_eq!(sst_files(dir.path()), 1);
}

#[test]
fn failed_or_closed_store_refuses_an_ingest() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    store.inner.mark_failed();
    assert!(matches!(store.begin_ingest(), Err(StoreError::Failed)));

    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    let mut ingest: Ingest<'_> = store.begin_ingest().unwrap();
    ingest.put(key(0x01, 1, 1, 1, 1), vec![1]).unwrap();
    store.close().unwrap();
    assert!(matches!(ingest.commit(), Err(StoreError::Closed)));
    assert_eq!(sst_files(dir.path()), 0);
    assert!(matches!(store.begin_ingest(), Err(StoreError::Closed)));
}
