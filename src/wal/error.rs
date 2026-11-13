#[derive(Debug)]
pub enum SizeField {
    Key,
    Value,
}

#[derive(Debug)]
pub enum CorruptionReason {
    CrcMismatch,
    UnknownType,
    TombstoneValue,
    LengthLimit(SizeField),
}

#[derive(Debug)]
pub enum WalError {
    // offset is where the bad record starts in the WAL file
    Corruption {
        offset: u64,
        reason: CorruptionReason,
    },
    Io(std::io::Error),
    Size {
        incorrect_size: usize,
        correct_max_size: usize,
        which_field: SizeField,
    },
    // a write or fsync failed earlier, so this WAL no longer accepts writes
    Failed,
}

// lets `?` turn a std::io::Error into a WalError
impl From<std::io::Error> for WalError {
    fn from(error: std::io::Error) -> WalError {
        WalError::Io(error)
    }
}
