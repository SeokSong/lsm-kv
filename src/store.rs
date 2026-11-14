mod error;

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config::{Config, SstableConfig};
use crate::fs_util::sync_parent_dir;
use crate::manifest::{
    FileKind, Manifest, SstMeta, obsolete_files, parse_file_name, sst_file_name, wal_file_name,
};
use crate::memtable::{Lookup, Memtable};
use crate::sstable::{SstInfo, SstLookup, SstReader, SstWriter};
use crate::wal::{self, Op, Record, WalError, WalWriter};

pub use error::StoreError;

// the single WAL file from before segments. a directory with one of these and no
// manifest isn't a new store
const OLD_WAL_FILE_NAME: &str = "wal.log";

// shareable across threads: callers use Arc<Store>, every method takes &self.
// the lock design and the rules that keep it deadlock free are in md/CONCURRENCY.md
pub struct Store {
    inner: Arc<Inner>,
    // only close touches this. None once the thread has been joined (or never started in tests)
    flush_thread: Mutex<Option<JoinHandle<()>>>,
}

// everything the flush thread shares with the store. the thread holds an Arc of this,
// not of Store, so dropping the last Arc<Store> still runs close and stops it
struct Inner {
    dir: PathBuf,
    config: Config,
    // lock order is always wal -> manifest -> state (rule 1)
    wal: Mutex<WalState>,
    manifest: Mutex<Manifest>,
    state: Mutex<State>,
    // notified whenever the frozen queue, failed, or closing changes
    state_changed: Condvar,
    stats: Stats,
    #[cfg(test)]
    hooks: TestHooks,
}

// held for a whole put: append, fsync, memtable insert (rule 8). the flush thread never takes it
struct WalState {
    writer: WalWriter,
    segment: u64,
    // seq the next put or delete gets
    next_seq: u64,
}

// held only for short memory-only steps, never during file I/O (rule 4)
struct State {
    active: Memtable,
    // oldest first. the queue only grows at the back, so this is also seq order
    frozen: VecDeque<FrozenSlot>,
    // newest max_seq first, the order a get searches them
    sstables: Vec<Arc<LiveSst>>,
    // a flush, manifest save, or WAL write failed: writes are refused, reads still work
    failed: bool,
    // close started: writes are refused, the flush thread finishes the queue and stops
    closing: bool,
    // the error that marked the store failed in the flush thread, handed back by close
    flush_error: Option<StoreError>,
}

// a memtable waiting to be flushed, with the WAL segment that holds the same writes.
// never changes once frozen, so a get can search it without a lock
struct Frozen {
    wal_segment: u64,
    memtable: Memtable,
}

struct FrozenSlot {
    frozen: Arc<Frozen>,
    attempts: u32,
    // after a failed attempt: not tried again before this. newer memtables can flush meanwhile
    retry_at: Option<Instant>,
}

// a live SSTable: what the manifest says about it plus the open file
struct LiveSst {
    meta: SstMeta,
    reader: SstReader,
}

// the steps of a flush, in order. tests stop after one of them to act out a crash there
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub(crate) enum FlushStep {
    WriteSstable,
    SaveManifest,
    DeleteSegment,
    Install,
}

// why a flush attempt failed, and whether trying again is safe
struct FlushFailure {
    error: StoreError,
    // only a failed SSTable write is retried. a failed manifest save leaves it unknown
    // whether the new manifest reached the disk, same as a failed fsync
    retryable: bool,
}

#[derive(Debug, Default)]
struct Stats {
    stalls: AtomicU64,
    stall_nanos: AtomicU64,
    flushes: AtomicU64,
    failed_flush_attempts: AtomicU64,
}

// counters for benchmarks: how often puts waited on a full frozen queue, and for how long
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoreStats {
    pub stalls: u64,
    pub stall_time: Duration,
    pub flushes: u64,
    pub failed_flush_attempts: u64,
}

impl Store {
    // opens the store in dir, creating it if needed. startup order:
    // 1. load the manifest (none = new store, only if there are no store files)
    // 2. delete files the manifest doesn't list, before any new file number is handed out
    // 3. open every listed SSTable
    // 4. replay every listed WAL segment. older ones are flushed now, the newest
    //    becomes the active memtable and keeps getting appends
    // 5. start the flush thread
    pub fn open(dir: &Path, config: Config) -> Result<Store, StoreError> {
        Store::open_with(dir, config, true)
    }

