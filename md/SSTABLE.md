# SSTable

**Status:** Writer and reader done and tested (bloom filter included). Flush, compaction, and wiring into the Store come next.

## Purpose

<!-- What is an SSTable, and why is it immutable? -->
- A sorted, immutable file on disk. One flush of the memtable writes one SSTable
- Immutable: never changed after it's written. Deletes are tombstones, overwrites are newer entries in newer files, compaction writes new files
- Everything in one SSTable comes from one flush, so nothing inside it is newer or older

## File layout

<!-- Data blocks, bloom filter, index, footer: in what order? Draw it. -->
```
| data block 0 | CRC32 |
| data block 1 | CRC32 |
| ...                  |
| bloom filter | CRC32 |   (missing when bloom_bits_per_key = 0)
| index        | CRC32 |
| footer (22 bytes)    |   read first
```
- All integers little-endian, keys stay big-endian (sorted byte order)
- Every section has its own CRC32, the footer has a magic number + format version
- Written to `<name>.tmp`, fsynced, renamed to `<name>`, then the directory is fsynced. A crash never leaves a half-written file that looks like a real SSTable

## Data blocks

<!-- Block size and why. How are entries laid out inside a block? What if a value is bigger than a block? Block CRC? -->
- Block size: 16 KiB soft target (`sstable.block_size_bytes`)
    - A block is closed once it reaches 16 KiB, an entry is never split
    - A value bigger than a block (up to 64 KiB) gets a block to itself
    - With 2 KB values a block holds ~8 entries, a 32 MiB SSTable has ~1,850 blocks

- Prefix compression (LevelDB style)
    - Each key stores only the bytes it doesn't share with the previous key
    - Every 16th entry is a restart point and stores its full key (`sstable.restart_interval`)
    - Lookup: binary search the restart points, then scan at most 16 entries

```
entry:   | type 1B | shared 1B | unshared 1B | valueLen 4B | key suffix | value |
block:   | entry | entry | ... | restart offsets 4B each | restart count 4B | CRC32 4B |
```

- type: 01 = Put, 02 = Delete (same as the WAL). A tombstone has valueLen 0
- Measured on a full 32 MiB SSTable of 2 KB telemetry values: prefix compression saved 0.56% (189 KB). Small, since keys are 12 bytes next to 2 KB values, as expected

## Index

<!-- What is stored per block? First key or last key, and why? -->
```
| keyLen 1B | last key | block offset 4B | block len 4B | ... | CRC32 4B |
```
- One entry per block: the block's last key, where it starts, and its length (CRC included)
- Last key, not first key: binary search for the first block whose last key >= the key
    - That block is the only one that could hold the key
    - If no block's last key is that big, the key isn't in this file and no block is read
- Kept in memory once the SSTable is open

## Bloom filter

- Built from every key in the file, tombstones included (a lookup of a deleted key must find its tombstone)
- 6 bits per key (~5-6% false positives), 4 hash functions, see [BLOOM_FILTER.md](BLOOM_FILTER.md)
- Checked before the index: "definitely not here" means no block read

## Footer

<!-- Every field with its width and byte order. Total footer size. -->
| Field | Width |
|---|---|
| index offset | 4 bytes |
| bloom offset | 4 bytes |
| index len | 4 bytes |
| bloom len | 4 bytes (0 = no bloom filter) |
| format version | 2 bytes (1) |
| magic number | 4 bytes (`0x4C534D4B`, "LSMK") |
| **Total** | **22 bytes** |

- The reader seeks to file length - 22 and reads the footer first
- Offsets are u32, so an SSTable is capped at 4 GiB (writer returns `FileTooLarge`). Fine for 32 MiB files
- Open checks: magic, version, every section and block sits before the footer

## Sizing

<!-- Target SSTable size and the math behind it. -->
- SSTable size = memtable flush threshold, 32 MiB by default
- One race is ~13 MB of data, so one SSTable holds ~2.5 races

| Full 32 MiB memtable (16,678 x 2 KB entries, Mac) | Result |
|---|---|
| SSTable size | 32.1 MiB, 1,854 blocks |
| Write + fsync + rename | ~40 ms |
| Bloom filter | 12.5 KB |

