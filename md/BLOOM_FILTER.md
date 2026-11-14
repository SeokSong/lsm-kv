# Bloom Filter

**Status:** Done and tested, one filter per SSTable.

## Purpose

<!-- What lookups does it save? What can it never get wrong, and what can it get wrong? -->
- Lets a get skip an SSTable without reading a block: "definitely not here" or "maybe here"
- Never wrong about a key that is in the file (no false negatives)
- Sometimes says "maybe" for a key that isn't (false positive), then the block read finds nothing
- Matters most once there are many SSTables: a missing key would otherwise read one block from every file

## How it works

<!-- Bits, hash functions, insert, check. -->
- A bit array, all zeros to start
- Insert: hash the key, set k bits
- Check: hash the key, if any of the k bits is 0 the key is definitely not there
- Every key in the SSTable goes in, tombstones included, so a lookup of a deleted key still finds its tombstone

## Parameters

<!-- Bits per key, number of hash functions, expected false-positive rate, and the math. -->
- `sstable.bloom_bits_per_key` = 6 (0 turns the filter off)
- k = bits_per_key x ln(2), rounded: 4 hashes at 6 bits/key, 7 at 10 bits/key
- At least 64 bits even for tiny files

| Bits per key | Hashes | Expected false positives | Measured (20,000 missing keys) |
|---|---|---|---|
| 6 | 4 | ~5.6% | under 8% (test bound) |
| 10 | 7 | ~0.8% | under 1.5% (test bound) |

- Size: 6 bits/key x 16,678 keys = 12.5 KB for a full 32 MiB SSTable (0.04% of the file)

## Hashing

<!-- Which hash function(s)? How do you get k hashes from one? -->
- One 64-bit hash per key, split into two halves for double hashing: bit i = h1 + i x h2
    - h2 is forced odd so every probe lands on a different bit
- The hash must never change: the bits are saved on disk
    - std's `DefaultHasher` doesn't promise that across Rust versions
    - Uses FNV-1a 64 then a splitmix64 finish to mix the bits, written out in `src/sstable/bloom.rs`
    - A test pins three hash values (computed separately) so a change can't slip in

## On-disk format

<!-- How is it stored inside the SSTable? -->
```
| num_hashes 1B | bit array | CRC32 4B |
```
- Sits between the data blocks and the index, footer has its offset and length (length 0 = no filter)
- Has its own CRC: a corrupted filter could say "not here" for a key that is there, which would be a wrong answer, not just a slow one

## Measured

| get, key missing (Mac, page cache warm) | Time |
|---|---|
| With bloom filter | ~0.18 us |
| Without bloom filter | ~1.67 us |

- ~9x faster with the page cache warm. With a cold cache the miss without a filter is a real disk read

## Interface

- Internal to the SSTable module: `BloomFilter::build(key_hashes, bits_per_key)`, `may_contain(key)`, `encode()`, `decode(bytes)`, `stable_hash(key)`

## Tests

<!-- How will you measure the real false-positive rate? -->
- `src/sstable/bloom/tests.rs`: pinned hash values, no false negatives over 10,000 keys, false positive rate at 6 and 10 bits/key over 20,000 missing keys, k per bits/key, encode/decode round trip, bit flip fails the CRC, every truncation is an error
- `src/sstable/reader/tests.rs`: bloom skips most missing keys in a real SSTable, SSTable without a filter still works

## Open questions

- 6 vs 10 bits per key: measure the read path once there are many SSTables (week 3)

## See also

[SSTABLE.md](SSTABLE.md) · [CONFIG.md](CONFIG.md)