    fn open_with(
        dir: &Path,
        config: Config,
        start_flush_thread: bool,
    ) -> Result<Store, StoreError> {
        if !dir.exists() {
            fs::create_dir_all(dir)?;
            // the new directory only survives a crash once its parent's entry is on disk too
            sync_parent_dir(dir)?;
        }

        let mut manifest: Manifest = match Manifest::load(dir)? {
            Some(manifest) => manifest,
            None => new_store_manifest(dir)?,
        };

        for path in obsolete_files(dir, &manifest)? {
            fs::remove_file(path)?;
        }

        let mut sstables: Vec<Arc<LiveSst>> = Vec::with_capacity(manifest.sstables.len());
        for meta in &manifest.sstables {
            sstables.push(Arc::new(open_sstable(dir, meta.clone())?));
        }
        sstables.sort_by_key(|sst| std::cmp::Reverse(sst.meta.max_seq));

        // next seq is past everything in an SSTable (last_seq, 0 for a new store) and
        // everything replayed. every segment comes back as its own memtable, oldest first
        let mut next_seq: u64 = manifest.last_seq + 1;
        let mut replayed: VecDeque<Frozen> = VecDeque::new();
        for &wal_segment in &manifest.wal_segments {
            let records: Vec<Record> = wal::replay(&dir.join(wal_file_name(wal_segment)))?;
            let mut memtable: Memtable = Memtable::new();
            for record in records {
                next_seq = next_seq.max(record.seq + 1);
                memtable.apply(record);
            }
            replayed.push_back(Frozen {
                wal_segment,
                memtable,
            });
        }

        // the newest segment is the active one. the manifest always lists at least one,
        // but a hand-edited or very old one might not
        let active: Frozen = match replayed.pop_back() {
            Some(active) => active,
            None => {
                let wal_segment: u64 = manifest.new_file_number();
                manifest.wal_segments.push(wal_segment);
                manifest.save(dir)?;
                Frozen {
                    wal_segment,
                    memtable: Memtable::new(),
                }
            }
        };
        let writer: WalWriter = WalWriter::open(&dir.join(wal_file_name(active.wal_segment)))?;

        let older: Vec<Arc<Frozen>> = replayed.into_iter().map(Arc::new).collect();
        let inner: Arc<Inner> = Arc::new(Inner {
            dir: dir.to_path_buf(),
            config,
            wal: Mutex::new(WalState {
                writer,
                segment: active.wal_segment,
                next_seq,
            }),
            manifest: Mutex::new(manifest),
            state: Mutex::new(State {
                active: active.memtable,
                frozen: older
                    .iter()
                    .map(|frozen| FrozenSlot::new(frozen.clone()))
                    .collect(),
                sstables,
                failed: false,
                closing: false,
                flush_error: None,
            }),
            state_changed: Condvar::new(),
            stats: Stats::default(),
            #[cfg(test)]
            hooks: TestHooks::default(),
        });

        // older segments: a crash happened while their memtables waited for a flush.
        // flush them before taking writes, so every memtable keeps its own segment
        for frozen in &older {
            inner
                .run_flush(frozen, FlushStep::Install)
                .map_err(|failure| failure.error)?;
        }

        let flush_thread: Option<JoinHandle<()>> = if start_flush_thread {
            let thread_inner: Arc<Inner> = inner.clone();
            Some(
                thread::Builder::new()
                    .name("lsm-flush".to_string())
                    .spawn(move || thread_inner.flush_loop())?,
            )
        } else {
            None
        };

        Ok(Store {
            inner,
            flush_thread: Mutex::new(flush_thread),
        })
    }

    // durable when this returns Ok
    pub fn put(&self, key: Vec<u8>, value: Vec<u8>) -> Result<(), StoreError> {
        self.write(key, Op::Put { value })
    }

    // writes a tombstone, durable when this returns Ok
    pub fn delete(&self, key: Vec<u8>) -> Result<(), StoreError> {
        self.write(key, Op::Delete)
    }