## Reading a key

<!-- Step by step: how a get uses the footer, bloom filter, index, and block. -->
1. (At open) read the footer, check magic + version, load the index and bloom filter into memory
2. Bloom filter says no -> `NotFound`, no disk read
3. Binary search the index for the first block whose last key >= key. None -> `NotFound`
4. Read that one block (`read_exact_at`, no seek) and check its CRC
5. Binary search the restart points, then scan forward -> `Found(value)` / `Deleted` / `NotFound`

| get (Mac, page cache warm) | Time |
|---|---|
| Key present | ~3.2 us |
| Key missing, with bloom filter | ~0.18 us |
| Key missing, no bloom filter | ~1.67 us |

## Errors

- `SstError::Corruption { offset, reason }`: offset is where the bad section starts in the file
    - `BlockCrc`, `IndexCrc`, `BloomCrc`, `BadMagic`, `UnsupportedVersion`, `SectionOutOfBounds`, `FileTooSmall`, `BadBlock`, `BadEntry`, `BadIndex`, `BadBloom`
- Writer: `OutOfOrder` (keys must strictly increase), `KeyTooLong`, `Empty` (no entries, no file is left behind), `FileTooLarge`
- `SstError::Io` for file errors

## Interface

<!-- Writer and reader functions (names and signatures, no bodies). -->
- Writer
    - `SstWriter::create(path: &Path, config: &SstableConfig) -> Result<SstWriter, SstError>`
    - `add(&mut self, key: &[u8], op: &Op) -> Result<(), SstError>`: keys in increasing order (memtable.iter() order)
    - `finish(self) -> Result<SstInfo, SstError>`: writes bloom, index, footer, fsyncs, renames into place
    - `SstInfo { path, entry_count, file_size, first_key, last_key }`
- Reader
    - `SstReader::open(path: &Path) -> Result<SstReader, SstError>`
    - `get(&self, key: &[u8]) -> Result<SstLookup, SstError>`: `Found(Vec<u8>)` / `Deleted` / `NotFound`
    - `iter(&self) -> SstIter`: every entry in key order, tombstones included. Stops at the first bad block instead of skipping it
    - `block_count(&self)`, `path(&self)`
- Reads take `&self` (`read_exact_at` doesn't move a file position), so one open SSTable can be shared with the flush thread later without a lock
- `read_exact_at` is Unix only (Linux and macOS)

## Tests

- Block (`src/sstable/block/tests.rs`): round trip, prefix compression shrinks the block, get every key for restart intervals 1 / 2 / 3 / 16 / 100, missing keys, bit flips fail the CRC, every truncation is an error not a panic, bad entry with a valid CRC, builder resets between blocks
- Bloom (`src/sstable/bloom/tests.rs`): pinned hash values, no false negatives, false positive rate at 6 and 10 bits/key, k per bits/key, encode/decode, CRC, every truncation
- Index + footer (`src/sstable/index/tests.rs`): round trip, find block by last key, CRC, every truncation, footer is 22 bytes, bad magic / version, sections must fit before the footer
- SSTable (`src/sstable/reader/tests.rs`): write a memtable then get every key (values and tombstones), missing keys, iter matches the memtable, first / last key, 64 KiB value bigger than a block, no bloom filter, bloom skips most missing keys, out of order and duplicate keys rejected, empty SSTable leaves no files, file only appears after finish, corrupt block reports its offset, bad magic, every truncation of a whole file fails open, random bytes fail open

## Open questions

- Block cache: none in v1, the OS page cache handles it. Revisit with cold-cache benchmarks on Linux
- Prefix compression only saved 0.56% here. Keep it (restart points give the in-block binary search), but it isn't a size win for this data
- File naming and which SSTables are live: comes with the manifest in week 3

## See also

DESIGN.md, section 7 · [BLOOM_FILTER.md](BLOOM_FILTER.md) · [CONFIG.md](CONFIG.md) · [BENCHMARKS.md](BENCHMARKS.md)
