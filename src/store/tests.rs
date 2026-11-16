use super::*;
use crate::config::{FlushConfig, MemtableConfig};
use crate::manifest::MANIFEST_FILE_NAME;
use crate::wal::{CorruptionReason, SizeField, WalError, encode};
use std::sync::mpsc;
use tempfile::TempDir;

// 12-byte telemetry key from the design: namespace | session | car | lap | sector | seq
fn telemetry_key(lap: u16, sector: u8, seq: u16) -> Vec<u8> {
    let mut key: Vec<u8> = vec![0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05];
    key.extend_from_slice(&lap.to_be_bytes());
    key.push(sector);
    key.extend_from_slice(&seq.to_be_bytes());
    key
}

// i-th key in key order: lap 1 sector 1, lap 1 sector 2, ...
fn nth_key(i: usize) -> Vec<u8> {
    telemetry_key((i / 3) as u16 + 1, (i % 3) as u8 + 1, 1)
}

// tiny flush threshold so a few puts make SSTables, 1 ms retry wait so retry tests are fast
fn small_config(flush_threshold_bytes: usize) -> Config {
    Config {
        memtable: MemtableConfig {
            flush_threshold_bytes,
            max_frozen: 1,
        },
        flush: FlushConfig {
            max_attempts: 3,
            retry_backoff_ms: 1,
        },
        ..Config::default()
    }
}

// every file name in the store directory, sorted
fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn wal_count(dir: &Path) -> usize {
    files_in(dir)
        .iter()
        .filter(|name| name.ends_with(".wal"))
        .count()
}

fn sst_count(dir: &Path) -> usize {
    files_in(dir)
        .iter()
        .filter(|name| name.ends_with(".sst"))
        .count()
}

// 12-byte key + 100-byte value = 112 bytes each
fn put_keys(store: &Store, keys: std::ops::Range<usize>) {
    for i in keys {
        store.put(nth_key(i), vec![i as u8; 100]).unwrap();
    }
}

fn assert_keys(store: &Store, keys: std::ops::Range<usize>) {
    for i in keys {
        assert_eq!(
            store.get(&nth_key(i)).unwrap(),
            Some(vec![i as u8; 100]),
            "key {i}"
        );
    }
}

// runs f on its own thread and fails the test if it takes longer than the timeout,
// so a deadlock fails the test instead of freezing CI
fn within<T: Send + 'static>(seconds: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(f());
    });
    receiver
        .recv_timeout(Duration::from_secs(seconds))
        .expect("timed out, deadlock?")
}

// test-only access to the store's insides
impl Store {
    // no flush thread: tests run the flush steps by hand
    fn open_without_flush_thread(dir: &Path, config: Config) -> Store {
        Store::open_with(dir, config, false).unwrap()
    }

    fn swap_memtable(&self) {
        let mut wal: MutexGuard<'_, WalState> = self.inner.wal.lock().unwrap();
        self.inner.swap(&mut wal).unwrap();
    }

    fn flush_oldest_until(&self, step: FlushStep) {
        let frozen: Arc<Frozen> = self.inner.state.lock().unwrap().frozen[0].frozen.clone();
        if self.inner.run_flush(&frozen, step).is_err() {
            panic!("flush failed");
        }
    }

    // until the frozen queue is empty or the store failed
    fn wait_for_flushes(&self) {
        let mut state: MutexGuard<'_, State> = self.inner.state.lock().unwrap();
        while !state.frozen.is_empty() && !state.failed {
            state = self
                .inner
                .state_changed
                .wait_timeout(state, Duration::from_millis(10))
                .unwrap()
                .0;
        }
    }

    fn frozen_count(&self) -> usize {
        self.inner.state.lock().unwrap().frozen.len()
    }

    fn active_len(&self) -> usize {
        self.inner.state.lock().unwrap().active.len()
    }

    fn active_size(&self) -> usize {
        self.inner.state.lock().unwrap().active.approximate_size()
    }

    fn is_failed(&self) -> bool {
        self.inner.state.lock().unwrap().failed
    }

    fn manifest(&self) -> Manifest {
        self.inner.manifest.lock().unwrap().clone()
    }

    fn active_segment(&self) -> u64 {
        self.inner.wal.lock().unwrap().segment
    }

    // entries per SSTable, in the order a get searches them
    fn sst_entry_counts(&self) -> Vec<usize> {
        let state: MutexGuard<'_, State> = self.inner.state.lock().unwrap();
        state
            .sstables
            .iter()
            .map(|sst| sst.reader.iter().count())
            .collect()
    }

