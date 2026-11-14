mod error;

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::fs_util::sync_parent_dir;
use crate::manifest::{
    FileKind, Manifest, SstMeta, obsolete_files, parse_file_name, sst_file_name, wal_file_name,
};
use crate::memtable::{Lookup, Memtable};
use crate::sstable::{SstInfo, SstLookup, SstReader, SstWriter};
use crate::wal::{self, Op, Record, WalWriter};

pub use error::StoreError;

// the single WAL file from before segments. a directory with one of these and no
// manifest isn't a new store
const OLD_WAL_FILE_NAME: &str = "wal.log";

// a memtable waiting to be flushed, with the WAL segment that holds the same writes
struct Frozen {
    wal_segment: u64,
    memtable: Memtable,
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

pub struct Store {
    dir: PathBuf,
    config: Config,
    manifest: Manifest,
    // the active WAL segment, holds the same writes as the active memtable
    wal: WalWriter,
    wal_segment: u64,
    memtable: Memtable,
    // oldest first. in PR 3a a frozen memtable is flushed right away, the flush thread
    // takes over in 3b
    frozen: VecDeque<Frozen>,
    // newest max_seq first, the order a get searches them
    sstables: Vec<LiveSst>,
    // seq the next put or delete gets
    next_seq: u64,
    // set when a flush or manifest save fails, refuses writes from then on
    failed: bool,
    // test hook: the next flush fails at step 1 as if the disk were full
    #[cfg(test)]
    fail_next_flush: bool,
}

impl Store {
    // opens the store in dir, creating it if needed. startup order:
    // 1. load the manifest (none = new store, only if there are no store files)
    // 2. delete files the manifest doesn't list, before any new file number is handed out
    // 3. open every listed SSTable
    // 4. replay every listed WAL segment. older ones are flushed now, the newest
    //    becomes the active memtable and keeps getting appends
    pub fn open(dir: &Path, config: Config) -> Result<Store, StoreError> {
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

        let mut sstables: Vec<LiveSst> = Vec::with_capacity(manifest.sstables.len());
        for meta in &manifest.sstables {
            sstables.push(open_sstable(dir, meta.clone())?);
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
        let wal: WalWriter = WalWriter::open(&dir.join(wal_file_name(active.wal_segment)))?;

        let mut store: Store = Store {
            dir: dir.to_path_buf(),
            config,
            manifest,
            wal,
            wal_segment: active.wal_segment,
            memtable: active.memtable,
            frozen: replayed,
            sstables,
            next_seq,
            failed: false,
            #[cfg(test)]
            fail_next_flush: false,
        };

        // older segments: a crash happened while their memtables waited for a flush.
        // flush them before taking writes, so every memtable keeps its own segment
        while !store.frozen.is_empty() {
            store.flush_oldest()?;
        }

        Ok(store)
    }

    // durable when this returns Ok
    pub fn put(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<(), StoreError> {
        self.write(key, Op::Put { value })
    }

    // writes a tombstone, durable when this returns Ok
    pub fn delete(&mut self, key: Vec<u8>) -> Result<(), StoreError> {
        self.write(key, Op::Delete)
    }

    // newest first: active memtable, frozen memtables, SSTables by max_seq.
    // the first Found or Deleted wins, a tombstone hides every older value
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        match self.memtable.get(key) {
            Lookup::Found(value) => return Ok(Some(value.to_vec())),
            Lookup::Deleted => return Ok(None),
            Lookup::NotFound => {}
        }

        for frozen in self.frozen.iter().rev() {
            match frozen.memtable.get(key) {
                Lookup::Found(value) => return Ok(Some(value.to_vec())),
                Lookup::Deleted => return Ok(None),
                Lookup::NotFound => {}
            }
        }

        for sst in &self.sstables {
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

    // seq of the newest write, 0 if nothing has been written. seqs start at 1
    pub fn last_seq(&self) -> u64 {
        self.next_seq - 1
    }

    pub fn sstable_count(&self) -> usize {
        self.sstables.len()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn write(&mut self, key: Vec<u8>, op: Op) -> Result<(), StoreError> {
        if self.failed {
            return Err(StoreError::Failed);
        }
        let record: Record = Record {
            seq: self.next_seq,
            key,
            op,
        };
        // bad input is rejected before it can trigger a swap and a flush
        wal::validate(&record)?;

        if self.needs_swap(&record) {
            self.swap_memtable()?;
            // PR 3a: flush inline. 3b hands this to the flush thread
            self.flush_oldest()?;
        }

        // WAL first: if the append fails (write or fsync error) the memtable is untouched
        self.wal.append(&record)?;
        // only an acked write uses up its seq. a rejected one leaves no gap
        self.next_seq += 1;
        self.memtable.apply(record);
        Ok(())
    }

    // swap before the put that would push the memtable over the threshold, so a memtable
    // (and the SSTable it becomes) never goes over. an empty memtable always takes the put,
    // otherwise a value bigger than the threshold would swap forever
    fn needs_swap(&self, record: &Record) -> bool {
        let value_len: usize = match &record.op {
            Op::Put { value } => value.len(),
            Op::Delete => 0,
        };
        let size_after: usize = self.memtable.approximate_size() + record.key.len() + value_len;
        !self.memtable.is_empty() && size_after > self.config.memtable.flush_threshold_bytes
    }

    // the active memtable becomes frozen and a new segment + memtable take new writes.
    // the manifest lists the new segment before it's created, so a segment the manifest
    // doesn't list can never hold an acked write
    pub(crate) fn swap_memtable(&mut self) -> Result<(), StoreError> {
        let wal_segment: u64 = self.manifest.new_file_number();
        self.manifest.wal_segments.push(wal_segment);
        if let Err(error) = self.manifest.save(&self.dir) {
            self.failed = true;
            return Err(error.into());
        }

        let wal: WalWriter = match WalWriter::open(&self.dir.join(wal_file_name(wal_segment))) {
            Ok(wal) => wal,
            Err(error) => {
                self.failed = true;
                return Err(error.into());
            }
        };

        let old_memtable: Memtable = std::mem::take(&mut self.memtable);
        let old_segment: u64 = std::mem::replace(&mut self.wal_segment, wal_segment);
        self.wal = wal;
        self.frozen.push_back(Frozen {
            wal_segment: old_segment,
            memtable: old_memtable,
        });
        Ok(())
    }

    // flushes the oldest frozen memtable, every step. a failure marks the store failed
    // (retries come with the flush thread in 3b), the frozen memtable stays readable
    fn flush_oldest(&mut self) -> Result<(), StoreError> {
        self.flush_oldest_until(FlushStep::Install)
    }

    // runs the flush steps up to and including last_step. stopping early is only for tests:
    // the store is then dropped and reopened, as if it crashed right there
    pub(crate) fn flush_oldest_until(&mut self, last_step: FlushStep) -> Result<(), StoreError> {
        let result: Result<(), StoreError> = self.run_flush_steps(last_step);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn run_flush_steps(&mut self, last_step: FlushStep) -> Result<(), StoreError> {
        let Some(frozen) = self.frozen.front() else {
            return Ok(());
        };
        let wal_segment: u64 = frozen.wal_segment;

        // 1. write the SSTable (tmp, fsync, rename, fsync dir). nothing to write for an
        //    empty memtable, its segment is just dropped
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_flush) {
            return Err(StoreError::Io(std::io::Error::other("test: flush failed")));
        }
        let meta: Option<SstMeta> = if frozen.memtable.is_empty() {
            None
        } else {
            let file_number: u64 = self.manifest.new_file_number();
            let path: PathBuf = self.dir.join(sst_file_name(file_number));
            let mut writer: SstWriter = SstWriter::create(&path, &self.config.sstable)?;
            for (key, seq, op) in frozen.memtable.iter() {
                writer.add(key, seq, op)?;
            }
            let info: SstInfo = writer.finish()?;
            Some(SstMeta::new(file_number, &info))
        };
        if last_step == FlushStep::WriteSstable {
            return Ok(());
        }

        // 2. the manifest: SSTable in, segment out. from here the segment's data lives in
        //    the SSTable, a crash after this never replays it
        if let Some(meta) = &meta {
            self.manifest.sstables.push(meta.clone());
            self.manifest.last_seq = self.manifest.last_seq.max(meta.max_seq);
        }
        self.manifest
            .wal_segments
            .retain(|&segment| segment != wal_segment);
        self.manifest.save(&self.dir)?;
        if last_step == FlushStep::SaveManifest {
            return Ok(());
        }

        // 3. delete the old segment. a crash before this leaves it unlisted, startup deletes it
        fs::remove_file(self.dir.join(wal_file_name(wal_segment)))?;
        if last_step == FlushStep::DeleteSegment {
            return Ok(());
        }

        // 4. reads switch from the frozen memtable to the SSTable
        if let Some(meta) = meta {
            let sst: LiveSst = open_sstable(&self.dir, meta)?;
            let position: usize = self
                .sstables
                .partition_point(|live| live.meta.max_seq > sst.meta.max_seq);
            self.sstables.insert(position, sst);
        }
        self.frozen.pop_front();
        Ok(())
    }
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

#[cfg(test)]
mod tests;
