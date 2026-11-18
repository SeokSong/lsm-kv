use crate::sstable::SstError;

// why a merge stopped. after any of these the iterator yields nothing more
#[derive(Debug)]
pub enum MergeError {
    // a source failed to read (SSTable I/O or corruption)
    Sst(SstError),
    // source at this index gave a key that wasn't greater than its previous key:
    // the source isn't sorted, or has the same key twice
    OutOfOrder { source: usize },
    // two sources have the same key with the same seq. seqs are unique per write, so
    // one of them is wrong and it's unknown which version is real
    DuplicateSeq { seq: u64 },
}

impl From<SstError> for MergeError {
    fn from(error: SstError) -> MergeError {
        MergeError::Sst(error)
    }
}
