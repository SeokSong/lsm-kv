use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::fs_util::sync_parent_dir;
use crate::memtable::{Lookup, Memtable};
use crate::wal::{self, Op, Record, WalError, WalWriter};

// every store error is a WAL error for now. this becomes its own enum once
// SSTable and manifest errors exist, without changing the Store signatures
pub type StoreError = WalError;

const WAL_FILE_NAME: &str = "wal.log";

// seq 0 is never written, so "nothing written yet" needs no special case
const FIRST_SEQ: u64 = 1;

pub struct Store {
    dir: PathBuf,
    config: Config,
    wal: WalWriter,
    memtable: Memtable,
    // seq the next put or delete gets
    next_seq: u64,
}

impl Store {
    // opens the store in dir, creating it if needed, and rebuilds the memtable from the WAL
    pub fn open(dir: &Path, config: Config) -> Result<Store, StoreError> {
        if !dir.exists() {
            fs::create_dir_all(dir)?;
            // the new directory only survives a crash once its parent's entry is on disk too
            sync_parent_dir(dir)?;
        }

        let wal_path: PathBuf = dir.join(WAL_FILE_NAME);

        // replay before opening the writer, so a torn tail is cut off before new appends
        let records: Vec<Record> = wal::replay(&wal_path)?;
        // replay checked that seqs strictly increase, so the last record has the highest.
        // the manifest's last seq joins this once SSTables exist (week 3)
        let next_seq: u64 = match records.last() {
            Some(record) => record.seq + 1,
            None => FIRST_SEQ,
        };
        let mut memtable: Memtable = Memtable::new();
        for record in records {
            memtable.apply(record);
        }

        let wal: WalWriter = WalWriter::open(&wal_path)?;

        Ok(Store {
            dir: dir.to_path_buf(),
            config,
            wal,
            memtable,
            next_seq,
        })
    }

    // durable when this returns Ok
    pub fn put(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<(), StoreError> {
        let record: Record = Record {
            seq: self.next_seq,
            key,
            op: Op::Put { value },
        };
        self.write(record)
    }

    // writes a tombstone, durable when this returns Ok
    pub fn delete(&mut self, key: Vec<u8>) -> Result<(), StoreError> {
        let record: Record = Record {
            seq: self.next_seq,
            key,
            op: Op::Delete,
        };
        self.write(record)
    }

    // returns an owned copy of the value. Result because reads will hit SSTables (disk) later
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        match self.memtable.get(key) {
            Lookup::Found(value) => Ok(Some(value.to_vec())),
            // later NotFound falls through to the frozen memtables and SSTables
            Lookup::Deleted | Lookup::NotFound => Ok(None),
        }
    }

    // true once the memtable reaches the configured flush threshold. flush comes in week 3
    pub fn memtable_full(&self) -> bool {
        self.memtable.approximate_size() >= self.config.memtable.flush_threshold_bytes
    }

    // seq of the newest write, 0 if nothing has been written
    pub fn last_seq(&self) -> u64 {
        self.next_seq - 1
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn write(&mut self, record: Record) -> Result<(), StoreError> {
        // WAL first: if the append fails (bad input, write or fsync error) the memtable is untouched
        self.wal.append(&record)?;
        // only an acked write uses up its seq. a rejected one leaves no gap
        self.next_seq += 1;
        self.memtable.apply(record);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