    // newest first: active memtable, frozen memtables, SSTables by max_seq.
    // the first Found or Deleted wins, a tombstone hides every older value.
    // only the active memtable is read under the lock, the rest after it's released (rule 5)
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        let (frozen, sstables): (Vec<Arc<Frozen>>, Vec<Arc<LiveSst>>) = {
            let state: MutexGuard<'_, State> = lock(&self.inner.state)?;
            match state.active.get(key) {
                Lookup::Found(value) => return Ok(Some(value.to_vec())),
                Lookup::Deleted => return Ok(None),
                Lookup::NotFound => {}
            }
            let frozen: Vec<Arc<Frozen>> = state
                .frozen
                .iter()
                .rev()
                .map(|slot| slot.frozen.clone())
                .collect();
            (frozen, state.sstables.clone())
        };

        for frozen in &frozen {
            match frozen.memtable.get(key) {
                Lookup::Found(value) => return Ok(Some(value.to_vec())),
                Lookup::Deleted => return Ok(None),
                Lookup::NotFound => {}
            }
        }

        for sst in &sstables {
            // outside the file's key range: skip it without touching the bloom filter
            if key < sst.meta.first_key.as_slice() || key > sst.meta.last_key.as_slice() {
                continue;
            }
            match sst.reader.get(key)? {
                SstLookup::Found(value) => return Ok(Some(value)),
                SstLookup::Deleted => return Ok(None),
                SstLookup::NotFound => {}
            }
        }

        Ok(None)
    }

    // stops taking writes, waits for the queued flushes, stops the flush thread.
    // returns the flush error that failed the store, if any. the active memtable isn't
    // flushed: it's in the WAL already. safe to call more than once
    pub fn close(&self) -> Result<(), StoreError> {
        {
            let mut state: MutexGuard<'_, State> = lock(&self.inner.state)?;
            state.closing = true;
            // wakes the flush thread and any put waiting for queue space (rule 7)
            self.inner.state_changed.notify_all();
        }

        // joined with no lock held, the flush thread needs State to finish (rule 9)
        let handle: Option<JoinHandle<()>> = lock(&self.flush_thread)?.take();
        if let Some(handle) = handle
            && handle.join().is_err()
        {
            // the flush thread panicked
            return Err(StoreError::Failed);
        }

        let mut state: MutexGuard<'_, State> = lock(&self.inner.state)?;
        match state.flush_error.take() {
            Some(error) => Err(error),
            None if state.failed => Err(StoreError::Failed),
            None => Ok(()),
        }
    }

    // seq of the newest write, 0 if nothing has been written. seqs start at 1
    pub fn last_seq(&self) -> u64 {
        // read only, a poisoned lock still has a valid number in it
        let wal: MutexGuard<'_, WalState> = self
            .inner
            .wal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        wal.next_seq - 1
    }

    pub fn sstable_count(&self) -> usize {
        self.inner
            .state
            .lock()
            .map(|state| state.sstables.len())
            .unwrap_or(0)
    }

    pub fn stats(&self) -> StoreStats {
        let stats: &Stats = &self.inner.stats;
        StoreStats {
            stalls: stats.stalls.load(Ordering::Relaxed),
            stall_time: Duration::from_nanos(stats.stall_nanos.load(Ordering::Relaxed)),
            flushes: stats.flushes.load(Ordering::Relaxed),
            failed_flush_attempts: stats.failed_flush_attempts.load(Ordering::Relaxed),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.inner.dir
    }

    fn write(&self, key: Vec<u8>, op: Op) -> Result<(), StoreError> {
        let mut wal: MutexGuard<'_, WalState> = lock(&self.inner.wal)?;
        let record: Record = Record {
            seq: wal.next_seq,
            key,
            op,
        };
        // bad input is rejected before it can trigger a swap and a flush
        wal::validate(&record)?;

        let mut state: MutexGuard<'_, State> = lock(&self.inner.state)?;
        check_writable(&state)?;
        if needs_swap(&state.active, &record, &self.inner.config) {
            // wait for queue space holding only WAL: the condvar wait releases State, and the
            // flush thread never takes WAL, so it can always finish and free a slot (rule 3).
            // in a loop: a wakeup doesn't mean there's room yet (rule 6)
            let mut stall_start: Option<Instant> = None;
            while state.frozen.len() >= self.inner.config.memtable.max_frozen
                && !state.failed
                && !state.closing
            {
                stall_start.get_or_insert_with(Instant::now);
                state = self
                    .inner
                    .state_changed
                    .wait(state)
                    .map_err(|_| StoreError::Failed)?;
            }
            if let Some(stall_start) = stall_start {
                self.inner.stats.stalls.fetch_add(1, Ordering::Relaxed);
                self.inner
                    .stats
                    .stall_nanos
                    .fetch_add(stall_start.elapsed().as_nanos() as u64, Ordering::Relaxed);
            }
            // woken by a failure or close instead of a free slot (rule 7)
            check_writable(&state)?;
            drop(state);

            // the manifest lock comes after the wait, never before it
            self.inner.swap(&mut wal)?;
        } else {
            drop(state);
        }

        // WAL first: if the append fails the memtable is untouched
        match wal.writer.append(&record) {
            Ok(()) => {}
            // the WAL writer refuses everything after a failed write or fsync, so the store does too
            Err(error @ (WalError::Io(_) | WalError::Failed)) => {
                self.inner.mark_failed();
                return Err(error.into());
            }
            Err(error) => return Err(error.into()),
        }
        // only an acked write uses up its seq. a rejected one leaves no gap
        wal.next_seq += 1;
        lock(&self.inner.state)?.active.apply(record);
        Ok(())
    }
}

