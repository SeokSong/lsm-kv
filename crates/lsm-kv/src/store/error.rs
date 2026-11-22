use crate::manifest::ManifestError;
use crate::merge::MergeError;
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
    // a bulk load was refused. nothing it wrote is left behind or visible
    Ingest(IngestError),
    // a scan's merge stopped: an SSTable failed to read (I/O, corruption), or a source was
    // out of order or had a duplicate seq (a bug). see MergeError
    Merge(MergeError),
}

#[derive(Debug, PartialEq)]
pub enum IngestError {
    // keys must strictly increase across the whole ingest
    OutOfOrder,
    // commit with nothing added
    Empty,
    // existing data (memtable or SSTable) is inside the ingest's key range, e.g. the same
    // session imported twice. refused instead of mixing two copies
    Overlaps,
    // an earlier put on this ingest failed, it can only be dropped now
    Aborted,
}

impl From<IngestError> for StoreError {
    fn from(error: IngestError) -> StoreError {
        StoreError::Ingest(error)
    }
}

impl From<MergeError> for StoreError {
    fn from(error: MergeError) -> StoreError {
        StoreError::Merge(error)
    }
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
