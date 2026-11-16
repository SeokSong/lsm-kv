# Benchmarks

**Status:** Criterion benchmarks for CRC-32, memtable get, fsync / group commit / `Store::put`, and a full-size SSTable, run on the Mac and on Linux in the Docker dev container (Nov 16). CPU and memory numbers from the container are usable, its fsync numbers aren't. Linux x86 numbers, fsync included, come from the `Bench` GitHub Actions workflow (4 runs, Nov 16). `Mutex` vs `RwLock` comes after compaction (PR 4e).

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

- MacBook Pro, Apple M3 Pro (aarch64), macOS
- Linux: the dev container (`Dockerfile`, `compose.yaml`), Docker Desktop's Linux VM, kernel 6.12, Rust 1.100, same M3 Pro cores
    - Run with `docker compose run --rm dev cargo bench`
    - Good for CPU and memory work (CRC, memtable, warm-cache SSTable reads): Linux kernel and allocator, real cores
    - Not for fsync: it reaches the VM's virtual disk, and the hypervisor decides when that reaches the SSD. Not for cold-cache reads either: macOS can cache the VM's disk image
    - Not x86: x86 numbers come from GitHub Actions
- GitHub Actions: `.github/workflows/bench.yml`, started by hand (Actions tab -> Bench -> Run workflow, or `gh workflow run bench.yml`)
    - `ubuntu-latest` runner: Azure VM, 4 cores, x86. The CPU and disk change between runs, each run saves them in `machine.txt`
    - Results: the run's summary page, and a zip (`bench.txt`, `machine.txt`, criterion HTML report) kept 90 days. `gh run download <run id>`
    - Free for a public repo
- `--release` builds
- CRC: criterion 0.8.2, `benchmark/crc.rs`, run with `cargo bench --bench crc`
- Memtable get: criterion, `benchmark/memtable_get.rs`, run with `cargo bench --bench memtable_get`
- fsync: criterion, `benchmark/fsync.rs`, run with `cargo bench --bench fsync`
    - Raw appends of a 2,030 byte record (2 KB put with the 8-byte seq): write only, write + `sync_data`, write + `sync_all`
    - Group commit preview: 1 / 4 / 16 / 64 records, one `sync_data`
    - `Store::put` end to end, flush threshold raised to 1 GiB so no flush lands in the measurement
- Merge: criterion, `benchmark/merge.rs`, run with `cargo bench --bench merge`
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

### Linux in the dev container vs Mac (2026-11-16)

Short run (3 s per case), every benchmark in one `cargo bench` run. Mac numbers are the runs above.

| Benchmark | Mac | Linux (Docker VM) |
|---|---|---|
| crc32fast, 22 B / 2 KB / 5 KB / 64 KB | 2.7 ns / 30 ns / 64 ns / 0.73 us | 3.8 ns / 39 ns / 89 ns / 1.1 us |
| crc32c, 22 B / 2 KB / 5 KB / 64 KB | 5.4 ns / 110 ns / 239 ns / 2.4 us | 9.4 ns / 190 ns / 436 ns / 5.0 us |
| Memtable get: borrow / copy / `to_vec` | 113 / 362 / 312 ns | 165 / 394 / 388 ns |
| SSTable open | ~62 us | ~50 us |
| SSTable get, present | ~2.1 us | ~2.7 us |
| SSTable get, missing, bloom / no bloom | ~0.20 / ~2.1 us | ~0.20 / ~2.4 us |
| SSTable iterate all | ~5.1 ms | ~6.2 ms |
| Write only, 2,030 B | ~2.7 us | ~5.7 us |
| Write + `sync_data` | ~3.2 ms | 1.2-130 ms (unusable) |
| Write + `sync_all` | ~3.2 ms | ~1.3 ms (unusable) |
| `Store::put` | ~3.1 ms | 0.3-47 ms (unusable) |
| Group commit, 1 / 4 / 16 / 64 records per sync | ~3.0-3.2 ms per batch | 338 ms / 0.53 / 0.64 / 0.70 ms per batch (unusable) |
| Full flush, 32 MiB | ~57 ms | ~76 ms (has fsyncs, unusable) |

- crc32fast is still 2.5-4.5x faster than crc32c on Linux, so the WAL choice holds on aarch64 Linux too. x86 is still unchecked
- CPU and memory work is 0-40% slower in the container than on macOS. The same SSTable benchmark run alone earlier that day was 10-20% faster than the Mac, so run-to-run noise in the VM is about as big as the gap. Don't read small differences into these
- fsync in the VM is all over the place: the same `sync_data` call took 1.2 ms to 130 ms in one run, `sync_all` (more work on real Linux) came out faster than `sync_data`, and 1 record per sync was slower than 64. That's the virtual disk, not Linux
- The Mac's ~3 ms fsync stays the reference until there's a run on real Linux hardware with an NVMe drive (week 6, for group commit)

### Linux x86 on GitHub Actions, 4 runs (2026-11-16)

5 s per case, every benchmark, commit `main` after PR #14. Each run got a different runner:

