use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::codec::encode;
use super::error::WalError;
use super::record::Record;
use crate::fs_util::sync_parent_dir;

pub struct WalWriter {
    file: File,
    path: PathBuf,
    failed: bool,
}

impl WalWriter {
    // opens the WAL for appending, creating it if it doesn't exist
    pub fn open(path: &Path) -> Result<WalWriter, WalError> {
        let is_new_file: bool = !path.exists();

        let file: File = OpenOptions::new().create(true).append(true).open(path)?;

        // new file only survives a crash once its directory entry is on disk too
        if is_new_file {
            sync_parent_dir(path)?;
        }

        Ok(WalWriter {
            file,
            path: path.to_path_buf(),
            failed: false,
        })
    }

    // durable when this returns Ok: the record is written and fsynced
    pub fn append(&mut self, record: &Record) -> Result<(), WalError> {
        if self.failed {
            return Err(WalError::Failed);
        }

        // bad input is rejected before anything is written, so the writer stays healthy
        let encoded_record: Vec<u8> = encode(record)?;

        // after a failed write or fsync we can't know what reached the disk, so refuse all later writes
        if let Err(error) = self.write_and_sync(&encoded_record) {
            self.failed = true;
            return Err(WalError::Io(error));
        }

        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn write_and_sync(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.file.write_all(bytes)?;
        // sync_data is enough for an append: it flushes the bytes and the new file length
        self.file.sync_data()
    }
}

#[cfg(test)]
mod tests;
