# Compaction

**Status:** Designed (Nov 16). Merge iterator done (PR 4b, [MERGE.md](MERGE.md)). PRs: 4c compaction core (levels, merge, output splitting, called by hand in tests), 4d compaction thread, 4e bulk load.

## Purpose

<!-- What problems does compaction solve? What happens if you never compact? -->

- The workload is append-mostly: telemetry is written once, no re-ingests of a session, deletes are whole sessions. Little garbage to reclaim
- The real cost without compaction is read amplification: every flush adds an SSTable, and a get or scan checks more and more files
- Also reclaims space from the few overwrites (an ingest restarted after a crash rewrites the same keys) and from deleted sessions

## The key-order problem

- Data arrives in time order (every car's sector 1, then sector 2, telemetry and position mixed), but keys sort as `namespace | session | car | lap | sector | seq`
- A memtable filled during sessions N .. N+2 covers `01 | N ...` to `02 | N+2 ...`. That range contains every older session's position data (`02 | older`)
- With plain first / last key ranges every new SSTable overlaps all old position data, and a range-based compaction rewrites it every time: write amplification grows with the database
- Fix: split output files at prefix boundaries (RocksDB's `SstPartitioner` does the same)

| Namespace | Split output at |
|---|---|
| `0x01` telemetry, `0x02` position | Namespace and session change, plus the 32 MiB size target |
| `0x03` session metadata, `0x04` driver map | Namespace change only. Tiny (a few KB), rewriting them is cheap |
| Sim data (reach goal, new namespaces) | Same as `0x01` / `0x02` |

- A file holding one session only overlaps files of the same session, whatever order sessions arrive in. Session ids don't need to increase (they don't: 2023 Bahrain 7953, Jeddah 7779)
- Most compactions become trivial moves: the file moves down a level in the manifest, nothing is rewritten

| | Namespace only | Namespace + session (chosen) |
|---|---|---|
| New files overlap | Old files whenever session ids arrive out of order | Only files of the same session |
| Compaction rewrites | Data of the sessions in between, often | Almost never |
| Get / scan of one session reads | Files holding other sessions too | That session's files only |
| Delete a whole session | Tombstone per key | Can drop whole files later |
| Files | Fewer, bigger | ~2 per session per flush, plus 2 small ones |

## Strategy

<!-- Size-tiered first? What triggers a compaction? Which SSTables get picked? -->

- Leveled, LevelDB style
    - L0: flushed files, may overlap, read newest `max_seq` first
    - L1 and deeper: files never overlap inside a level, each level 10x the one above (L1 = 10 x 32 MiB)
    - A get checks the L0 files whose range holds the key, then at most one file per level
- Rejected: size-tiered. Less writing, but every file overlaps, so a scan of one driver's race reads every file. Scans are the main query
- Data volume is small (~13 MB per race, ~300 MB per season), so leveled's extra writing is cheap

| Trigger (config) | Default |
|---|---|
| L0 -> L1 | L0 has 4+ files |
| L(n) -> L(n+1) | Level over its size target |
| Stall puts on too many L0 files | Not in v1, add when a benchmark shows it's needed (LevelDB: 12) |

- Picking inputs
    - L0 -> L1: an L0 file plus every older L0 file that overlaps it, plus the L1 files they overlap
    - Never an L0 file newer than the oldest frozen memtable still waiting to flush (rule 13, [CONCURRENCY.md](CONCURRENCY.md)). A retried flush could otherwise put older data in L0 after newer data moved to L1
    - L(n) -> L(n+1): one file plus the files it overlaps in the next level
    - No overlap in the next level: trivial move

## Merge

<!-- How do you merge several sorted SSTables? Which version of a key wins? -->

- K-way merge of sorted inputs (memtables and SSTables), the merge iterator from PR 4b, see [MERGE.md](MERGE.md). Also what `scan` uses in week 5
- Same key in several inputs: the highest seq wins, older versions are dropped (no snapshots, nobody can read them)

## Tombstones

<!-- When is it safe to drop a tombstone, and why? -->

- Dropped only when compacting into the bottom level that holds its key range: no older value can sit below it
- Anywhere else it stays, or an older value in a lower level would come back

## Swapping in the result

<!-- How do the new SSTable(s) replace the old ones safely? What if you crash midway? -->

1. Write the outputs (tmp, fsync, rename, fsync dir) and open them, no lock
2. Change a copy of the manifest: outputs in (with level), inputs out. Save, keep the copy only if the save worked
3. Swap the SSTable lists under the State lock
4. Delete the inputs. A get still reading one holds its `Arc`, an unlinked open file stays readable

- Crash before step 2: the outputs aren't listed, open deletes them, the inputs are still live
- Crash after step 2: the inputs aren't listed anymore, open deletes them
- Manifest format version 2: each SSTable gets a level. Version 1 files load as everything in L0
- Locks and threads: [CONCURRENCY.md](CONCURRENCY.md), own compaction thread, never takes the WAL lock, nobody waits for it

## Bulk load (PR 4e)

- Historical sessions are complete before ingest, so they can skip the WAL, memtable, L0, and compaction (RocksDB: `SstFileWriter` + `IngestExternalFile`)
    1. Sort the session outside the store (~13 MB per race, fits in memory)
    2. Write SSTables with `SstWriter`, split by namespace + session like flush
    3. `store.ingest(files)`: reserve a seq range, check nothing existing overlaps, list the files at the bottom level, save the manifest
- Overlap with existing data: refused, the caller uses puts. Historical sessions are ingested once, so this shouldn't happen
- Live data keeps the normal put path

## Write amplification

<!-- How will you measure it? -->

- Stats: bytes flushed, bytes written by compaction, compactions, trivial moves
- Write amplification = (WAL bytes + flushed bytes + compaction bytes) / user bytes
- Benchmark (PR 4e): the 2023 Belgian GP race from OpenF1 (session_key 9141), ingested with puts vs bulk load

| Measure | Puts | Bulk load |
|---|---|---|
| Total time | | |
| fsyncs | | |
| Bytes written / write amplification | | |
| Files per level after | | |
| Same answers for every key | | |

- Data: a script in `f1-data` exports the race from OpenF1 (free, no login) into key / value records. The file stays local, F1 data is never committed

## Interface

- To be decided in PR 4c. Likely `Store::compact_now` for tests, `CompactionConfig` (L0 trigger, level ratio, target file size), `StoreStats` gains the write amplification counters, `Store::ingest(paths)` in 4e

## Tests

- Planned
    - Merge: newest seq wins, tombstones hide older values, dropped only at the bottom level
    - Output splitting at namespace and session boundaries
    - Trivial move when nothing overlaps
    - Rule 13: a retried older memtable flushed after a newer one was compacted still reads the newest value
    - Crash before / after the manifest save, nothing lost, no garbage
    - Gets during compaction read the old files until the swap, the new ones after

## Open questions

- Whole-session delete without tombstones: drop the session's files from the manifest (RocksDB `DeleteFilesInRange`). After compaction works
- L0 stall threshold: only if benchmarks show L0 piling up
- Sim data namespaces and how their session ids are made (reach goal, DESIGN.md)

## See also

[SSTABLE.md](SSTABLE.md) · [MANIFEST.md](MANIFEST.md) · [RECOVERY.md](RECOVERY.md) · [CONCURRENCY.md](CONCURRENCY.md)