| Run | CPU (4 cores) | Disk under `/tmp` |
|---|---|---|
| A | AMD EPYC 9V74 | Azure NVMe (`MSFT NVMe Accelerator`) |
| B | AMD EPYC 7763 | Azure `Virtual Disk` |
| C | AMD EPYC 9V45 | Azure NVMe (`MSFT NVMe Accelerator`) |
| D | AMD EPYC 7763 | Azure `Virtual Disk` |

| Benchmark | A | B | C | D | Spread | Mac |
|---|---|---|---|---|---|---|
| crc32fast 22 B / 2 KB / 64 KB | 8 ns / 58 ns / 1.1 us | 10 ns / 87 ns / 2.6 us | 6 ns / 43 ns / 1.0 us | 10 ns / 87 ns / 2.6 us | 1.8-2.6x | 2.7 ns / 30 ns / 0.73 us |
| crc32c 22 B / 2 KB / 64 KB | 8 ns / 115 ns / 2.5 us | 10 ns / 129 ns / 2.8 us | 6 ns / 76 ns / 2.1 us | 10 ns / 129 ns / 2.8 us | 1.4-1.7x | 5.4 ns / 110 ns / 2.4 us |
| Memtable `to_vec` | 488 ns | 447 ns | 410 ns | 467 ns | 1.2x | 312 ns |
| Write only, 2,030 B | 9.1 us | 4.7 us | 9.3 us | 4.7 us | 2.0x | 2.7 us |
| Write + `sync_data` | 816 us | 254 us | 257 us | 375 us | 3.2x | 3.2 ms |
| Write + `sync_all` | 727 us | 253 us | 222 us | 364 us | 3.3x | 3.2 ms |
| `Store::put` | 299 us | 254 us | 240 us | 328 us | 1.4x | 3.1 ms |
| Group commit 1 / 4 / 16 / 64 per sync | 452 / 488 / 783 / 1,116 us | 252 / 327 / 395 / 620 us | 217 / 271 / 331 / 710 us | 350 / 387 / 440 / 624 us | 1.8-2.4x | 3.0-3.2 ms each |
| SSTable flush, 32 MiB | 159 ms | 78 ms | 131 ms | 78 ms | 2.0x | 57 ms |
| SSTable open | 72 us | 90 us | 82 us | 90 us | 1.2x | 62 us |
| SSTable get, present | 3.6 us | 3.7 us | 2.4 us | 4.0 us | 1.7x | 2.1 us |
| SSTable get, missing, bloom / no bloom | 0.25 / 3.6 us | 0.28 / 3.6 us | 0.16 / 2.3 us | 0.28 / 3.8 us | 1.6-1.8x | 0.20 / 2.1 us |
| SSTable iterate all | 7.7 ms | 8.5 ms | 5.4 ms | 8.7 ms | 1.6x | 5.1 ms |

- The noise is which machine you get, not run to run: B and D (same CPU, same disk) are within ~5% on every CPU benchmark. Only compare runs on the same CPU model, and write the CPU next to every number
- fsync: `Store::put` is 0.24-0.33 ms on every runner, ~3,000-4,000 puts/s, ~10x the Mac. The Mac's `F_FULLFSYNC` waits for the drive's own cache. Azure's disks acknowledge the flush earlier and protect the data on their side, so the number reflects that storage, not "Linux". Production databases on cloud VMs get the same
- A race replay at max speed (~6,000 puts) takes ~1.5-2 s here, against ~19 s on the Mac
- `sync_all` costs the same as `sync_data`: every WAL append grows the file, so `fdatasync` has to flush the size change anyway. `sync_data` stays, no reason to change
- Group commit still pays off, but less than on the Mac: 64 records per sync costs ~2.5x one record (on the Mac it cost the same). Per record that's ~10 us at 64 vs ~250 us at 1, so ~25x the throughput. Week 6 still worth doing
- CRC: crc32fast is never slower. Same speed at 22 B (x86 has a CRC32C instruction, so crc32c is fast on tiny buffers), 1.1-2x faster from 2 KB up. The WAL choice holds on x86 too
- The SSTable flush (78-159 ms) is slower than the Mac (57 ms) but still small next to the ~4-6 s it takes to fill a memtable (16,678 puts) at this put rate
- Bloom filter: ~13-15x faster misses on every runner

### Merge iterator (2026-11-16, Linux dev container)

`benchmark/merge.rs`, short run (3 s per case). 4 overlapping SSTables, ~8 MiB each, 42 MB total, 20,800 entries, 16,000 distinct keys.

| Case | Time | Throughput |
|---|---|---|
| Read the 4 files one after another | ~8.3 ms | ~4.7 GiB/s |
| Merge them (newest seq per key) | ~10.2 ms | ~3.8 GiB/s |

- The merge adds ~20% over plain reads. A compaction's cost is writing the output (50-160 ms per 32 MiB SSTable with fsync), not merging. Details in [MERGE.md](MERGE.md)

## See also

DESIGN.md, section 10 · [WAL.md](WAL.md)
