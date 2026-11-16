# Benchmarks

**Status:** Criterion benchmarks for CRC-32, memtable get, fsync / group commit / `Store::put`, and a full-size SSTable, all run on the Mac (Nov 16). Rerun on Linux before using as results. `Mutex` vs `RwLock` comes after compaction (PR 4e).

## Questions to answer

- Which CRC is faster for our record sizes, crc32fast or crc32c?
- What limits put throughput?
- How much does group commit help over fsync every write?
- Shared read state: does `Mutex` hold back concurrent gets enough to need `RwLock`? (week 3, see [FLUSH.md](FLUSH.md))

## Workloads

<!-- F1 replay, synthetic, YCSB-style (uniform, Zipfian). Mix of puts/gets/scans. -->
- Microbenchmarks so far: single record sizes from the design
    - 22 B: tombstone
    - 2 KB: typical sector chunk
    - 5 KB: slowest measured sector (242 samples)
    - 64 KB: max value
- F1 replay (later)
    - Live: ~1.4 puts/s (20 cars x 2 streams, one chunk per ~28 s sector)
    - Max speed: ~6,000 puts per race

## Variables

<!-- Chunk size, fsync policy, block size, bloom bits, compaction strategy, ... -->
- CRC algorithm: crc32fast vs crc32c
- fsync policy: every write (v1), group commit, periodic
- State lock: `Mutex` (v1) vs `RwLock`, with 1 / 2 / 4 / 8 reader threads + 1 writer

## Metrics

- Time per operation (p50, p99, max)
- Throughput (appends/s)

## Setup

- MacBook Pro, Apple Silicon (aarch64), macOS
- `--release` builds
- CRC: criterion 0.8.2, `benchmark/crc.rs`, run with `cargo bench --bench crc`
- Memtable get: criterion, `benchmark/memtable_get.rs`, run with `cargo bench --bench memtable_get`
- fsync: criterion, `benchmark/fsync.rs`, run with `cargo bench --bench fsync`
    - Raw appends of a 2,030 byte record (2 KB put with the 8-byte seq): write only, write + `sync_data`, write + `sync_all`
    - Group commit preview: 1 / 4 / 16 / 64 records, one `sync_data`
    - `Store::put` end to end, flush threshold raised to 1 GiB so no flush lands in the measurement
- Full SSTable: criterion, `benchmark/sstable.rs`, run with `cargo bench --bench sstable`
    - Fake data with the real shape: 12-byte telemetry keys, 20 cars, 2,000 byte values, filled until the 32 MiB flush threshold like the store does
    - No value compression, so value contents don't change the timings, only their size. F1 data isn't needed here
    - Page cache warm. A cold cache needs Linux (`drop_caches`)
- macOS `sync_data` = `F_FULLFSYNC`, which is known to be slow. Final numbers come from Linux
- `sync_all` is `F_FULLFSYNC` on macOS too, so the two cost the same there. On Linux they're `fdatasync` vs `fsync`

## Comparisons

<!-- RocksDB, sled: how to make the comparison fair. -->

## Results

### CRC-32: crc32fast vs crc32c (2026-11-11, Mac)

Short run (0.5 s per case), rough numbers.

| Buffer | crc32fast | crc32c |
|---|---|---|
| 22 B tombstone | 2.7 ns | 5.4 ns |
| 2 KB typical | 30 ns | 110 ns |
| 5 KB max sector | 64 ns | 239 ns |
| 64 KB max value | 0.73 us | 2.4 us |

- crc32fast was 3-4x faster at every size, so the WAL uses crc32fast
- Opposite of the common assumption that Castagnoli is faster. Rerun on Linux (x86), the result can depend on the CPU

### Append: write vs write + fsync (2026-11-13, Mac)

| Step (2,022 byte record) | Time |
|---|---|
| CRC-32 (crc32fast) | ~0.03 us |
| Write without fsync | ~3 us |
| Write + fsync | ~3.0 ms p50, 4.8 ms p99, 9.9 ms max |
| Throughput with fsync | ~350 appends/s |

- fsync is ~99.9% of a put. The CRC choice and encode speed don't change put throughput
- At ~350/s, live F1 ingest (~1.4 puts/s) is fine, but replaying a race at max speed (~6,000 puts) takes ~17 s waiting on fsync
- Group commit is the next thing worth measuring

### fsync, group commit, Store::put (2026-11-16, Mac, criterion)

Short run (3 s per case), rough numbers. 2,030 byte records.