    fn pause_flush(&self, paused: bool) {
        self.inner.hooks.pause_flush.store(paused, Ordering::SeqCst);
    }

    fn fail_next_flushes(&self, count: u32) {
        self.inner.hooks.fail_flushes.store(count, Ordering::SeqCst);
    }
}

#[test]
fn open_creates_directory_manifest_and_segment() {
    let dir: TempDir = TempDir::new().unwrap();
    let store_dir: PathBuf = dir.path().join("data").join("lsm");

    let store: Store = Store::open(&store_dir, Config::default()).unwrap();

    assert_eq!(store.dir(), store_dir.as_path());
    assert_eq!(files_in(&store_dir), vec!["000001.wal", "MANIFEST"]);
}

#[test]
fn put_then_get() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
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
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    let key: Vec<u8> = telemetry_key(12, 3, 1);

    store.put(key.clone(), vec![0xAB; 2000]).unwrap();
    store.delete(key.clone()).unwrap();

    assert_eq!(store.get(&key).unwrap(), None);
}

#[test]
fn overwrite_returns_newest() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
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

    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
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

    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    store.put(key_a.clone(), vec![1; 50]).unwrap();
    drop(store);

    // crash in the middle of the next append: half a record at the end of the WAL
    let wal_path: PathBuf = dir.path().join(wal_file_name(1));
    let torn_record: Vec<u8> = encode(&Record {
        seq: 2,
        key: key_b.clone(),
        op: Op::Put { value: vec![2; 50] },
    })
    .unwrap();
    let mut wal_bytes: Vec<u8> = fs::read(&wal_path).unwrap();
    wal_bytes.extend_from_slice(&torn_record[..20]);
    fs::write(&wal_path, &wal_bytes).unwrap();

    // the acked write survives, the torn one never happened
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
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

    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    store.put(telemetry_key(12, 1, 1), vec![1; 50]).unwrap();
    store.put(telemetry_key(12, 2, 1), vec![2; 50]).unwrap();
    drop(store);

    // flip a bit in the first record's value, a valid record follows it
    let wal_path: PathBuf = dir.path().join(wal_file_name(1));
    let mut wal_bytes: Vec<u8> = fs::read(&wal_path).unwrap();
    wal_bytes[30] ^= 0x01;
    fs::write(&wal_path, &wal_bytes).unwrap();

    let result = Store::open(dir.path(), Config::default());

    assert!(matches!(
        result,
        Err(StoreError::Wal(WalError::Corruption {
            offset: 0,
            reason: CorruptionReason::CrcMismatch,
        }))
    ));
}

#[test]
fn rejected_put_changes_nothing() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    let too_long_key: Vec<u8> = vec![0u8; 17];

    let result = store.put(too_long_key.clone(), vec![1; 50]);

    assert!(matches!(
        result,
        Err(StoreError::Wal(WalError::Size {
            which_field: SizeField::Key,
            ..
        }))
    ));
    // WAL first: the failed append never reached the memtable
    assert_eq!(store.get(&too_long_key).unwrap(), None);
    // and it didn't use up a seq
    assert_eq!(store.last_seq(), 0);

    // and the store still accepts good writes
    store.put(telemetry_key(12, 1, 1), vec![1; 50]).unwrap();
    assert_eq!(
        store.get(&telemetry_key(12, 1, 1)).unwrap(),
        Some(vec![1; 50])
    );
}

#[test]
fn seqs_count_up_from_one() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    assert_eq!(store.last_seq(), 0);

    store.put(telemetry_key(12, 1, 1), vec![1; 50]).unwrap();
    store.delete(telemetry_key(12, 1, 1)).unwrap();
    store.put(telemetry_key(12, 2, 1), vec![2; 50]).unwrap();

    // deletes use a seq too
    assert_eq!(store.last_seq(), 3);
    let seqs: Vec<u64> = wal::replay(&dir.path().join(wal_file_name(1)))
        .unwrap()
        .iter()
        .map(|record| record.seq)
        .collect();
    assert_eq!(seqs, vec![1, 2, 3]);
}