// a close that wasn't called by hand still stops the flush thread. errors can't be
// returned from drop, call close to see them
impl Drop for Store {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

impl Inner {
    // the active memtable becomes frozen and a new segment + memtable take new writes.
    // called with the WAL lock held and the State lock not held
    fn swap(&self, wal: &mut WalState) -> Result<(), StoreError> {
        // the manifest lists the new segment before it's created, so a segment the manifest
        // doesn't list can never hold an acked write
        let segment: u64 = {
            let mut manifest: MutexGuard<'_, Manifest> = lock(&self.manifest)?;
            let mut next: Manifest = manifest.clone();
            let segment: u64 = next.new_file_number();
            next.wal_segments.push(segment);
            // change a copy and keep it only once it's saved
            if let Err(error) = next.save(&self.dir) {
                drop(manifest);
                self.mark_failed();
                return Err(error.into());
            }
            *manifest = next;
            segment
        };

        let writer: WalWriter = match WalWriter::open(&self.dir.join(wal_file_name(segment))) {
            Ok(writer) => writer,
            Err(error) => {
                self.mark_failed();
                return Err(error.into());
            }
        };
        let old_segment: u64 = std::mem::replace(&mut wal.segment, segment);
        wal.writer = writer;

        let mut state: MutexGuard<'_, State> = lock(&self.state)?;
        let memtable: Memtable = std::mem::take(&mut state.active);
        state.frozen.push_back(FrozenSlot::new(Arc::new(Frozen {
            wal_segment: old_segment,
            memtable,
        })));
        // wake the flush thread
        self.state_changed.notify_all();
        Ok(())
    }

    // the flush thread: flush whatever is ready until close says stop (or the store fails)
    fn flush_loop(&self) {
        while let Some(frozen) = self.next_to_flush() {
            if let Err(failure) = self.run_flush(&frozen, FlushStep::Install) {
                self.flush_failed(&frozen, failure);
            }
        }
    }

    // the oldest frozen memtable that isn't waiting out a retry. sleeps until there is one.
    // None means stop: the store failed, or it's closing and the queue is empty
    fn next_to_flush(&self) -> Option<Arc<Frozen>> {
        let mut state: MutexGuard<'_, State> = self.state.lock().ok()?;
        loop {
            if state.failed {
                return None;
            }
            let now: Instant = Instant::now();
            if let Some(slot) = state
                .frozen
                .iter()
                .find(|slot| slot.retry_at.is_none_or(|retry_at| retry_at <= now))
            {
                return Some(slot.frozen.clone());
            }

            match state.frozen.iter().filter_map(|slot| slot.retry_at).min() {
                // everything left is waiting out a retry: sleep until the first one is due
                Some(retry_at) => {
                    let timeout: Duration = retry_at.saturating_duration_since(now);
                    state = self.state_changed.wait_timeout(state, timeout).ok()?.0;
                }
                None if state.closing => return None,
                None => state = self.state_changed.wait(state).ok()?,
            }
        }
    }

