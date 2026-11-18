// why a chunk was refused, on encode (a bug in the caller) or decode (a bad or foreign value)
#[derive(Debug, PartialEq)]
pub enum ChunkError {
    // shorter than the fixed header, or than the header + columns it describes
    TooShort,
    UnsupportedVersion(u16),
    // header_len smaller than the fixed header + column list
    BadHeaderLen,
    NoColumns,
    // every chunk needs exactly one time column
    MissingTime,
    DuplicateColumn(u16),
    UnknownType(u8),
    // a catalog column stored with another type than the catalog says
    WrongType(u16),
    // 0 rows, or more than MAX_ROWS
    BadRowCount(usize),
    // columns of different lengths (encode), or bytes left over after the last column (decode)
    LengthMismatch,
    // over the 64 KiB value limit
    TooLarge(usize),
    // the first time value must be 0: times are offsets from the chunk's start
    TimeNotFromZero,
    TimeGoesBackwards,
    // end in the header isn't start + the last time offset
    EndMismatch,
}