#[test]
fn reopen_continues_seqs() {
    let dir: TempDir = TempDir::new().unwrap();

    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    store.put(telemetry_key(12, 1, 1), vec![1; 50]).unwrap();
    store.put(telemetry_key(12, 2, 1), vec![2; 50]).unwrap();
    drop(store);

    // without this, the first write after a restart would reuse seq 1 and replay would
    // reject the whole WAL as out of order
    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    assert_eq!(store.last_seq(), 2);
    store.put(telemetry_key(12, 3, 1), vec![3; 50]).unwrap();
    assert_eq!(store.last_seq(), 3);
    drop(store);

    let store: Store = Store::open(dir.path(), Config::default()).unwrap();
    assert_eq!(store.last_seq(), 3);
    assert_eq!(
        store.get(&telemetry_key(12, 3, 1)).unwrap(),
        Some(vec![3; 50])
    );
}

#[test]
fn swap_happens_before_the_put_that_would_overflow() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();

    // 112 + 112 = 224 bytes, under 300
    put_keys(&store, 0..2);
    assert_eq!(store.sstable_count(), 0);

    // 224 + 112 = 336 would go over: the first two are flushed, the third starts a new memtable
    put_keys(&store, 2..3);
    store.wait_for_flushes();

    assert_eq!(store.sst_entry_counts(), vec![2]);
    assert_eq!(store.active_len(), 1);
    assert!(store.active_size() <= 300);
    assert_keys(&store, 0..3);
}

#[test]
fn empty_memtable_takes_an_oversized_put() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), small_config(50)).unwrap();

    // 112 bytes into a 50-byte memtable: goes in anyway, no endless swapping
    put_keys(&store, 0..1);
    assert_eq!(store.sstable_count(), 0);

    // the next one swaps the full memtable out first
    put_keys(&store, 1..2);
    store.wait_for_flushes();
    assert_eq!(store.sstable_count(), 1);
    assert_keys(&store, 0..2);
}

#[test]
fn rejected_put_does_not_trigger_a_flush() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();
    put_keys(&store, 0..2);

    // would overflow the memtable, but the key is too long: rejected before the swap
    let result = store.put(vec![0u8; 17], vec![1; 100]);

    assert!(matches!(
        result,
        Err(StoreError::Wal(WalError::Size { .. }))
    ));
    assert_eq!(store.sstable_count(), 0);
    assert_eq!(wal_count(dir.path()), 1);
}

#[test]
fn flush_leaves_one_segment_and_the_sstables() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();

    // 2 keys per memtable: 10 puts = 4 flushes + 2 keys still in the memtable
    put_keys(&store, 0..10);
    store.wait_for_flushes();

    assert_eq!(store.sstable_count(), 4);
    assert_eq!(store.stats().flushes, 4);
    assert_eq!(wal_count(dir.path()), 1);
    assert_eq!(sst_count(dir.path()), 4);
    assert_eq!(store.manifest().wal_segments, vec![store.active_segment()]);
    assert_eq!(store.manifest().sstables.len(), 4);
    assert_keys(&store, 0..10);
}

#[test]
fn newer_writes_hide_older_sstables() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();
    put_keys(&store, 0..6);
    store.wait_for_flushes();
    assert_eq!(store.sstable_count(), 2);

    // key 0 is in the oldest SSTable. overwrite it, delete key 1, both flushed to a newer SSTable
    store.put(nth_key(0), vec![0xEE; 100]).unwrap();
    store.delete(nth_key(1)).unwrap();
    // key 2 overwritten, still only in the memtable
    store.put(nth_key(2), vec![0xDD; 100]).unwrap();

    let check = |store: &Store| {
        assert_eq!(store.get(&nth_key(0)).unwrap(), Some(vec![0xEE; 100]));
        // the tombstone hides the older SSTable's value
        assert_eq!(store.get(&nth_key(1)).unwrap(), None);
        assert_eq!(store.get(&nth_key(2)).unwrap(), Some(vec![0xDD; 100]));
        assert_keys(store, 3..6);
    };
    check(&store);
    store.wait_for_flushes();
    check(&store);
    assert!(store.sstable_count() >= 3);

    // same answers after a restart, when everything comes from disk
    drop(store);
    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();
    check(&store);
}

#[test]
fn reopen_reads_sstables_and_continues_seqs() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();
    put_keys(&store, 0..10);
    drop(store);

    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();

    assert_eq!(store.sstable_count(), 4);
    assert_eq!(store.last_seq(), 10);
    assert_keys(&store, 0..10);

    put_keys(&store, 10..11);
    assert_eq!(store.last_seq(), 11);
}

