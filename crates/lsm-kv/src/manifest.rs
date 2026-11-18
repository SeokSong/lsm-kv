// the list of live files: which SSTables count and which WAL segments still need replaying.
// rewritten as a whole file on every change (tmp, fsync, rename, fsync dir), so a crash
// leaves the old manifest or the new one, never half of each.
// rule at startup: anything in the directory the manifest doesn't list is garbage
mod codec;
mod error;
mod files;

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use crate::fs_util::sync_parent_dir;
use crate::sstable::SstInfo;

pub use codec::{decode, encode};
pub use error::{ManifestCorruption, ManifestError};
pub use files::{
    FileKind, MANIFEST_FILE_NAME, obsolete_files, parse_file_name, sst_file_name, wal_file_name,
};

// file numbers start at 1, 0 is never a file
const FIRST_FILE_NUMBER: u64 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    // one counter for WAL segments and SSTables. only names files, seqs say what's newer
    pub next_file_number: u64,
    // highest seq that made it into an SSTable. after a restart the next seq is
    // max(this, last replayed seq) + 1, even if every WAL segment was already flushed
    pub last_seq: u64,
    // live WAL segments, oldest first. every one of them is replayed at startup
    pub wal_segments: Vec<u64>,
    pub sstables: Vec<SstMeta>,
}

// what the manifest keeps about one SSTable
#[derive(Debug, Clone, PartialEq)]
pub struct SstMeta {
    pub file_number: u64,
    // checked against the real file at open, a cheap catch for a truncated file
    pub file_size: u64,
    // reads go newest max_seq first
    pub min_seq: u64,
    pub max_seq: u64,
    // lets a get skip a file whose key range can't hold the key, before the bloom filter
    pub first_key: Vec<u8>,
    pub last_key: Vec<u8>,
}

impl SstMeta {
    pub fn new(file_number: u64, info: &SstInfo) -> SstMeta {
        SstMeta {
            file_number,
            file_size: info.file_size,
            min_seq: info.min_seq,
            max_seq: info.max_seq,
            first_key: info.first_key.clone(),
            last_key: info.last_key.clone(),
        }
    }
}

impl Manifest {
    // manifest for a brand new store: no files yet
    pub fn new() -> Manifest {
        Manifest {
            next_file_number: FIRST_FILE_NUMBER,
            last_seq: 0,
            wal_segments: Vec::new(),
            sstables: Vec::new(),
        }
    }

    // hands out the next file number. it's only on disk once the manifest is saved, so a
    // crash can hand the same number out again. safe because startup deletes files the
    // manifest doesn't list before creating any new ones
    pub fn new_file_number(&mut self) -> u64 {
        let file_number: u64 = self.next_file_number;
        self.next_file_number += 1;
        file_number
    }

    // None means no manifest yet: a brand new store
    pub fn load(dir: &Path) -> Result<Option<Manifest>, ManifestError> {
        let bytes: Vec<u8> = match fs::read(dir.join(MANIFEST_FILE_NAME)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(ManifestError::Io(error)),
        };
        let manifest: Manifest = decode(&bytes).map_err(ManifestError::Corruption)?;
        Ok(Some(manifest))
    }

    // MANIFEST.tmp, fsync, rename to MANIFEST, fsync the directory. durable when it returns Ok
    pub fn save(&self, dir: &Path) -> Result<(), ManifestError> {
        // never write a manifest that load would reject
        let bytes: Vec<u8> = encode(self).map_err(ManifestError::Invalid)?;

        let path: PathBuf = dir.join(MANIFEST_FILE_NAME);
        let mut tmp_name: OsString = path.as_os_str().to_os_string();
        tmp_name.push(".tmp");
        let tmp_path: PathBuf = PathBuf::from(tmp_name);

        let mut file: File = File::create(&tmp_path)?;
        file.write_all(&bytes)?;
        // the bytes must be on disk before the file gets the real name
        file.sync_all()?;
        drop(file);
        // rename is atomic: MANIFEST is the old one or the new one, never half of each
        fs::rename(&tmp_path, &path)?;
        // and the rename itself only survives a crash once the directory is fsynced
        sync_parent_dir(&path)?;
        Ok(())
    }
}

impl Default for Manifest {
    fn default() -> Manifest {
        Manifest::new()
    }
}

#[cfg(test)]
mod tests;
