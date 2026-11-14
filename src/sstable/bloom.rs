use super::block::CRC_LEN;
use super::error::SstCorruption;

// bloom section layout: | num_hashes 1B | bit array | CRC32 4B |
// the filter can say "maybe here" for a key that isn't, but never "not here" for a key
// that is. a corrupted filter could break that, so it has its own CRC
pub(crate) struct BloomFilter {
    bits: Vec<u8>,
    num_hashes: u8,
}

impl BloomFilter {
    // built from the stable hash of every key in the SSTable, tombstones included
    pub(crate) fn build(key_hashes: &[u64], bits_per_key: usize) -> BloomFilter {
        // small filters have a high false positive rate, so use at least 64 bits
        let num_bits: usize = (key_hashes.len() * bits_per_key).max(64);
        let num_bytes: usize = num_bits.div_ceil(8);

        let mut filter: BloomFilter = BloomFilter {
            bits: vec![0u8; num_bytes],
            num_hashes: num_hashes_for(bits_per_key),
        };
        for &hash in key_hashes {
            for bit in filter.bit_positions(hash) {
                filter.bits[bit / 8] |= 1 << (bit % 8);
            }
        }
        filter
    }

    pub(crate) fn may_contain(&self, key: &[u8]) -> bool {
        self.bit_positions(stable_hash(key))
            .all(|bit| self.bits[bit / 8] & (1 << (bit % 8)) != 0)
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut encoded: Vec<u8> = Vec::with_capacity(1 + self.bits.len() + CRC_LEN);
        encoded.push(self.num_hashes);
        encoded.extend_from_slice(&self.bits);
        let checksum: u32 = crc32fast::hash(&encoded);
        encoded.extend_from_slice(&checksum.to_le_bytes());
        encoded
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<BloomFilter, SstCorruption> {
        // num_hashes + at least one byte of bits + CRC
        if bytes.len() < 1 + 1 + CRC_LEN {
            return Err(SstCorruption::BadBloom);
        }
        let content_len: usize = bytes.len() - CRC_LEN;
        let stored_crc: u32 = u32::from_le_bytes(bytes[content_len..].try_into().unwrap());
        if crc32fast::hash(&bytes[..content_len]) != stored_crc {
            return Err(SstCorruption::BloomCrc);
        }

        let num_hashes: u8 = bytes[0];
        if num_hashes == 0 {
            return Err(SstCorruption::BadBloom);
        }
        Ok(BloomFilter {
            bits: bytes[1..content_len].to_vec(),
            num_hashes,
        })
    }

    // double hashing: k bit positions from one 64-bit hash, h1 + i * h2
    fn bit_positions(&self, hash: u64) -> impl Iterator<Item = usize> + use<> {
        let num_bits: u64 = (self.bits.len() * 8) as u64;
        let h1: u64 = hash & 0xFFFF_FFFF;
        // odd step so every probe lands on a different bit
        let h2: u64 = (hash >> 32) | 1;
        (0..self.num_hashes as u64)
            .map(move |i| (h1.wrapping_add(i.wrapping_mul(h2)) % num_bits) as usize)
    }
}

// k = bits_per_key * ln(2) is the number of hashes with the lowest false positive rate
fn num_hashes_for(bits_per_key: usize) -> u8 {
    let k: usize = (bits_per_key as f64 * std::f64::consts::LN_2).round() as usize;
    k.clamp(1, 30) as u8
}

// the bloom bits are saved on disk, so the hash must never change between builds or Rust
// versions. std's DefaultHasher doesn't promise that. FNV-1a, then a splitmix64 finish to mix the bits
pub(crate) fn stable_hash(key: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in key {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }

    hash ^= hash >> 30;
    hash = hash.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash ^= hash >> 27;
    hash = hash.wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^= hash >> 31;
    hash
}

#[cfg(test)]
mod tests;