#[test]
fn seqs_continue_when_every_segment_was_flushed() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();
    put_keys(&store, 0..2);
    // swap by hand: now the only live segment is empty and every write ends up in an SSTable
    store.swap_memtable();
    drop(store);

    // next seq has to come from the manifest's last_seq, the WAL has nothing
    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();
    assert_eq!(store.last_seq(), 2);
}

// a crash in the middle of a flush: fill one memtable, swap it out, run the flush up to
// some step by hand (no flush thread), drop the store (the "crash"), reopen
fn crash_during_flush(dir: &Path, stop_after: Option<FlushStep>) -> Crash {
    let store: Store = Store::open_without_flush_thread(dir, small_config(10_000));
    put_keys(&store, 0..5);
    store.swap_memtable();
    put_keys(&store, 5..7);
    if let Some(step) = stop_after {
        store.flush_oldest_until(step);
    }
    drop(store);

    // what was on disk at the crash, before open cleans anything up
    let files: Vec<String> = files_in(dir);
    let manifest: Manifest = Manifest::load(dir).unwrap().unwrap();
    let store: Store = Store::open(dir, small_config(10_000)).unwrap();
    Crash {
        files,
        manifest,
        store,
    }
}

struct Crash {
    files: Vec<String>,
    manifest: Manifest,
    store: Store,
}

// after any crash: nothing lost, one SSTable, one live segment, no garbage left
fn assert_recovered(dir: &Path, store: &Store) {
    assert_keys(store, 0..7);
    assert_eq!(store.last_seq(), 7);
    assert_eq!(store.sstable_count(), 1);
    assert_eq!(files_in(dir).len(), 3, "{:?}", files_in(dir));
    assert_eq!(wal_count(dir), 1);
    assert_eq!(sst_count(dir), 1);
}

#[test]
fn crash_before_flush_starts() {
    let dir: TempDir = TempDir::new().unwrap();

    // two live segments at startup: the older one is flushed at open, the newer stays active
    let crash: Crash = crash_during_flush(dir.path(), None);

    assert_eq!(crash.files, vec!["000001.wal", "000002.wal", "MANIFEST"]);
    assert_eq!(crash.manifest.wal_segments, vec![1, 2]);
    assert_recovered(dir.path(), &crash.store);
    assert_eq!(crash.store.active_len(), 2);
}

#[test]
fn crash_after_sstable_written() {
    let dir: TempDir = TempDir::new().unwrap();

    // the SSTable isn't in the manifest: deleted as an orphan, the segment is replayed and flushed again
    let crash: Crash = crash_during_flush(dir.path(), Some(FlushStep::WriteSstable));

    assert_eq!(
        crash.files,
        vec!["000001.wal", "000002.wal", "000003.sst", "MANIFEST"]
    );
    assert_eq!(crash.manifest.wal_segments, vec![1, 2]);
    assert!(crash.manifest.sstables.is_empty());
    assert_recovered(dir.path(), &crash.store);
}

#[test]
fn crash_after_manifest_saved() {
    let dir: TempDir = TempDir::new().unwrap();

    // the old segment is still on disk but unlisted: deleted, not replayed
    let crash: Crash = crash_during_flush(dir.path(), Some(FlushStep::SaveManifest));

    // same files as a crash after step 1, the difference is what the manifest says
    assert_eq!(
        crash.files,
        vec!["000001.wal", "000002.wal", "000003.sst", "MANIFEST"]
    );
    assert_eq!(crash.manifest.wal_segments, vec![2]);
    assert_eq!(crash.manifest.sstables[0].file_number, 3);
    assert_recovered(dir.path(), &crash.store);
}

#[test]
fn crash_after_segment_deleted() {
    let dir: TempDir = TempDir::new().unwrap();

    let crash: Crash = crash_during_flush(dir.path(), Some(FlushStep::DeleteSegment));

    assert_eq!(crash.files, vec!["000002.wal", "000003.sst", "MANIFEST"]);
    assert_recovered(dir.path(), &crash.store);
}

#[test]
fn reads_see_a_frozen_memtable_mid_flush() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open_without_flush_thread(dir.path(), small_config(10_000));
    put_keys(&store, 0..5);
    store.swap_memtable();

    // before step 4 the data is only in the frozen memtable (and an SSTable reads don't use yet)
    store.flush_oldest_until(FlushStep::DeleteSegment);
    assert_eq!(store.sstable_count(), 0);
    assert_eq!(store.frozen_count(), 1);
    assert_keys(&store, 0..5);
}

