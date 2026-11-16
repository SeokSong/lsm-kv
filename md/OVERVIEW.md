# lsm-kv Project Overview

<!-- Fill this in yourself. Keep it short: someone should understand the project in 2 minutes. -->
Build a LSM (log strucuted merge tree) in rust to handle heavy write applications such as live data ingestion to a live feed. 
Example pulling formula one car data such as throttle, brake, steering angle and etc and displaying it live 
This can be extended to personal sims as well allowing users to track and visualize their telemetry without paying for subscribition services.
This will alos be pairs with a follow up c++ query service that will query the data written from the LSM and drivers and see their past telemetry and compare against friends and compeitoris (whoever is willing to share data)  


## What it is

<!-- One or two sentences: what lsm-kv is and what problem it solves. -->
A LSM KV solves the problem of heavy write applications.
This turns random writes into sequential writes, unlike a b tree that updates pages in place, a LSM will only append to a WAL and keep a sorted memtable in memory. When the memtable is full it gets flushed to disk as a sorted file (SSTable)


## Why I'm building it

<!-- What you want to learn or demonstrate. -->
I am building this to learn rust, learn more about rust as companies such as Discord and Dropbox use rust and those are target companies.
I am aiming for a mid level role and with my current data pipeline flow background. Working on a LSM KV tree could really round out my resume as a mid level data pipeline/workflow engineer 
Discord LSM based ScyllaDB with storage facing rust services  here: https://discord.com/blog/how-discord-stores-trillions-of-messages

## Data

<!-- Where the data comes from and how it reaches the store. -->
1. FastF1 data extracted through python scripts
2. Use replay to read exported Parquet files sorted by SessionTime.
    2_1. Emit each sample when its timestampe comes due by sleeping for a gap btween consective samples
    2_2. Add a speed multiplier to be able to feed the data in at 1x speed, 10x speed and nx speed to find the maximum preformance of our aplication with live data for benchmarks
    2_3. The ingest layer collects samples into a sector chunk and calls put when the sector ends, exactly as it would with live data.



## How it works (big picture)

<!-- The write path and read path in a few lines. A small ASCII diagram is fine. -->
![alt text](image-5.png)
Request will first check the memtable, if not found in memtable we will check the DB for the request
- Get 
    - Check the active memtable to see if key exist
    - Check the memtable being flushed
    - Check SSTables newest to oldest
        - Check against bloom filter to see if this exist in SSTables
    - if not found or TOMBSTONE return OK(None)
For the live feed, we will feed it to the WAL. 

- Write: 
    - user uses put(key, value) -> write operation to WAL
                                fsync
                                -> write to memtable
                                <- return success write
                                -> check memtable threshold against capacity
                                    -> if threshold >= capacity 
                                        -> start background Flush job 
The user can request the live feed but for now that is out of scope

## Components

| Component | One-line summary | Status | Doc |
|---|---|---|---|
| WAL | Append-only log on disk, every put/delete is written + fsynced here first so the memtable can be rebuilt after a crash | Done for v1: encode, decode, append + fsync, replay, sequence numbers (34 tests). One segment per memtable | [WAL.md](WAL.md) |
| Memtable | Sorted in-memory map of recent writes (values and tombstones) | In progress: BTreeMap memtable with seqs done (14 tests). Background flush + frozen queue next | [MEMTABLE.md](MEMTABLE.md) |
| SSTable | Immutable sorted file on disk: data blocks, index, bloom filter, footer | Done: writer + reader, prefix compression, block CRCs, tmp + rename (37 tests). Not wired into the store yet | [SSTABLE.md](SSTABLE.md) |
| Bloom filter | Per-SSTable bit array that says "definitely not here" to skip disk reads | Done: 6 bits/key, stable hash, own CRC | [BLOOM_FILTER.md](BLOOM_FILTER.md) |
| Flush | Writes a full memtable to a new SSTable | Done: four steps on a background thread, retries with growing wait, backpressure, crash tested at each step | [FLUSH.md](FLUSH.md) |
| Compaction | Merges SSTables, keeps the newest version of each key | Not started | [COMPACTION.md](COMPACTION.md) |
| Manifest | List of live SSTables and the current WAL, the source of truth on startup | Done: whole-file rewrite with CRC, tmp + rename, file naming, obsolete files (29 tests). Used by the Store | [MANIFEST.md](MANIFEST.md) |
| Concurrency | Lock design for the background flush: three locks, lock order, 10 rules to avoid deadlock | Done: built and tested, deadlock test checked to fail when a rule is broken | [CONCURRENCY.md](CONCURRENCY.md) |
| Crash recovery | Rebuilds state on startup from the manifest + WAL replay | Done for 3a: manifest, cleanup, SSTables, segment replay, tested at every flush step. kill -9 tests in week 5 | [RECOVERY.md](RECOVERY.md) |
| Benchmarks | Microbenchmarks now, F1 replay and YCSB-style workloads later | In progress: CRC-32, write + fsync, memtable get, full-size SSTable measured on Mac | [BENCHMARKS.md](BENCHMARKS.md) |
| Config | Tuning settings in TOML (`config/*.toml`), format constants stay in code | Done for memtable + SSTable settings (11 tests) | [CONFIG.md](CONFIG.md) |
| Store | Public API: open (replay), put / delete (WAL then memtable), get | Done for v1 writes + reads: WAL segments, manifest, background flush, get across memtables + SSTables, shareable across threads, close (39 tests). Scan in week 5 | [STORE.md](STORE.md) |

