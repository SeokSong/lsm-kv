use std::fs::{self, File};
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::memtable::{Lookup, Memtable};
use crate::wal::{self, Op, Record, WalError, WalWriter};

// every store error is a WAL error for now. this becomes its own enum once
// SSTable and manifest errors exist, without changing the Store signatures
pub type StoreError = WalError;

const WAL_FILE_NAME: &str = "wal.log";

pub struct Store {
    dir: PathBuf,
    config: Config,
    wal: WalWriter,
    memtable: Memtable,
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
        })
    }

    // durable when this returns Ok
    pub fn put(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<(), StoreError> {
        let record: Record = Record {
            key,
            op: Op::Put { value },
        };
        self.write(record)
    }

    // writes a tombstone, durable when this returns Ok
    pub fn delete(&mut self, key: Vec<u8>) -> Result<(), StoreError> {
        let record: Record = Record {
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

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn write(&mut self, record: Record) -> Result<(), StoreError> {
        // WAL first: if the append fails (bad input, write or fsync error) the memtable is untouched
        self.wal.append(&record)?;
        self.memtable.apply(record);
        Ok(())
    }
}

fn sync_parent_dir(path: &Path) -> std::io::Result<()> {
    let parent: &Path = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    File::open(parent)?.sync_all()
}

#[cfg(test)]
mod tests;
