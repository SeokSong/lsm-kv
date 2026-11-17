// bulk load: a whole session written straight to SSTables, skipping the WAL and memtable.
// for data that's complete before it's written (an iRacing session, a friend's export,
// a historical race). md/BULK_LOAD.md has the design
//
//   let mut ingest = store.begin_ingest()?;
//   ingest.put(key, value)?;   // keys strictly increasing
//   let info = ingest.commit()?;
//
// all or nothing: commit lists every file in one manifest save. dropping an ingest without
// committing deletes what it wrote, a crash before commit leaves unlisted files that the
// next open deletes
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, MutexGuard};

use crate::manifest::{Manifest, SstMeta, sst_file_name};
use crate::memtable::Memtable;
use crate::sstable::{SstInfo, SstWriter};
use crate::wal::{self, Op};

use super::{
    Inner, LiveSst, State, Store, StoreError, WalState, check_writable, lock, open_sstable,
    remove_sstable_files,
};

pub use super::error::IngestError;

// what a commit wrote
#[derive(Debug, Clone, PartialEq)]
pub struct IngestInfo {
    // every entry of the ingest has this seq: the whole import counts as one write
    pub seq: u64,
    pub files: usize,
    pub entries: usize,
    pub bytes: u64,
}

pub struct Ingest<'a> {
    inner: &'a Inner,
    seq: u64,
    // the file being written
    current: Option<OpenFile>,
    // finished files, not listed anywhere until commit
    written: Vec<(u64, SstInfo)>,
    last_key: Option<Vec<u8>>,
    entries: usize,
    // a put failed: everything is cleaned up, only drop is left
    aborted: bool,
    // listed in the manifest (or possibly listed, after a failed save): drop must not delete
    committed: bool,
}

struct OpenFile {
    file_number: u64,
    path: PathBuf,
    writer: SstWriter,
    // key + value bytes added, compared with target_file_bytes
    bytes: usize,
}

impl Store {
    // reserves the seq every entry of this ingest gets. puts can keep going meanwhile,
    // the ingest only takes the WAL lock again at commit
    pub fn begin_ingest(&self) -> Result<Ingest<'_>, StoreError> {
        let mut wal: MutexGuard<'_, WalState> = lock(&self.inner.wal)?;
        check_writable(&*lock(&self.inner.state)?)?;
        let seq: u64 = wal.next_seq;
        // never saved before commit: after a crash the seq is handed out again, which is
        // fine, the files it wrote are unlisted and get deleted at open
        wal.next_seq += 1;
        Ok(Ingest {
            inner: &self.inner,
            seq,
            current: None,
            written: Vec::new(),
            last_key: None,
            entries: 0,
            aborted: false,
            committed: false,
        })
    }
}