<!-- Status ideas: Not started / Designing / In progress / Done / Tested / Benchmarked -->

## Current status

<!-- Where the project is today, and what's next. -->
- Done
    - WAL record format: encode + decode with CRC-32, size limits, torn tail and corruption detection
    - WalWriter: append + fsync every write, failed state after a write or fsync error
    - WAL replay: decodes records in order, truncates a torn tail, reports corruption with the file offset
    - Memtable: BTreeMap of key -> Op, Lookup enum (Found / Deleted / NotFound), running size counter, sorted iter and range
    - Config: TOML tuning settings (memtable flush threshold, max frozen memtables), default + 16/64 MiB benchmark profiles
    - Store: open (replay, then WAL writer), put / delete (WAL first, then memtable), get from the memtable
    - SSTable: writer + reader with prefix-compressed 16 KiB blocks, last-key index, bloom filter, 22-byte footer, CRC on every section, tmp + rename
    - Sequence numbers: every write gets a u64 seq, stored in the WAL, the memtable, and every SSTable entry
    - Manifest: live WAL segments + SSTables (seq and key ranges), next file number, last seq, CRC, atomic save, finds obsolete files
    - Store on disk (PR 3a): WAL segments, flush to SSTables, get across every layer, crash recovery at each flush step, StoreError enum
    - Background flush + Store shareable across threads (PR 3b): three locks, backpressure, retries, close, concurrency tests under timeouts
    - 165 unit tests, CI on Linux and macOS
    - Benchmarks on Mac: crc32fast is 3-4x faster than crc32c, fsync is ~99.9% of a put (~3 ms, ~350 appends/s), memtable get allocation costs nothing measurable (copy ~0.2 us per 2 KB), a full 32 MiB SSTable flush takes ~40 ms, bloom makes missing-key gets ~9x faster
- Next
    - Week 4: compaction (size-tiered), atomic swap through the manifest, dropping tombstones
    - Benchmarks to turn into criterion benches: fsync, full SSTable, `Mutex` vs `RwLock` for reads
    - Read path across memtables + SSTables, StoreError becomes its own enum (WAL + SSTable errors)

## Milestones

Revised 2026-11-13: background flush pulls WAL segments and the manifest into week 3, and the ingest layer gets its own slot in week 6.

| Week | Dates | Goal | Done? |
|---|---|---|---|
| 0 | Nov 3-9 | Rust basics, repo, CI, design doc | Done |
| 1 | Nov 10-16 | WAL + replay, memtable, store (open / put / delete / get) | Done |
| 2 | Nov 17-23 | SSTable writer + reader: data blocks, index, footer, block CRC, point lookup, bloom filters | Done (Nov 13) |
| 3 | Nov 24-30 | Sequence numbers, minimal manifest, background flush, frozen memtable queue, WAL segments, read path across memtables + SSTables, Store shareable across threads | Done (Nov 13) |
| 4 | Dec 1-7 | Compaction (size-tiered), atomic swap through the manifest, dropping tombstones | |
| 5 | Dec 8-14 | Scan (merge iterator), crash tests (kill -9), property tests, recovery cleanup | |
| 6 | Dec 15-21 | Ingest (FastF1 -> Parquet -> keys / values), benchmarks on Linux, group commit, RocksDB / sled comparison, README | |

## Full design

See [DESIGN.md](DESIGN.md).
