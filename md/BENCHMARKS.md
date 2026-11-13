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

## See also

DESIGN.md, section 10 · [WAL.md](WAL.md)