    // one flush attempt, steps up to and including last_step. stopping early is only for
    // tests: the store is then dropped and reopened, as if it crashed right there
    fn run_flush(&self, frozen: &Frozen, last_step: FlushStep) -> Result<(), FlushFailure> {
        // 1. write the SSTable (tmp, fsync, rename, fsync dir) and open it. no lock held while
        //    writing. an empty memtable has nothing to write, its segment is just dropped
        let sst: Option<Arc<LiveSst>> = if frozen.memtable.is_empty() {
            None
        } else {
            let file_number: u64 = lock(&self.manifest)
                .map_err(FlushFailure::fatal)?
                .new_file_number();
            let path: PathBuf = self.dir.join(sst_file_name(file_number));
            match self.write_sstable(&path, file_number, &frozen.memtable) {
                Ok(sst) => Some(Arc::new(sst)),
                Err(error) => {
                    // nothing lists this file yet, so a retry can start clean
                    remove_sstable_files(&path);
                    return Err(FlushFailure::retryable(error));
                }
            }
        };
        if last_step == FlushStep::WriteSstable {
            return Ok(());
        }

        // 2. the manifest: SSTable in, segment out. from here the segment's data lives in
        //    the SSTable, a crash after this never replays it
        {
            let mut manifest: MutexGuard<'_, Manifest> =
                lock(&self.manifest).map_err(FlushFailure::fatal)?;
            let mut next: Manifest = manifest.clone();
            if let Some(sst) = &sst {
                next.sstables.push(sst.meta.clone());
                next.last_seq = next.last_seq.max(sst.meta.max_seq);
            }
            next.wal_segments
                .retain(|&segment| segment != frozen.wal_segment);
            // not retried, and the SSTable is kept: the new manifest may be on disk already
            next.save(&self.dir)
                .map_err(|error| FlushFailure::fatal(error.into()))?;
            *manifest = next;
        }
        if last_step == FlushStep::SaveManifest {
            return Ok(());
        }

        // 3. delete the old segment. a failure only leaves an unlisted file that the next
        //    open deletes, the data is safe in the SSTable, so the flush still counts
        let _ = fs::remove_file(self.dir.join(wal_file_name(frozen.wal_segment)));
        if last_step == FlushStep::DeleteSegment {
            return Ok(());
        }

        // 4. reads switch from the frozen memtable to the SSTable in one step
        let mut state: MutexGuard<'_, State> = lock(&self.state).map_err(FlushFailure::fatal)?;
        if let Some(sst) = sst {
            let position: usize = state
                .sstables
                .partition_point(|live| live.meta.max_seq > sst.meta.max_seq);
            state.sstables.insert(position, sst);
        }
        state
            .frozen
            .retain(|slot| slot.frozen.wal_segment != frozen.wal_segment);
        self.stats.flushes.fetch_add(1, Ordering::Relaxed);
        // a put may be waiting for this slot
        self.state_changed.notify_all();
        Ok(())
    }

    fn write_sstable(
        &self,
        path: &Path,
        file_number: u64,
        memtable: &Memtable,
    ) -> Result<LiveSst, StoreError> {
        #[cfg(test)]
        self.hooks.before_sstable_write()?;

        let config: &SstableConfig = &self.config.sstable;
        let mut writer: SstWriter = SstWriter::create(path, config)?;
        for (key, seq, op) in memtable.iter() {
            writer.add(key, seq, op)?;
        }
        let info: SstInfo = writer.finish()?;
        // opened before the manifest lists it, so a bad file is still a retryable failure
        open_sstable(&self.dir, SstMeta::new(file_number, &info))
    }

    // a failed attempt: try again later (behind newer memtables), or give up and fail the store
    fn flush_failed(&self, frozen: &Frozen, failure: FlushFailure) {
        self.stats
            .failed_flush_attempts
            .fetch_add(1, Ordering::Relaxed);
        let Ok(mut state) = self.state.lock() else {
            return;
        };

        let max_attempts: u32 = self.config.flush.max_attempts;
        let backoff_ms: u64 = self.config.flush.retry_backoff_ms;
        let give_up: bool = match state
            .frozen
            .iter_mut()
            .find(|slot| slot.frozen.wal_segment == frozen.wal_segment)
        {
            Some(slot) => {
                slot.attempts += 1;
                // 100, 200, 400 ms with the default config
                let backoff: Duration = Duration::from_millis(
                    backoff_ms.saturating_mul(1 << (slot.attempts - 1).min(16)),
                );
                slot.retry_at = Some(Instant::now() + backoff);
                !failure.retryable || slot.attempts >= max_attempts
            }
            None => true,
        };

        if give_up {
            state.failed = true;
            state.flush_error.get_or_insert(failure.error);
            // a put waiting for queue space would otherwise wait forever (rule 7)
            self.state_changed.notify_all();
        }
    }

