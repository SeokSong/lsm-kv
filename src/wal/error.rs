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
    Corruption {
        reason: CorruptionReason,
    },
    Io(std::io::Error),
    Size {
        incorrect_size: usize,
        correct_max_size: usize,
        which_field: SizeField,
    },
}