#[test]
fn failed_flush_is_retried() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();
    store.fail_next_flushes(2);

    // swaps, the flush fails twice, the third attempt works
    put_keys(&store, 0..3);
    store.wait_for_flushes();

    assert!(!store.is_failed());
    assert_eq!(store.stats().failed_flush_attempts, 2);
    assert_eq!(store.stats().flushes, 1);
    assert_eq!(store.sstable_count(), 1);
    // the failed attempts left nothing behind
    assert_eq!(sst_count(dir.path()), 1);
    assert_eq!(files_in(dir.path()).len(), 3, "{:?}", files_in(dir.path()));
    assert_keys(&store, 0..3);
}

#[test]
fn store_fails_after_max_flush_attempts() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();
    store.fail_next_flushes(3);

    // the put that swaps still works: the flush runs in the background
    put_keys(&store, 0..3);
    store.wait_for_flushes();

    assert!(store.is_failed());
    assert_eq!(store.stats().failed_flush_attempts, 3);
    // no more writes, but reads still work, the first two from the frozen memtable
    assert!(matches!(
        store.put(nth_key(3), vec![3; 100]),
        Err(StoreError::Failed)
    ));
    assert_keys(&store, 0..3);
    // close hands back the error that failed the store
    assert!(matches!(store.close(), Err(StoreError::Io(_))));
    drop(store);

    // a restart replays both segments, nothing acked is lost and writes work again
    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();
    assert_keys(&store, 0..3);
    put_keys(&store, 3..5);
    assert_keys(&store, 0..5);
}

#[test]
fn missing_manifest_with_store_files_fails_open() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();
    put_keys(&store, 0..5);
    drop(store);
    fs::remove_file(dir.path().join(MANIFEST_FILE_NAME)).unwrap();
    let files_before: Vec<String> = files_in(dir.path());

    let result = Store::open(dir.path(), small_config(300));

    // refuses instead of treating it as a new store and deleting every file
    assert!(matches!(result, Err(StoreError::MissingManifest)));
    assert_eq!(files_in(dir.path()), files_before);
}

#[test]
fn old_single_wal_file_without_manifest_fails_open() {
    let dir: TempDir = TempDir::new().unwrap();
    fs::write(dir.path().join("wal.log"), b"old data").unwrap();

    assert!(matches!(
        Store::open(dir.path(), Config::default()),
        Err(StoreError::MissingManifest)
    ));
}

#[test]
fn leftover_tmp_from_first_open_is_cleaned_up() {
    let dir: TempDir = TempDir::new().unwrap();
    // crash during the very first manifest save
    fs::write(dir.path().join("MANIFEST.tmp"), b"half").unwrap();

    let store: Store = Store::open(dir.path(), Config::default()).unwrap();

    assert_eq!(files_in(dir.path()), vec!["000001.wal", "MANIFEST"]);
    assert_eq!(store.last_seq(), 0);
}

#[test]
fn sstable_with_the_wrong_size_fails_open() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();
    put_keys(&store, 0..3);
    store.wait_for_flushes();
    let file_number: u64 = store.manifest().sstables[0].file_number;
    drop(store);

    let path: PathBuf = dir.path().join(sst_file_name(file_number));
    let mut bytes: Vec<u8> = fs::read(&path).unwrap();
    bytes.pop();
    fs::write(&path, &bytes).unwrap();

    assert!(matches!(
        Store::open(dir.path(), small_config(300)),
        Err(StoreError::SstSizeMismatch { file_number: n, .. }) if n == file_number
    ));
}

#[test]
fn unlisted_files_are_deleted_at_open() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();
    put_keys(&store, 0..3);
    drop(store);
    let live_files: Vec<String> = files_in(dir.path());

    // garbage a crash could leave behind, with numbers the store won't hand out again soon
    for name in ["000050.sst", "000051.wal", "000052.sst.tmp", "MANIFEST.tmp"] {
        fs::write(dir.path().join(name), b"x").unwrap();
    }
    // not a store file: left alone
    fs::write(dir.path().join("notes.txt"), b"x").unwrap();

    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();

    let mut expected: Vec<String> = live_files;
    expected.push("notes.txt".to_string());
    expected.sort();
    assert_eq!(files_in(dir.path()), expected);
    assert_keys(&store, 0..3);
}

// ---- PR 3b: background flush and sharing across threads ----