    fn mark_failed(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.failed = true;
            self.state_changed.notify_all();
        }
    }
}

impl FrozenSlot {
    fn new(frozen: Arc<Frozen>) -> FrozenSlot {
        FrozenSlot {
            frozen,
            attempts: 0,
            retry_at: None,
        }
    }
}

impl FlushFailure {
    fn retryable(error: StoreError) -> FlushFailure {
        FlushFailure {
            error,
            retryable: true,
        }
    }

    fn fatal(error: StoreError) -> FlushFailure {
        FlushFailure {
            error,
            retryable: false,
        }
    }
}

// a poisoned lock means a thread panicked in the middle of a change: treat the store as
// failed instead of panicking every other thread too (rule 10)
fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, StoreError> {
    mutex.lock().map_err(|_| StoreError::Failed)
}

fn check_writable(state: &State) -> Result<(), StoreError> {
    if state.failed {
        return Err(StoreError::Failed);
    }
    if state.closing {
        return Err(StoreError::Closed);
    }
    Ok(())
}

// swap before the put that would push the memtable over the threshold, so a memtable
// (and the SSTable it becomes) never goes over. an empty memtable always takes the put,
// otherwise a value bigger than the threshold would swap forever
fn needs_swap(active: &Memtable, record: &Record, config: &Config) -> bool {
    let value_len: usize = match &record.op {
        Op::Put { value } => value.len(),
        Op::Delete => 0,
    };
    let size_after: usize = active.approximate_size() + record.key.len() + value_len;
    !active.is_empty() && size_after > config.memtable.flush_threshold_bytes
}

// no manifest: a new store, unless the directory already holds store files. treating that
// as new would delete them all as garbage on the next step
fn new_store_manifest(dir: &Path) -> Result<Manifest, StoreError> {
    for entry in fs::read_dir(dir)? {
        let name: String = entry?.file_name().to_string_lossy().into_owned();
        let is_store_file: bool = matches!(
            parse_file_name(&name),
            Some(FileKind::Wal(_) | FileKind::Sst(_))
        );
        if is_store_file || name == OLD_WAL_FILE_NAME {
            return Err(StoreError::MissingManifest);
        }
    }

    // listed before the file exists: a listed segment with no file replays as empty
    let mut manifest: Manifest = Manifest::new();
    let wal_segment: u64 = manifest.new_file_number();
    manifest.wal_segments.push(wal_segment);
    manifest.save(dir)?;
    Ok(manifest)
}

fn open_sstable(dir: &Path, meta: SstMeta) -> Result<LiveSst, StoreError> {
    let path: PathBuf = dir.join(sst_file_name(meta.file_number));
    let actual: u64 = fs::metadata(&path)?.len();
    if actual != meta.file_size {
        return Err(StoreError::SstSizeMismatch {
            file_number: meta.file_number,
            expected: meta.file_size,
            actual,
        });
    }
    let reader: SstReader = SstReader::open(&path)?;
    Ok(LiveSst { meta, reader })
}

// best effort: whatever is left is unlisted, the next open deletes it anyway
fn remove_sstable_files(path: &Path) {
    let mut tmp_name: std::ffi::OsString = path.as_os_str().to_os_string();
    tmp_name.push(".tmp");
    let _ = fs::remove_file(PathBuf::from(tmp_name));
    let _ = fs::remove_file(path);
}

// test-only controls over the flush thread
#[cfg(test)]
#[derive(Default)]
struct TestHooks {
    // while true the flush thread waits before writing an SSTable (holding no lock)
    pause_flush: std::sync::atomic::AtomicBool,
    // this many upcoming SSTable writes fail as if the disk were full
    fail_flushes: std::sync::atomic::AtomicU32,
}

#[cfg(test)]
impl TestHooks {
    fn before_sstable_write(&self) -> Result<(), StoreError> {
        while self.pause_flush.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(1));
        }
        let failed: bool = self
            .fail_flushes
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        if failed {
            return Err(StoreError::Io(std::io::Error::other("test: flush failed")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
