# Benchmarks

**Status:** First microbenchmarks done on the Mac (CRC-32, write + fsync). Rerun on Linux before using as results.

## Questions to answer

- Which CRC is faster for our record sizes, crc32fast or crc32c?
- What limits put throughput?
- How much does group commit help over fsync every write?

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

## Metrics

- Time per operation (p50, p99, max)
- Throughput (appends/s)

## Setup

- MacBook Pro, Apple Silicon (aarch64), macOS
- `--release` builds
- CRC: criterion 0.8.2, `benchmark/crc.rs`, run with `cargo bench --bench crc`
- Memtable get: criterion, `benchmark/memtable_get.rs`, run with `cargo bench --bench memtable_get`
- Write + fsync: quick timed loop of 300 appends of a 2,022 byte record, `write_all` + `sync_data` per append. Not in the repo yet, should become a criterion bench in `benchmark/`
- macOS `sync_data` = `F_FULLFSYNC`, which is known to be slow. Final numbers come from Linux

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
- Quick timed program outside the repo (like the fsync loop), page cache warm. Should become a criterion bench

| Measurement | Result |
|---|---|
| SSTable size | 32.1 MiB (raw key + value bytes 32.0 MiB), 1,854 blocks |
| Write + fsync + rename (one whole flush) | ~40 ms |
| Prefix compression savings | 189 KB, 0.56% |
| Bloom filter size | 12.5 KB |
| get, key present | ~3.2 us |
| get, key missing, with bloom filter | ~0.18 us |
| get, key missing, no bloom filter | ~1.67 us |
| Iterate all entries | ~6 ms |

- A flush takes ~40 ms, filling a 32 MiB memtable at ~350 puts/s takes ~45 s, so the background flush thread will mostly be idle
- Prefix compression barely helps here: keys are 12 bytes next to 2 KB values
- The bloom filter makes missing-key lookups ~9x faster even with the file cached. With a cold cache the miss without a filter is a disk read
- Rerun on Linux with a cold page cache before using these numbers

Revisit zero-copy reads when:
- We're doing hundreds of thousands or millions of reads per second
- Profiling shows allocation or memory-copy overhead is significant
- We're repeatedly copying the same payload through multiple layers
- The workload is latency-sensitive

## See also

DESIGN.md, section 10 · [WAL.md](WAL.md)
