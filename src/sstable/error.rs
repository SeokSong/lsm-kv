#[derive(Debug, PartialEq)]
pub enum SstCorruption {
    // file is shorter than a footer
    FileTooSmall,
    BadMagic,
    UnsupportedVersion(u16),
    // footer points outside the file, or sections overlap the footer
    SectionOutOfBounds,
    BlockCrc,
    IndexCrc,
    BloomCrc,
    // block layout is wrong: restart array, counts, sizes
    BadBlock,
    // an entry runs past its block, bad type byte, or a shared prefix longer than the previous key
    BadEntry,
    BadIndex,
    BadBloom,
}

#[derive(Debug)]
pub enum SstError {
    Io(std::io::Error),
    // offset is where the bad section (block, index, bloom, footer) starts in the file
    Corruption { offset: u64, reason: SstCorruption },
    // writer: keys must be added in strictly increasing order
    OutOfOrder,
    // writer: keys longer than 255 bytes don't fit the 1-byte key lengths
    KeyTooLong { len: usize },
    // writer: an SSTable needs at least one entry
    Empty,
    // writer: offsets in the index and footer are u32, so a file is capped at 4 GiB
    FileTooLarge,
}

// lets `?` turn a std::io::Error into an SstError
impl From<std::io::Error> for SstError {
    fn from(error: std::io::Error) -> SstError {
        SstError::Io(error)
    }
}
