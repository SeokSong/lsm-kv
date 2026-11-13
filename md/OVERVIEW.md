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
| WAL | Append-only log on disk, every put/delete is written + fsynced here first so the memtable can be rebuilt after a crash | Done for v1: encode, decode, append + fsync, replay (30 tests). Segments come with flush | [WAL.md](WAL.md) |
| Memtable | Sorted in-memory map of recent writes (values and tombstones) | In progress: BTreeMap memtable done (12 tests). Background flush + frozen queue next | [MEMTABLE.md](MEMTABLE.md) |
| SSTable | Immutable sorted file on disk: data blocks, index, bloom filter, footer | Designing | [SSTABLE.md](SSTABLE.md) |
| Bloom filter | Per-SSTable bit array that says "definitely not here" to skip disk reads | Designing | [BLOOM_FILTER.md](BLOOM_FILTER.md) |
| Flush | Writes a full memtable to a new SSTable | Designing | [FLUSH.md](FLUSH.md) |
| Compaction | Merges SSTables, keeps the newest version of each key | Not started | [COMPACTION.md](COMPACTION.md) |
| Manifest | List of live SSTables and the current WAL, the source of truth on startup | Designing | [MANIFEST.md](MANIFEST.md) |
| Crash recovery | Rebuilds state on startup from the manifest + WAL replay | Designing | [RECOVERY.md](RECOVERY.md) |
| Benchmarks | Microbenchmarks now, F1 replay and YCSB-style workloads later | In progress: CRC-32 and write + fsync measured on Mac | [BENCHMARKS.md](BENCHMARKS.md) |
| Config | Tuning settings in TOML (`config/*.toml`), format constants stay in code | Done for memtable settings (8 tests) | [CONFIG.md](CONFIG.md) |

<!-- Status ideas: Not started / Designing / In progress / Done / Tested / Benchmarked -->

## Current status

<!-- Where the project is today, and what's next. -->
- Done
    - WAL record format: encode + decode with CRC-32, size limits, torn tail and corruption detection
    - WalWriter: append + fsync every write, failed state after a write or fsync error
    - WAL replay: decodes records in order, truncates a torn tail, reports corruption with the file offset
    - Memtable: BTreeMap of key -> Op, Lookup enum (Found / Deleted / NotFound), running size counter, sorted iter and range
    - Config: TOML tuning settings (memtable flush threshold, max frozen memtables), default + 16/64 MiB benchmark profiles
    - 50 unit tests, CI on Linux and macOS
    - Benchmarks on Mac: crc32fast is 3-4x faster than crc32c, fsync is ~99.9% of a put (~3 ms, ~350 appends/s)
- Next
    - Store: put / get / delete on top of WAL + memtable, replay on open
    - Background flush thread, bounded frozen memtable queue, WAL segments

## Milestones

| Week | Goal | Done? |
|---|---|---|
| 0 | Rust basics, repo, CI, design doc | Done |
| 1 | Memtable + WAL + replay | In progress (WAL + replay + memtable done, store wiring next) |
| 2 | SSTable writer/reader + index | |
| 3 | Read path, tombstones, bloom filters | |
| 4 | Compaction (size-tiered first), atomic file swap | |
| 5 | Manifest, crash tests, property tests | |
| 6 | Benchmarks, compare to RocksDB/sled, README | |

## Full design

See [DESIGN.md](DESIGN.md).