#[test]
fn store_is_send_and_sync() {
    // compile-time check: Arc<Store> can be handed to other threads
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Store>();
}

#[test]
fn flush_runs_in_the_background() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();
    store.pause_flush(true);

    // the put that swaps returns right away, it doesn't wait for the flush
    put_keys(&store, 0..3);
    assert_eq!(store.frozen_count(), 1);
    assert_eq!(store.sstable_count(), 0);
    // the frozen memtable is readable while it waits
    assert_keys(&store, 0..3);

    store.pause_flush(false);
    store.wait_for_flushes();
    assert_eq!(store.sstable_count(), 1);
    assert_eq!(store.frozen_count(), 0);
    assert_keys(&store, 0..3);
}

#[test]
fn put_waits_while_the_queue_is_full_and_reads_keep_going() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Arc<Store> = Arc::new(Store::open(dir.path(), small_config(300)).unwrap());
    store.pause_flush(true);
    // fills the active memtable, swaps it into the queue (1 of max 1), fills the next one
    put_keys(&store, 0..4);
    assert_eq!(store.frozen_count(), 1);

    // this put needs another swap, but there's no room in the queue
    let (done_sender, done_receiver) = mpsc::channel();
    let writer_store: Arc<Store> = store.clone();
    let writer = thread::spawn(move || {
        writer_store.put(nth_key(4), vec![4; 100]).unwrap();
        done_sender.send(()).unwrap();
    });

    assert!(
        done_receiver
            .recv_timeout(Duration::from_millis(200))
            .is_err(),
        "put should wait for queue space"
    );
    // a waiting writer holds only the WAL lock: reads are not blocked
    let reader_store: Arc<Store> = store.clone();
    within(5, move || assert_keys(&reader_store, 0..4));

    // the flush frees the slot and the waiting put goes through
    store.pause_flush(false);
    done_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("put never woke up");
    writer.join().unwrap();

    store.wait_for_flushes();
    assert_eq!(store.stats().stalls, 1);
    assert!(store.stats().stall_time >= Duration::from_millis(200));
    assert_keys(&store, 0..5);
}

#[test]
fn waiting_put_wakes_up_when_the_store_fails() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Arc<Store> = Arc::new(Store::open(dir.path(), small_config(300)).unwrap());
    store.pause_flush(true);
    put_keys(&store, 0..4);

    let writer_store: Arc<Store> = store.clone();
    let writer = thread::spawn(move || writer_store.put(nth_key(4), vec![4; 100]));
    thread::sleep(Duration::from_millis(50));

    // every attempt fails: the store gives up, and the waiting put must not wait forever
    store.fail_next_flushes(3);
    store.pause_flush(false);
    let result = within(5, move || writer.join().unwrap());

    assert!(matches!(result, Err(StoreError::Failed)));
    assert_keys(&store, 0..4);
}

#[test]
fn waiting_put_wakes_up_on_close() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Arc<Store> = Arc::new(Store::open(dir.path(), small_config(300)).unwrap());
    store.pause_flush(true);
    put_keys(&store, 0..4);

    let writer_store: Arc<Store> = store.clone();
    let writer = thread::spawn(move || writer_store.put(nth_key(4), vec![4; 100]));
    thread::sleep(Duration::from_millis(50));

    // close waits for the queued flush, which is paused, so run it on its own thread
    let closer_store: Arc<Store> = store.clone();
    let closer = thread::spawn(move || closer_store.close());

    let result = within(5, move || writer.join().unwrap());
    assert!(matches!(result, Err(StoreError::Closed)));

    store.pause_flush(false);
    within(5, move || closer.join().unwrap()).unwrap();
}

#[test]
fn close_finishes_queued_flushes_and_refuses_writes() {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();
    store.pause_flush(true);
    put_keys(&store, 0..3);
    assert_eq!(store.frozen_count(), 1);
    store.pause_flush(false);

    store.close().unwrap();

    // the queue was flushed before the thread stopped
    assert_eq!(store.frozen_count(), 0);
    assert_eq!(store.sstable_count(), 1);
    assert!(matches!(
        store.put(nth_key(3), vec![3; 100]),
        Err(StoreError::Closed)
    ));
    assert_keys(&store, 0..3);
    // a second close is fine
    store.close().unwrap();
    drop(store);

    // only the active segment is replayed after a restart
    assert_eq!(wal_count(dir.path()), 1);
    let store: Store = Store::open(dir.path(), small_config(300)).unwrap();
    assert_keys(&store, 0..3);
}

