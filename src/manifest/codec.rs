use std::collections::HashSet;

use super::error::ManifestCorruption;
use super::{Manifest, SstMeta};

// manifest layout (little endian):
//   | magic 4B | version 2B | next_file_number 8B | last_seq 8B |
//   | wal count 4B | wal file number 8B ... |
//   | sst count 4B | sst ... |
//   | CRC32 4B |
// one sst:
//   | file_number 8B | file_size 8B | min_seq 8B | max_seq 8B |
//   | first keyLen 1B | first key | last keyLen 1B | last key |
// the CRC covers everything before it
pub const MAGIC: u32 = 0x4C53_4D4D; // "LSMM"
pub const FORMAT_VERSION: u16 = 1;
const CRC_LEN: usize = 4;
// magic + version + next_file_number + last_seq + wal count + sst count + CRC
const MIN_LEN: usize = 4 + 2 + 8 + 8 + 4 + 4 + CRC_LEN;

pub fn encode(manifest: &Manifest) -> Result<Vec<u8>, ManifestCorruption> {
    validate(manifest)?;

    let mut encoded: Vec<u8> = Vec::new();
    encoded.extend_from_slice(&MAGIC.to_le_bytes());
    encoded.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    encoded.extend_from_slice(&manifest.next_file_number.to_le_bytes());
    encoded.extend_from_slice(&manifest.last_seq.to_le_bytes());

    encoded.extend_from_slice(&(manifest.wal_segments.len() as u32).to_le_bytes());
    for wal_segment in &manifest.wal_segments {
        encoded.extend_from_slice(&wal_segment.to_le_bytes());
    }

    encoded.extend_from_slice(&(manifest.sstables.len() as u32).to_le_bytes());
    for sst in &manifest.sstables {
        encoded.extend_from_slice(&sst.file_number.to_le_bytes());
        encoded.extend_from_slice(&sst.file_size.to_le_bytes());
        encoded.extend_from_slice(&sst.min_seq.to_le_bytes());
        encoded.extend_from_slice(&sst.max_seq.to_le_bytes());
        encoded.push(sst.first_key.len() as u8);
        encoded.extend_from_slice(&sst.first_key);
        encoded.push(sst.last_key.len() as u8);
        encoded.extend_from_slice(&sst.last_key);
    }

    let checksum: u32 = crc32fast::hash(&encoded);
    encoded.extend_from_slice(&checksum.to_le_bytes());
    Ok(encoded)
}

pub fn decode(bytes: &[u8]) -> Result<Manifest, ManifestCorruption> {
    if bytes.len() < MIN_LEN {
        return Err(ManifestCorruption::TooShort);
    }

    // CRC first: nothing else is trusted until it passes
    let content_len: usize = bytes.len() - CRC_LEN;
    let stored_crc: u32 = u32::from_le_bytes(bytes[content_len..].try_into().unwrap());
    if crc32fast::hash(&bytes[..content_len]) != stored_crc {
        return Err(ManifestCorruption::Crc);
    }

    let mut reader: Reader<'_> = Reader {
        bytes: &bytes[..content_len],
        position: 0,
    };
    if reader.u32()? != MAGIC {
        return Err(ManifestCorruption::BadMagic);
    }
    let version: u16 = reader.u16()?;
    if version != FORMAT_VERSION {
        return Err(ManifestCorruption::UnsupportedVersion(version));
    }

    let next_file_number: u64 = reader.u64()?;
    let last_seq: u64 = reader.u64()?;

    // counts come from the file, so no with_capacity: a bad count can't make a huge allocation
    let wal_count: u32 = reader.u32()?;
    let mut wal_segments: Vec<u64> = Vec::new();
    for _ in 0..wal_count {
        wal_segments.push(reader.u64()?);
    }

    let sst_count: u32 = reader.u32()?;
    let mut sstables: Vec<SstMeta> = Vec::new();
    for _ in 0..sst_count {
        sstables.push(SstMeta {
            file_number: reader.u64()?,
            file_size: reader.u64()?,
            min_seq: reader.u64()?,
            max_seq: reader.u64()?,
            first_key: reader.key()?,
            last_key: reader.key()?,
        });
    }

    if reader.position != reader.bytes.len() {
        return Err(ManifestCorruption::TrailingBytes);
    }

    let manifest: Manifest = Manifest {
        next_file_number,
        last_seq,
        wal_segments,
        sstables,
    };
    validate(&manifest)?;
    Ok(manifest)
}

// checks that hold for every manifest, run on save and on load
fn validate(manifest: &Manifest) -> Result<(), ManifestCorruption> {
    let mut seen: HashSet<u64> = HashSet::new();
    let file_numbers = manifest
        .wal_segments
        .iter()
        .copied()
        .chain(manifest.sstables.iter().map(|sst| sst.file_number));

    for file_number in file_numbers {
        if file_number >= manifest.next_file_number {
            return Err(ManifestCorruption::FileNumberNotAllocated(file_number));
        }
        if !seen.insert(file_number) {
            return Err(ManifestCorruption::DuplicateFileNumber(file_number));
        }
    }

    for sst in &manifest.sstables {
        if sst.min_seq > sst.max_seq || sst.max_seq > manifest.last_seq {
            return Err(ManifestCorruption::BadSeqRange(sst.file_number));
        }
        if sst.first_key > sst.last_key {
            return Err(ManifestCorruption::BadKeyRange(sst.file_number));
        }
        if sst.first_key.len() > u8::MAX as usize || sst.last_key.len() > u8::MAX as usize {
            return Err(ManifestCorruption::KeyTooLong(sst.file_number));
        }
    }
    Ok(())
}

// reads fixed-width fields front to back, Truncated instead of a panic when bytes run out
struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], ManifestCorruption> {
        let end: usize = self
            .position
            .checked_add(len)
            .ok_or(ManifestCorruption::Truncated)?;
        if end > self.bytes.len() {
            return Err(ManifestCorruption::Truncated);
        }
        let taken: &'a [u8] = &self.bytes[self.position..end];
        self.position = end;
        Ok(taken)
    }

    fn u16(&mut self) -> Result<u16, ManifestCorruption> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, ManifestCorruption> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, ManifestCorruption> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    // | keyLen 1B | key |
    fn key(&mut self) -> Result<Vec<u8>, ManifestCorruption> {
        let key_len: usize = self.take(1)?[0] as usize;
        Ok(self.take(key_len)?.to_vec())
    }
}

#[cfg(test)]
mod tests;
