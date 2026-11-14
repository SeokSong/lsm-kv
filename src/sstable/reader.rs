use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::vec::IntoIter;

use crate::wal::Op;

use super::block::{Block, BlockLookup};
use super::bloom::BloomFilter;
use super::error::{SstCorruption, SstError};
use super::index::{FOOTER_LEN, Footer, IndexEntry, decode_index, find_block};

// what get found in this SSTable. Deleted means a tombstone: stop searching, don't
// check older SSTables. same idea as the memtable's Lookup, but the value is owned (read from disk)
#[derive(Debug, PartialEq)]
pub enum SstLookup {
    Found(Vec<u8>),
    Deleted,
    NotFound,
}

// an open SSTable. the footer, index, and bloom filter are read once and kept in memory,
// a get reads at most one data block
pub struct SstReader {
    path: PathBuf,
    file: File,
    index: Vec<IndexEntry>,
    bloom: Option<BloomFilter>,
}

impl SstReader {
    pub fn open(path: &Path) -> Result<SstReader, SstError> {
        let file: File = File::open(path)?;
        let file_len: u64 = file.metadata()?.len();
        if file_len < FOOTER_LEN as u64 {
            return Err(corruption(0, SstCorruption::FileTooSmall));
        }

        let footer_offset: u64 = file_len - FOOTER_LEN as u64;
        let mut footer_bytes: [u8; FOOTER_LEN] = [0u8; FOOTER_LEN];
        file.read_exact_at(&mut footer_bytes, footer_offset)?;
        let footer: Footer = Footer::decode(&footer_bytes, file_len)
            .map_err(|reason| corruption(footer_offset, reason))?;

        let index_bytes: Vec<u8> = read_section(&file, footer.index_offset, footer.index_len)?;
        let index: Vec<IndexEntry> = decode_index(&index_bytes)
            .map_err(|reason| corruption(footer.index_offset as u64, reason))?;

        let bloom: Option<BloomFilter> = if footer.bloom_len > 0 {
            let bloom_bytes: Vec<u8> = read_section(&file, footer.bloom_offset, footer.bloom_len)?;
            Some(
                BloomFilter::decode(&bloom_bytes)
                    .map_err(|reason| corruption(footer.bloom_offset as u64, reason))?,
            )
        } else {
            None
        };

        // blocks must sit before the bloom filter and index
        let data_end: u64 = if footer.bloom_len > 0 {
            footer.bloom_offset as u64
        } else {
            footer.index_offset as u64
        };
        for entry in &index {
            if entry.offset as u64 + entry.len as u64 > data_end {
                return Err(corruption(
                    footer.index_offset as u64,
                    SstCorruption::SectionOutOfBounds,
                ));
            }
        }

        Ok(SstReader {
            path: path.to_path_buf(),
            file,
            index,
            bloom,
        })
    }

    // &self: reads use read_exact_at (no seek), so one open SSTable can be shared across threads later
    pub fn get(&self, key: &[u8]) -> Result<SstLookup, SstError> {
        // bloom says definitely not here: no disk read
        if let Some(bloom) = &self.bloom
            && !bloom.may_contain(key)
        {
            return Ok(SstLookup::NotFound);
        }

        // key is past the last block's last key: not in this file
        let entry: &IndexEntry = match find_block(&self.index, key) {
            Some(entry) => entry,
            None => return Ok(SstLookup::NotFound),
        };

        let block_bytes: Vec<u8> = read_section(&self.file, entry.offset, entry.len)?;
        let block: Block<'_> =
            Block::parse(&block_bytes).map_err(|reason| corruption(entry.offset as u64, reason))?;

        match block
            .get(key)
            .map_err(|reason| corruption(entry.offset as u64, reason))?
        {
            BlockLookup::Found(value) => Ok(SstLookup::Found(value)),
            BlockLookup::Deleted => Ok(SstLookup::Deleted),
            BlockLookup::NotFound => Ok(SstLookup::NotFound),
        }
    }

    // every entry in key order, tombstones included. reads one block at a time
    pub fn iter(&self) -> SstIter<'_> {
        SstIter {
            reader: self,
            next_block: 0,
            current: Vec::new().into_iter(),
            failed: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn block_count(&self) -> usize {
        self.index.len()
    }

    fn read_block_entries(&self, entry: &IndexEntry) -> Result<Vec<(Vec<u8>, Op)>, SstError> {
        let block_bytes: Vec<u8> = read_section(&self.file, entry.offset, entry.len)?;
        let block: Block<'_> =
            Block::parse(&block_bytes).map_err(|reason| corruption(entry.offset as u64, reason))?;
        block
            .entries()
            .map_err(|reason| corruption(entry.offset as u64, reason))
    }
}

pub struct SstIter<'a> {
    reader: &'a SstReader,
    next_block: usize,
    current: IntoIter<(Vec<u8>, Op)>,
    // after an error the iterator stops instead of skipping the bad block
    failed: bool,
}

impl Iterator for SstIter<'_> {
    type Item = Result<(Vec<u8>, Op), SstError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        loop {
            if let Some(entry) = self.current.next() {
                return Some(Ok(entry));
            }
            let index_entry: &IndexEntry = self.reader.index.get(self.next_block)?;
            self.next_block += 1;
            match self.reader.read_block_entries(index_entry) {
                Ok(entries) => self.current = entries.into_iter(),
                Err(error) => {
                    self.failed = true;
                    return Some(Err(error));
                }
            }
        }
    }
}

fn read_section(file: &File, offset: u32, len: u32) -> Result<Vec<u8>, SstError> {
    let mut bytes: Vec<u8> = vec![0u8; len as usize];
    file.read_exact_at(&mut bytes, offset as u64)?;
    Ok(bytes)
}

fn corruption(offset: u64, reason: SstCorruption) -> SstError {
    SstError::Corruption { offset, reason }
}

#[cfg(test)]
mod tests;