#[test]
fn out_of_order_flush_still_reads_the_newest_value() {
    let dir: TempDir = TempDir::new().unwrap();
    let config: Config = Config {
        memtable: MemtableConfig {
            flush_threshold_bytes: 300,
            max_frozen: 2,
        },
        flush: FlushConfig {
            max_attempts: 3,
            // long enough for a newer memtable to flush first
            retry_backoff_ms: 300,
        },
        ..Config::default()
    };
    let store: Store = Store::open(dir.path(), config.clone()).unwrap();

    // memtable A: key 0 = old value. its first flush attempt fails and waits 300 ms
    store.fail_next_flushes(1);
    store.put(nth_key(0), vec![0xAA; 100]).unwrap();
    store.put(nth_key(1), vec![1; 100]).unwrap();
    // memtable B: key 0 = new value. swapped in, flushed while A waits out its retry
    store.put(nth_key(0), vec![0xBB; 100]).unwrap();
    store.put(nth_key(2), vec![2; 100]).unwrap();
    // swaps B out
    store.put(nth_key(3), vec![3; 100]).unwrap();
    store.wait_for_flushes();

    // B got the lower file number (flushed first), A the higher one. reads order by seq
    let manifest: Manifest = store.manifest();
    assert_eq!(manifest.sstables.len(), 2);
    let flushed_first: &SstMeta = &manifest.sstables[0];
    let flushed_second: &SstMeta = &manifest.sstables[1];
    assert!(flushed_first.file_number < flushed_second.file_number);
    assert!(
        flushed_first.max_seq > flushed_second.max_seq,
        "B should flush before A"
    );

    assert_eq!(store.get(&nth_key(0)).unwrap(), Some(vec![0xBB; 100]));
    drop(store);

    // same answer when the SSTables are opened from the manifest
    let store: Store = Store::open(dir.path(), config).unwrap();
    assert_eq!(store.get(&nth_key(0)).unwrap(), Some(vec![0xBB; 100]));
    assert_eq!(store.get(&nth_key(1)).unwrap(), Some(vec![1; 100]));
}

#[test]
fn many_threads_put_and_get_at_once() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().to_path_buf();
    // ~9 entries per memtable: lots of swaps, stalls, and flushes while the threads run
    let store: Arc<Store> = Arc::new(Store::open(&path, small_config(1_000)).unwrap());
    const WRITERS: usize = 4;
    const PUTS_PER_WRITER: usize = 40;

    let run_store: Arc<Store> = store.clone();
    within(60, move || {
        let mut handles = Vec::new();
        for writer in 0..WRITERS {
            let store: Arc<Store> = run_store.clone();
            handles.push(thread::spawn(move || {
                for i in 0..PUTS_PER_WRITER {
                    let n: usize = writer * PUTS_PER_WRITER + i;
                    store.put(nth_key(n), vec![n as u8; 100]).unwrap();
                    // read back our own write right away: it must be there
                    assert_eq!(store.get(&nth_key(n)).unwrap(), Some(vec![n as u8; 100]));
                }
            }));
        }
        // readers in the background: whatever they find must be the right value
        for _ in 0..2 {
            let store: Arc<Store> = run_store.clone();
            handles.push(thread::spawn(move || {
                for n in 0..WRITERS * PUTS_PER_WRITER {
                    if let Some(value) = store.get(&nth_key(n)).unwrap() {
                        assert_eq!(value, vec![n as u8; 100]);
                    }
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
    });

    let total: usize = WRITERS * PUTS_PER_WRITER;
    store.wait_for_flushes();
    assert!(store.sstable_count() > 5);
    assert_eq!(store.last_seq(), total as u64);
    assert_keys(&store, 0..total);
    drop(store);

    let store: Store = Store::open(&path, small_config(1_000)).unwrap();
    assert_keys(&store, 0..total);
}

#[test]
fn dropping_the_last_arc_stops_the_flush_thread() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().to_path_buf();
    let store: Arc<Store> = Arc::new(Store::open(&path, small_config(300)).unwrap());
    put_keys(&store, 0..10);

    // drop runs close: the queue is flushed and the thread joined, no hang
    within(5, move || drop(store));

    let store: Store = Store::open(&path, small_config(300)).unwrap();
    assert_keys(&store, 0..10);
    assert_eq!(wal_count(&path), 1);
}
