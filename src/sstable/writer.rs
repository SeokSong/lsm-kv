use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::config::SstableConfig;
use crate::wal::Op;

use super::block::BlockBuilder;
use super::bloom::{BloomFilter, stable_hash};
use super::error::SstError;
use super::index::{Footer, IndexEntry, encode_index};

// what finish wrote
#[derive(Debug, Clone, PartialEq)]
pub struct SstInfo {
    pub path: PathBuf,
    pub entry_count: usize,
    pub file_size: u64,
    pub first_key: Vec<u8>,
    pub last_key: Vec<u8>,
}

// writes one SSTable from entries in increasing key order (memtable.iter() order).
// everything goes to "<path>.tmp" and is renamed to path in finish, so a crash never
// leaves a half-written file that looks like a real SSTable
pub struct SstWriter {
    path: PathBuf,
    tmp_path: PathBuf,
    file: BufWriter<File>,
    offset: u64,
    block: BlockBuilder,
    block_size_bytes: usize,
    bloom_bits_per_key: usize,
    index: Vec<IndexEntry>,
    key_hashes: Vec<u64>,
    first_key: Option<Vec<u8>>,
    last_key: Option<Vec<u8>>,
    entry_count: usize,
}

impl SstWriter {
    pub fn create(path: &Path, config: &SstableConfig) -> Result<SstWriter, SstError> {
        let mut tmp_name: OsString = path.as_os_str().to_os_string();
        tmp_name.push(".tmp");
        let tmp_path: PathBuf = PathBuf::from(tmp_name);

        let file: File = File::create(&tmp_path)?;

        Ok(SstWriter {
            path: path.to_path_buf(),
            tmp_path,
            file: BufWriter::new(file),
            offset: 0,
            block: BlockBuilder::new(config.restart_interval),
            block_size_bytes: config.block_size_bytes,
            bloom_bits_per_key: config.bloom_bits_per_key,
            index: Vec::new(),
            key_hashes: Vec::new(),
            first_key: None,
            last_key: None,
            entry_count: 0,
        })
    }

    pub fn add(&mut self, key: &[u8], op: &Op) -> Result<(), SstError> {
        if key.len() > u8::MAX as usize {
            return Err(SstError::KeyTooLong { len: key.len() });
        }
        if let Some(last_key) = &self.last_key
            && key <= last_key.as_slice()
        {
            return Err(SstError::OutOfOrder);
        }

        self.block.add(key, op);
        self.key_hashes.push(stable_hash(key));
        if self.first_key.is_none() {
            self.first_key = Some(key.to_vec());
        }
        self.last_key = Some(key.to_vec());
        self.entry_count += 1;

        // soft target: the block closes once it reaches the size, never splits an entry
        if self.block.size_estimate() >= self.block_size_bytes {
            self.finish_block()?;
        }
        Ok(())
    }

    // writes the last block, bloom filter, index, and footer, fsyncs, then renames into place
    pub fn finish(mut self) -> Result<SstInfo, SstError> {
        if self.entry_count == 0 {
            drop(self.file);
            let _ = fs::remove_file(&self.tmp_path);
            return Err(SstError::Empty);
        }
        if !self.block.is_empty() {
            self.finish_block()?;
        }

        let (bloom_offset, bloom_len): (u64, u64) = if self.bloom_bits_per_key > 0 {
            let bloom: Vec<u8> =
                BloomFilter::build(&self.key_hashes, self.bloom_bits_per_key).encode();
            let bloom_offset: u64 = self.offset;
            self.write_bytes(&bloom)?;
            (bloom_offset, bloom.len() as u64)
        } else {
            (0, 0)
        };

        let index: Vec<u8> = encode_index(&self.index);
        let index_offset: u64 = self.offset;
        self.write_bytes(&index)?;

        // offsets and lengths are u32 in the footer
        if self.offset > u32::MAX as u64 {
            return Err(SstError::FileTooLarge);
        }
        let footer: Footer = Footer {
            index_offset: index_offset as u32,
            bloom_offset: bloom_offset as u32,
            index_len: index.len() as u32,
            bloom_len: bloom_len as u32,
        };
        self.write_bytes(&footer.encode())?;

        // the whole file must be on disk before it gets its real name
        let file: File = self.file.into_inner().map_err(|error| error.into_error())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&self.tmp_path, &self.path)?;
        sync_parent_dir(&self.path)?;

        Ok(SstInfo {
            path: self.path,
            entry_count: self.entry_count,
            file_size: self.offset,
            first_key: self.first_key.unwrap_or_default(),
            last_key: self.last_key.unwrap_or_default(),
        })
    }

    fn finish_block(&mut self) -> Result<(), SstError> {
        let block: Vec<u8> = self.block.finish();
        let block_offset: u64 = self.offset;
        self.write_bytes(&block)?;

        if block_offset > u32::MAX as u64 || block.len() > u32::MAX as usize {
            return Err(SstError::FileTooLarge);
        }
        self.index.push(IndexEntry {
            last_key: self.last_key.clone().unwrap_or_default(),
            offset: block_offset as u32,
            len: block.len() as u32,
        });
        Ok(())
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), SstError> {
        self.file.write_all(bytes)?;
        self.offset += bytes.len() as u64;
        Ok(())
    }
}

fn sync_parent_dir(path: &Path) -> std::io::Result<()> {
    let parent: &Path = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    File::open(parent)?.sync_all()
}