| Case | Time per call | Throughput |
|---|---|---|
| Write only | ~2.7 us | ~364,000 appends/s |
| Write + `sync_data` | ~3.2 ms | ~314 appends/s |
| Write + `sync_all` | ~3.2 ms | ~317 appends/s |
| `Store::put` (2 KB value) | ~3.1 ms | ~320 puts/s |

| Records per `sync_data` | Time per batch | Records/s |
|---|---|---|
| 1 | ~3.0 ms | ~328 |
| 4 | ~3.1 ms | ~1,280 |
| 16 | ~3.2 ms | ~4,980 |
| 64 | ~3.2 ms | ~20,100 |

- Matches the Nov 13 loop: fsync is the whole put. `Store::put` is no slower than a raw write + `sync_data`, so encode, the WAL lock, and the memtable insert don't show up
- One fsync costs the same for 2 KB or 128 KB (64 records), so group commit throughput grows almost 1:1 with the batch size
- A race replay at max speed (~6,000 puts): ~19 s at one fsync per put, well under 1 s at 16+ per fsync
- Group commit only helps when several writers are waiting at once. Week 6 has to check how many puts really queue up behind the WAL lock

### Memtable get: borrow vs copy vs allocate + copy (2026-11-13, Mac)

Question: `Store::get` returns an owned `Vec<u8>` (one ~2 KB copy per read). Is the allocation more expensive than the copy itself?

- 16,384 entries x 2,000 byte values (one full 32 MiB memtable), lookups jump around the map
- `benchmark/memtable_get.rs`, short run (3 s per case)

| Case | Time per get |
|---|---|
| Borrow only (`Lookup::Found(&[u8])`, no copy) | ~113 ns |
| Copy into a reused buffer (copy, no allocation) | ~362 ns |
| `to_vec` (allocation + copy, what `Store::get` does) | ~312 ns |

- Allocation is not the cost: `to_vec` was no slower than copying into a reused buffer (the gap is noise)
- The ~200-250 ns extra is reading 2 KB of value bytes from RAM. A 32 MiB memtable doesn't fit in the CPU cache, so the copy waits on memory
- The borrow case never reads the value bytes, so it flatters zero-copy. A real caller would read those bytes too and pay most of that memory cost anyway
- Owned values stay for now. Later, SSTable reads (disk, us to ms) will make ~0.3 us per copy even less significant

### SSTable: full 32 MiB flush (2026-11-13, Mac)

- One full memtable: 16,678 telemetry entries x 2,000 byte values, 20 cars, default config (16 KiB blocks, restart interval 16, 6 bloom bits/key)
- First run: quick timed program outside the repo, page cache warm. Now `benchmark/sstable.rs`, rerun Nov 16 with criterion (short run, 3 s per case)

| Measurement | Result |
|---|---|
| SSTable size | 32.1 MiB (raw key + value bytes 32.0 MiB), 1,854 blocks. Criterion: 33,750,450 bytes, raw 33,556,136 |
| Write + fsync + rename (one whole flush) | ~40 ms. Criterion: ~57 ms (44-64 ms) |
| Prefix compression savings | 189 KB, 0.56% |
| Bloom filter size | 12.5 KB |
| Open (footer, index, bloom filter) | Criterion: ~62 us |
| get, key present | ~3.2 us. Criterion: ~2.1 us |
| get, key missing, with bloom filter | ~0.18 us. Criterion: ~0.20 us |
| get, key missing, no bloom filter | ~1.67 us. Criterion: ~2.1 us |
| Iterate all entries | ~6 ms. Criterion: ~5.1 ms |

- A flush takes ~40 ms, filling a 32 MiB memtable at ~350 puts/s takes ~45 s, so the background flush thread will mostly be idle
- Prefix compression barely helps here: keys are 12 bytes next to 2 KB values
- The bloom filter makes missing-key lookups ~9x faster even with the file cached. With a cold cache the miss without a filter is a disk read
- Criterion's missing keys sort between real keys (same key, seq 2), so without the filter every miss reads and searches a block: costs the same as a hit. The bloom filter makes misses ~10x faster
- Rerun on Linux with a cold page cache before using these numbers

Revisit zero-copy reads when:
- We're doing hundreds of thousands or millions of reads per second
- Profiling shows allocation or memory-copy overhead is significant
- We're repeatedly copying the same payload through multiple layers
- The workload is latency-sensitive

## See also

DESIGN.md, section 10 · [WAL.md](WAL.md)
