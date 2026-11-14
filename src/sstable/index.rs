use super::block::CRC_LEN;
use super::error::SstCorruption;

// index layout: | keyLen 1B | last key | block offset 4B | block len 4B | ... | CRC32 4B |
// one entry per data block. block len includes the block's CRC
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct IndexEntry {
    pub(crate) last_key: Vec<u8>,
    pub(crate) offset: u32,
    pub(crate) len: u32,
}

pub(crate) fn encode_index(entries: &[IndexEntry]) -> Vec<u8> {
    let mut encoded: Vec<u8> = Vec::new();
    for entry in entries {
        encoded.push(entry.last_key.len() as u8);
        encoded.extend_from_slice(&entry.last_key);
        encoded.extend_from_slice(&entry.offset.to_le_bytes());
        encoded.extend_from_slice(&entry.len.to_le_bytes());
    }
    let checksum: u32 = crc32fast::hash(&encoded);
    encoded.extend_from_slice(&checksum.to_le_bytes());
    encoded
}

pub(crate) fn decode_index(bytes: &[u8]) -> Result<Vec<IndexEntry>, SstCorruption> {
    if bytes.len() < CRC_LEN {
        return Err(SstCorruption::BadIndex);
    }
    let content_len: usize = bytes.len() - CRC_LEN;
    let stored_crc: u32 = u32::from_le_bytes(bytes[content_len..].try_into().unwrap());
    if crc32fast::hash(&bytes[..content_len]) != stored_crc {
        return Err(SstCorruption::IndexCrc);
    }

    let content: &[u8] = &bytes[..content_len];
    let mut entries: Vec<IndexEntry> = Vec::new();
    let mut offset: usize = 0;
    while offset < content.len() {
        let key_len: usize = content[offset] as usize;
        let key_end: usize = offset + 1 + key_len;
        let entry_end: usize = key_end + 8;
        if entry_end > content.len() {
            return Err(SstCorruption::BadIndex);
        }

        let entry: IndexEntry = IndexEntry {
            last_key: content[offset + 1..key_end].to_vec(),
            offset: u32::from_le_bytes(content[key_end..key_end + 4].try_into().unwrap()),
            len: u32::from_le_bytes(content[key_end + 4..entry_end].try_into().unwrap()),
        };
        // last keys strictly increase, same as the blocks they point to
        if let Some(previous) = entries.last()
            && entry.last_key <= previous.last_key
        {
            return Err(SstCorruption::BadIndex);
        }
        entries.push(entry);
        offset = entry_end;
    }

    if entries.is_empty() {
        return Err(SstCorruption::BadIndex);
    }
    Ok(entries)
}

// the only block that could hold key: the first one whose last key is >= key
pub(crate) fn find_block<'a>(entries: &'a [IndexEntry], key: &[u8]) -> Option<&'a IndexEntry> {
    let position: usize = entries.partition_point(|entry| entry.last_key.as_slice() < key);
    entries.get(position)
}

// footer layout (22 bytes, little endian), order from DESIGN.md:
// | index offset 4B | bloom offset 4B | index len 4B | bloom len 4B | format version 2B | magic 4B |
// bloom len 0 means the file has no bloom filter
pub(crate) const FOOTER_LEN: usize = 4 + 4 + 4 + 4 + 2 + 4;
pub(crate) const FORMAT_VERSION: u16 = 1;
// "LSMK"
pub(crate) const MAGIC: u32 = 0x4C53_4D4B;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Footer {
    pub(crate) index_offset: u32,
    pub(crate) bloom_offset: u32,
    pub(crate) index_len: u32,
    pub(crate) bloom_len: u32,
}

impl Footer {
    pub(crate) fn encode(&self) -> [u8; FOOTER_LEN] {
        let mut encoded: [u8; FOOTER_LEN] = [0u8; FOOTER_LEN];
        encoded[0..4].copy_from_slice(&self.index_offset.to_le_bytes());
        encoded[4..8].copy_from_slice(&self.bloom_offset.to_le_bytes());
        encoded[8..12].copy_from_slice(&self.index_len.to_le_bytes());
        encoded[12..16].copy_from_slice(&self.bloom_len.to_le_bytes());
        encoded[16..18].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        encoded[18..22].copy_from_slice(&MAGIC.to_le_bytes());
        encoded
    }

    // file_len is used to check every section sits before the footer
    pub(crate) fn decode(bytes: &[u8; FOOTER_LEN], file_len: u64) -> Result<Footer, SstCorruption> {
        let magic: u32 = u32::from_le_bytes(bytes[18..22].try_into().unwrap());
        if magic != MAGIC {
            return Err(SstCorruption::BadMagic);
        }
        let version: u16 = u16::from_le_bytes(bytes[16..18].try_into().unwrap());
        if version != FORMAT_VERSION {
            return Err(SstCorruption::UnsupportedVersion(version));
        }

        let footer: Footer = Footer {
            index_offset: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            bloom_offset: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            index_len: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            bloom_len: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
        };

        let footer_start: u64 = file_len - FOOTER_LEN as u64;
        let index_end: u64 = footer.index_offset as u64 + footer.index_len as u64;
        let bloom_end: u64 = footer.bloom_offset as u64 + footer.bloom_len as u64;
        if footer.index_len == 0 || index_end > footer_start || bloom_end > footer_start {
            return Err(SstCorruption::SectionOutOfBounds);
        }
        Ok(footer)
    }
}

#[cfg(test)]
mod tests;
