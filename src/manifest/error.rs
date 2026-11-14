#[derive(Debug, PartialEq)]
pub enum ManifestCorruption {
    // shorter than the smallest possible manifest
    TooShort,
    Crc,
    BadMagic,
    UnsupportedVersion(u16),
    // the CRC passed but a count or length runs past the end
    Truncated,
    // bytes left over after the last SSTable
    TrailingBytes,
    // the same file number is listed twice (WAL segments and SSTables share one counter)
    DuplicateFileNumber(u64),
    // a listed file number that was never handed out (>= next_file_number)
    FileNumberNotAllocated(u64),
    // SSTable with min_seq > max_seq, or max_seq > last_seq
    BadSeqRange(u64),
    // SSTable with first_key > last_key
    BadKeyRange(u64),
    // SSTable key longer than the 1-byte length allows (only on save)
    KeyTooLong(u64),
}

#[derive(Debug)]
pub enum ManifestError {
    Io(std::io::Error),
    // load: the file on disk is bad
    Corruption(ManifestCorruption),
    // save: refused to write a manifest that load would reject
    Invalid(ManifestCorruption),
}

// lets `?` turn a std::io::Error into a ManifestError
impl From<std::io::Error> for ManifestError {
    fn from(error: std::io::Error) -> ManifestError {
        ManifestError::Io(error)
    }
}