impl Ingest<'_> {
    // keys in strictly increasing order, same size limits as Store::put. only puts: nothing
    // existing can overlap an ingest, so a tombstone in one would never hide anything
    pub fn put(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<(), StoreError> {
        if self.aborted {
            return Err(IngestError::Aborted.into());
        }
        let result: Result<(), StoreError> = self.add(key, value);
        if result.is_err() {
            // imports are untrusted: one bad entry and nothing of this ingest is kept
            self.abort();
        }
        result
    }

    // lists every file in one manifest save, then makes them readable
    pub fn commit(mut self) -> Result<IngestInfo, StoreError> {
        if self.aborted {
            return Err(IngestError::Aborted.into());
        }
        // drop cleans up on every early return below
        self.finish_current()?;
        if self.written.is_empty() {
            return Err(IngestError::Empty.into());
        }

        // open (and check the size of) every file before anything is listed. file I/O, no lock
        let mut ssts: Vec<Arc<LiveSst>> = Vec::with_capacity(self.written.len());
        for (file_number, info) in &self.written {
            let meta: SstMeta = SstMeta::new(*file_number, info);
            ssts.push(Arc::new(open_sstable(&self.inner.dir, meta)?));
        }

        // held until the files are readable: no put can land in the ingest's key range between
        // the overlap check and the install. lock order WAL -> Manifest -> State (rule 1)
        let _wal: MutexGuard<'_, WalState> = lock(&self.inner.wal)?;
        {
            let state: MutexGuard<'_, State> = lock(&self.inner.state)?;
            check_writable(&state)?;
            if ssts.iter().any(|sst| overlaps(&state, sst)) {
                return Err(IngestError::Overlaps.into());
            }
        }

        {
            let mut manifest: MutexGuard<'_, Manifest> = lock(&self.inner.manifest)?;
            let mut next: Manifest = manifest.clone();
            next.sstables
                .extend(ssts.iter().map(|sst| sst.meta.clone()));
            next.last_seq = next.last_seq.max(self.seq);
            if let Err(error) = next.save(&self.inner.dir) {
                // same as a failed flush save: the new manifest may be on disk already, so
                // the files may be listed. keep them, fail the store, a reopen sorts it out
                self.committed = true;
                drop(manifest);
                self.inner.mark_failed();
                return Err(error.into());
            }
            *manifest = next;
        }
        self.committed = true;

        let mut state: MutexGuard<'_, State> = lock(&self.inner.state)?;
        for sst in &ssts {
            let position: usize = state
                .sstables
                .partition_point(|live| live.meta.max_seq > sst.meta.max_seq);
            state.sstables.insert(position, sst.clone());
        }
        self.inner.stats.ingests.fetch_add(1, Ordering::Relaxed);
        self.inner.state_changed.notify_all();

        Ok(IngestInfo {
            seq: self.seq,
            files: self.written.len(),
            entries: self.entries,
            bytes: self.written.iter().map(|(_, info)| info.file_size).sum(),
        })
    }

    fn add(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<(), StoreError> {
        wal::check_sizes(key.len(), value.len())?;
        if let Some(last_key) = &self.last_key
            && key.as_slice() <= last_key.as_slice()
        {
            return Err(IngestError::OutOfOrder.into());
        }

        if let Some(current) = &self.current {
            let config = &self.inner.config.sstable;
            let full: bool = current.bytes >= config.target_file_bytes;
            let new_prefix: bool = match &self.last_key {
                Some(last_key) => {
                    prefix(last_key, config.split_prefix_len)
                        != prefix(&key, config.split_prefix_len)
                }
                None => false,
            };
            if full || new_prefix {
                self.finish_current()?;
            }
        }

        if self.current.is_none() {
            let file_number: u64 = lock(&self.inner.manifest)?.new_file_number();
            let path: PathBuf = self.inner.dir.join(sst_file_name(file_number));
            let writer: SstWriter = SstWriter::create(&path, &self.inner.config.sstable)?;
            self.current = Some(OpenFile {
                file_number,
                path,
                writer,
                bytes: 0,
            });
        }

        let current: &mut OpenFile = self.current.as_mut().unwrap();
        current.bytes += key.len() + value.len();
        current.writer.add(&key, self.seq, &Op::Put { value })?;
        self.last_key = Some(key);
        self.entries += 1;
        Ok(())
    }

    // finish (tmp, fsync, rename, fsync dir) the file being written, still unlisted
    fn finish_current(&mut self) -> Result<(), StoreError> {
        if let Some(current) = self.current.take() {
            let path: PathBuf = current.path.clone();
            match current.writer.finish() {
                Ok(info) => self.written.push((current.file_number, info)),
                Err(error) => {
                    remove_sstable_files(&path);
                    return Err(error.into());
                }
            }
        }
        Ok(())
    }

    // deletes everything this ingest wrote. nothing lists these files, so it's safe
    fn abort(&mut self) {
        self.aborted = true;
        if let Some(current) = self.current.take() {
            // the open file is closed before it's deleted
            drop(current.writer);
            remove_sstable_files(&current.path);
        }
        for (file_number, _) in self.written.drain(..) {
            remove_sstable_files(&self.inner.dir.join(sst_file_name(file_number)));
        }
    }
}

impl Drop for Ingest<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.abort();
        }
    }
}

// the first n bytes of a key, or the whole key if it's shorter
fn prefix(key: &[u8], n: usize) -> &[u8] {
    &key[..key.len().min(n)]
}

// does anything already in the store fall inside [first key, last key] of this file
fn overlaps(state: &State, sst: &LiveSst) -> bool {
    let first: &[u8] = &sst.meta.first_key;
    let last: &[u8] = &sst.meta.last_key;
    // range() is end exclusive: last key + a zero byte is the smallest key after it
    let mut end: Vec<u8> = last.to_vec();
    end.push(0);

    let in_memtable = |memtable: &Memtable| memtable.range(first, &end).next().is_some();
    in_memtable(&state.active)
        || state
            .frozen
            .iter()
            .any(|slot| in_memtable(&slot.frozen.memtable))
        || state.sstables.iter().any(|live| {
            live.meta.first_key.as_slice() <= last && first <= live.meta.last_key.as_slice()
        })
}

#[cfg(test)]
mod tests;
