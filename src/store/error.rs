use crate::manifest::ManifestError;
use crate::sstable::SstError;
use crate::wal::WalError;

#[derive(Debug)]
pub enum StoreError {
    Wal(WalError),
    Sst(SstError),
    Manifest(ManifestError),
    // file errors outside the WAL, SSTable, and manifest code (creating the dir, deleting files)
    Io(std::io::Error),
    // the directory has WAL segments or SSTables but no MANIFEST. opening it as a new store
    // would delete every one of them as garbage, so open refuses instead
    MissingManifest,
    // a listed SSTable isn't the size the manifest says: truncated or replaced
    SstSizeMismatch {
        file_number: u64,
        expected: u64,
        actual: u64,
    },
    // a flush or manifest save failed earlier, the store no longer accepts writes.
    // reads still work and nothing acked is lost, a restart replays the WAL
    Failed,
    // close was called, the store no longer accepts writes. reads still work
    Closed,
}

impl From<WalError> for StoreError {
    fn from(error: WalError) -> StoreError {
        StoreError::Wal(error)
    }
}

impl From<SstError> for StoreError {
    fn from(error: SstError) -> StoreError {
        StoreError::Sst(error)
    }
}

impl From<ManifestError> for StoreError {
    fn from(error: ManifestError) -> StoreError {
        StoreError::Manifest(error)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> StoreError {
        StoreError::Io(error)
    }
}
