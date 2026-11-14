use crate::wal::{Op, Record};

use super::error::SstCorruption;

// same type bytes as the WAL
pub(crate) const TYPE_PUT: u8 = 1;
pub(crate) const TYPE_DELETE: u8 = 2;

// type + shared + unshared + valueLen + seq
const ENTRY_HEADER_LEN: usize = 1 + 1 + 1 + 4 + 8;
pub(crate) const CRC_LEN: usize = 4;

// block layout (little endian):
//   entries:  | type 1B | shared 1B | unshared 1B | valueLen 4B | seq 8B | key suffix | value | ...
//   trailer:  | restart offsets 4B each | restart count 4B | CRC32 4B |
// shared = bytes of the key that match the previous key. at a restart point shared is 0
// and the full key is stored, so a lookup can binary search the restart points
pub(crate) struct BlockBuilder {
    buffer: Vec<u8>,
    restarts: Vec<u32>,
    restart_interval: usize,
    entries_since_restart: usize,
    last_key: Vec<u8>,
}

impl BlockBuilder {
    pub(crate) fn new(restart_interval: usize) -> BlockBuilder {
        BlockBuilder {
            buffer: Vec::new(),
            restarts: Vec::new(),
            restart_interval,
            entries_since_restart: 0,
            last_key: Vec::new(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    // caller makes sure keys come in increasing order and fit in 1-byte lengths
    pub(crate) fn add(&mut self, key: &[u8], seq: u64, op: &Op) {
        let starts_restart: bool =
            self.restarts.is_empty() || self.entries_since_restart >= self.restart_interval;

        let shared: usize = if starts_restart {
            self.restarts.push(self.buffer.len() as u32);
            self.entries_since_restart = 0;
            0
        } else {
            common_prefix_len(&self.last_key, key)
        };

        let (type_byte, value): (u8, &[u8]) = match op {
            Op::Put { value } => (TYPE_PUT, value),
            Op::Delete => (TYPE_DELETE, &[]),
        };
        let unshared: &[u8] = &key[shared..];

        self.buffer.push(type_byte);
        self.buffer.push(shared as u8);
        self.buffer.push(unshared.len() as u8);
        self.buffer
            .extend_from_slice(&(value.len() as u32).to_le_bytes());
        self.buffer.extend_from_slice(&seq.to_le_bytes());
        self.buffer.extend_from_slice(unshared);
        self.buffer.extend_from_slice(value);

        self.last_key.clear();
        self.last_key.extend_from_slice(key);
        self.entries_since_restart += 1;
    }

    // size of the block if it were finished now, compared against the block size target
    pub(crate) fn size_estimate(&self) -> usize {
        self.buffer.len() + self.restarts.len() * 4 + 4 + CRC_LEN
    }

    // returns the finished block bytes and resets the builder for the next block
    pub(crate) fn finish(&mut self) -> Vec<u8> {
        let mut block: Vec<u8> = std::mem::take(&mut self.buffer);
        block.reserve(self.restarts.len() * 4 + 4 + CRC_LEN);

        for restart in &self.restarts {
            block.extend_from_slice(&restart.to_le_bytes());
        }
        block.extend_from_slice(&(self.restarts.len() as u32).to_le_bytes());

        let checksum: u32 = crc32fast::hash(&block);
        block.extend_from_slice(&checksum.to_le_bytes());

        self.restarts.clear();
        self.entries_since_restart = 0;
        self.last_key.clear();
        block
    }
}

fn common_prefix_len(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

// result of looking a key up in one block
#[derive(Debug, PartialEq)]
pub(crate) enum BlockLookup {
    Found(Vec<u8>),
    Deleted,
    NotFound,
}

// a block that passed its CRC check, borrowed from the bytes read off disk
pub(crate) struct Block<'a> {
    entries: &'a [u8],
    restarts: Vec<usize>,
}

// one decoded entry, value borrowed from the block
struct EntryRef<'a> {
    key: Vec<u8>,
    seq: u64,
    type_byte: u8,
    value: &'a [u8],
    next_offset: usize,
}

impl<'a> Block<'a> {
    // checks the CRC and the trailer before anything else is read
    pub(crate) fn parse(bytes: &'a [u8]) -> Result<Block<'a>, SstCorruption> {
        if bytes.len() < 4 + CRC_LEN {
            return Err(SstCorruption::BadBlock);
        }

        let content_len: usize = bytes.len() - CRC_LEN;
        let stored_crc: u32 = u32::from_le_bytes(bytes[content_len..].try_into().unwrap());
        if crc32fast::hash(&bytes[..content_len]) != stored_crc {
            return Err(SstCorruption::BlockCrc);
        }

        let content: &[u8] = &bytes[..content_len];
        let restart_count: usize =
            u32::from_le_bytes(content[content_len - 4..].try_into().unwrap()) as usize;

        // every block has at least one entry, so at least one restart
        let restart_bytes: usize = restart_count
            .checked_mul(4)
            .ok_or(SstCorruption::BadBlock)?;
        if restart_count == 0 || restart_bytes + 4 > content_len {
            return Err(SstCorruption::BadBlock);
        }

        let entries_len: usize = content_len - 4 - restart_bytes;
        let mut restarts: Vec<usize> = Vec::with_capacity(restart_count);
        for i in 0..restart_count {
            let start: usize = entries_len + i * 4;
            let restart: usize =
                u32::from_le_bytes(content[start..start + 4].try_into().unwrap()) as usize;
            // first restart is the first entry, the rest strictly increase and stay inside the entries
            let in_order: bool = match restarts.last() {
                Some(&previous) => restart > previous,
                None => restart == 0,
            };
            if !in_order || restart >= entries_len {
                return Err(SstCorruption::BadBlock);
            }
            restarts.push(restart);
        }

        Ok(Block {
            entries: &content[..entries_len],
            restarts,
        })
    }

    // every entry in key order, tombstones included
    pub(crate) fn entries(&self) -> Result<Vec<Record>, SstCorruption> {
        let mut decoded: Vec<Record> = Vec::new();
        let mut offset: usize = 0;
        let mut prev_key: Vec<u8> = Vec::new();

        while offset < self.entries.len() {
            let entry: EntryRef<'a> = self.decode_entry(offset, &prev_key)?;
            let op: Op = to_op(entry.type_byte, entry.value)?;
            offset = entry.next_offset;
            prev_key = entry.key.clone();
            decoded.push(Record {
                seq: entry.seq,
                key: entry.key,
                op,
            });
        }

        Ok(decoded)
    }

    pub(crate) fn get(&self, key: &[u8]) -> Result<BlockLookup, SstCorruption> {
        // binary search: the last restart whose full key is <= the key we want
        let mut low: usize = 0;
        let mut high: usize = self.restarts.len();
        while low < high {
            let mid: usize = (low + high) / 2;
            let restart_key: Vec<u8> = self.decode_entry(self.restarts[mid], &[])?.key;
            if restart_key.as_slice() <= key {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        if low == 0 {
            // smaller than the first key in the block
            return Ok(BlockLookup::NotFound);
        }

        // linear scan from that restart until the key is found or passed
        let run_start: usize = self.restarts[low - 1];
        let run_end: usize = match self.restarts.get(low) {
            Some(&next_restart) => next_restart,
            None => self.entries.len(),
        };

        let mut offset: usize = run_start;
        let mut prev_key: Vec<u8> = Vec::new();
        while offset < run_end {
            let entry: EntryRef<'a> = self.decode_entry(offset, &prev_key)?;
            if entry.key.as_slice() == key {
                return match to_op(entry.type_byte, entry.value)? {
                    Op::Put { value } => Ok(BlockLookup::Found(value)),
                    Op::Delete => Ok(BlockLookup::Deleted),
                };
            }
            if entry.key.as_slice() > key {
                break;
            }
            offset = entry.next_offset;
            prev_key = entry.key;
        }

        Ok(BlockLookup::NotFound)
    }

    // decodes the entry at offset. prev_key is the previous entry's full key (empty at a restart)
    fn decode_entry(&self, offset: usize, prev_key: &[u8]) -> Result<EntryRef<'a>, SstCorruption> {
        let entries: &'a [u8] = self.entries;
        let header_end: usize = offset
            .checked_add(ENTRY_HEADER_LEN)
            .ok_or(SstCorruption::BadEntry)?;
        if header_end > entries.len() {
            return Err(SstCorruption::BadEntry);
        }

        let type_byte: u8 = entries[offset];
        let shared: usize = entries[offset + 1] as usize;
        let unshared: usize = entries[offset + 2] as usize;
        let value_len: usize =
            u32::from_le_bytes(entries[offset + 3..offset + 7].try_into().unwrap()) as usize;
        let seq: u64 = u64::from_le_bytes(entries[offset + 7..header_end].try_into().unwrap());

        if shared > prev_key.len() {
            return Err(SstCorruption::BadEntry);
        }
        let key_end: usize = header_end + unshared;
        let value_end: usize = key_end
            .checked_add(value_len)
            .ok_or(SstCorruption::BadEntry)?;
        if value_end > entries.len() {
            return Err(SstCorruption::BadEntry);
        }

        let mut key: Vec<u8> = Vec::with_capacity(shared + unshared);
        key.extend_from_slice(&prev_key[..shared]);
        key.extend_from_slice(&entries[header_end..key_end]);

        Ok(EntryRef {
            key,
            seq,
            type_byte,
            value: &entries[key_end..value_end],
            next_offset: value_end,
        })
    }
}

fn to_op(type_byte: u8, value: &[u8]) -> Result<Op, SstCorruption> {
    match type_byte {
        TYPE_PUT => Ok(Op::Put {
            value: value.to_vec(),
        }),
        TYPE_DELETE if value.is_empty() => Ok(Op::Delete),
        _ => Err(SstCorruption::BadEntry),
    }
}

#[cfg(test)]
mod tests;
