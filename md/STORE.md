# Store

**Status:** PR 3a + 3b done: WAL segments, manifest, background flush to SSTables with retries and backpressure, get across every layer, crash recovery at each flush step, shareable across threads (`Arc<Store>`), close (39 tests). Scan comes in week 5.

## Purpose

- The public API that ties the WAL, memtables, SSTables, and manifest together
- Owns the data directory, the manifest, the active WAL segment + memtable, the frozen memtables, the open SSTables, and the flush thread
- Shared across threads as `Arc<Store>`: every method takes `&self`, `Store: Send + Sync`. Locks and the rules that keep them deadlock free: [CONCURRENCY.md](CONCURRENCY.md)

## Data directory

| File | What |
|---|---|
| `MANIFEST` | Which files are live, see [MANIFEST.md](MANIFEST.md) |
| `000007.wal` | A WAL segment, one per memtable. Normally only the active one is on disk |
| `000008.sst` | An SSTable, one per flushed memtable |
| `*.tmp` | An unfinished write, deleted at the next open |

- WAL segments and SSTables share one file number counter
- The old single `wal.log` is gone. A directory with a `wal.log` and no manifest fails open with `MissingManifest`, no migration (not released yet)

## Open

1. Create the directory if it doesn't exist, and fsync its parent
2. Load the manifest
    - None and no store files: a new store. Save a manifest listing segment 1, before the file exists
    - None but `.wal` / `.sst` / `wal.log` files: `MissingManifest`. Treating it as new would delete them all
3. Delete every file the manifest doesn't list (`obsolete_files`). Before any new file number is handed out, since a number can be handed out again after a crash
4. Open every listed SSTable: file size must match the manifest (`SstSizeMismatch`), then footer, index, bloom filter
5. Replay every listed WAL segment into its own memtable, oldest first
    - Next seq = the higher of the manifest's last seq and the last replayed seq, plus 1
6. The newest segment stays active: its memtable takes new writes, its file keeps getting appends
7. Older segments (a crash while they waited for a flush) are flushed now, before any write is taken
8. Start the flush thread

- Why 7 instead of replaying everything into one memtable: every memtable keeps exactly one segment, so a flush always deletes exactly one file. And a normal restart (only the active segment live) writes no SSTable

## Write path (put / delete)

1. Take the WAL lock, held for the whole put: puts from many threads run one at a time
2. Build a `Record` with the next seq and check its sizes (`wal::validate`). Bad input is rejected here, before it can trigger a swap
3. Refused with `Failed` (an earlier flush, manifest save, or WAL write failed) or `Closed`
4. If it would push the memtable over `memtable.flush_threshold_bytes`: swap (see below). An empty memtable always takes the put, even an oversized one
    - Queue full (`memtable.max_frozen`): wait for the flush thread to free a slot, holding only the WAL lock. Counted in `stats().stalls`
5. `wal.append(&record)`: write + fsync to the active segment
6. Only after the WAL succeeded: the seq is used up, then the record goes into the active memtable
- Durable when put / delete returns Ok. A rejected write doesn't use a seq

## Swap and flush

- Swap
    1. New file number, add it to the manifest's segments, save the manifest
    2. Create the segment file (fsync the directory)
    3. Active memtable + segment go to the back of the frozen queue, an empty memtable takes over, the flush thread is woken
- The manifest lists the segment before the file exists: a listed segment with no file replays as empty, an unlisted one never holds an acked write
- Flush, on the flush thread (the oldest frozen memtable not waiting out a retry)
    1. Write the SSTable: `.tmp`, fsync, rename, fsync dir
    2. Save the manifest: SSTable in (with seq + key range), segment out, last seq updated
    3. Delete the old segment
    4. Open the SSTable, put it in the read list, drop the frozen memtable
- A failed SSTable write is retried (`flush.max_attempts`, growing wait), a failed manifest save marks the store failed at once. The frozen memtable stays readable either way, see [FLUSH.md](FLUSH.md)
- What a crash at each step leaves behind: [FLUSH.md](FLUSH.md)

## Read path (get)

Newest first, the first `Found` or `Deleted` wins:

1. Active memtable, under the State lock (copy the value out, or clone the frozen + SSTable `Arc`s and unlock)
2. Frozen memtables, newest first, no lock held
3. SSTables, newest `max_seq` first, no lock held
    - Skip a file when the key is outside its first / last key, no bloom filter check needed
    - Otherwise bloom filter, index, one block read

- A tombstone stops the search: the key is deleted even if an older SSTable has a value
- Never waits behind a put's fsync or a manifest save: gets only take the State lock, and only for memory work
- Returns an owned `Vec<u8>`. Benchmarked: the allocation costs nothing measurable, see [BENCHMARKS.md](BENCHMARKS.md)

## Errors

| `StoreError` | When |
|---|---|
| `Wal(WalError)` | Bad input (`Size`), append / fsync failure, WAL corruption at open |
| `Sst(SstError)` | Writing or reading an SSTable |
| `Manifest(ManifestError)` | Loading or saving the manifest |
| `Io` | Creating the directory, listing or deleting files |
| `MissingManifest` | Store files but no `MANIFEST` |
| `SstSizeMismatch { file_number, expected, actual }` | A live SSTable isn't the size the manifest says |
| `Failed` | A flush, manifest save, or WAL write failed earlier, writes are refused (reads still work). Also a poisoned lock |
| `Closed` | `close` was called, writes are refused (reads still work) |

- `open` takes an already parsed `Config`, so config errors never reach the store

## Interface

- `Store::open(dir: &Path, config: Config) -> Result<Store, StoreError>`
- `put(&self, key: Vec<u8>, value: Vec<u8>) -> Result<(), StoreError>`
- `delete(&self, key: Vec<u8>) -> Result<(), StoreError>`
- `get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError>`
- `close(&self) -> Result<(), StoreError>`: stop writes, finish queued flushes, join the flush thread, return the flush error that failed the store. Safe to call twice. `Drop` calls it too (errors dropped)
- `stats(&self) -> StoreStats { stalls, stall_time, flushes, failed_flush_attempts }`
- `last_seq(&self) -> u64`: seq of the newest write, 0 before the first one
- `sstable_count(&self) -> usize`, `dir(&self) -> &Path`
- For tests only: open without a flush thread, swap by hand, run a flush up to any `FlushStep`, pause the flush thread, make the next N flushes fail

## Tests

- `src/store/tests.rs`: 39 tests, done
- Basics: open creates the manifest + first segment, put / get, missing key, delete, overwrite, rejected put changes nothing and uses no seq
- Recovery: reopen after puts / deletes, after a torn tail, corrupt WAL fails with the offset, seqs continue after reopen, and continue from the manifest when every segment was already flushed
- Swap: happens before the put that would overflow (memtable never over the threshold), empty memtable takes an oversized put, a rejected put doesn't trigger a flush
- Flush: leaves one segment + the SSTables on disk, manifest matches
- Read path: overwrite and delete in a newer layer hide older SSTables, same answers after reopen, frozen memtable readable mid-flush
- Crash at each flush step (before step 1, after 1, 2, 3): checks the files and the manifest at the crash, then after reopen nothing is lost, one SSTable, one segment, no garbage
- Failed flush: retried and cleaned up, the store fails after max attempts, later puts get `Failed`, reads still work, close returns the error, reopen recovers everything and takes writes again
- Threads (PR 3b): Send + Sync, flush in the background, backpressure with reads still working, a waiting put wakes on failure and on close, close finishes the queue, out-of-order flushes read the newest value, 4 writers + 2 readers at once, dropping the last `Arc` stops the thread. Table in [CONCURRENCY.md](CONCURRENCY.md)
- Open refuses: missing manifest with store files (files untouched), old `wal.log`, SSTable with the wrong size
- Cleanup: unlisted `.sst`, `.wal`, `.tmp` files deleted at open, other files left alone, leftover `MANIFEST.tmp` from a crash during the first open
- Checked that the tests can fail: with the cleanup at open turned off, the crash and cleanup tests fail. Holding the manifest lock while waiting for queue space deadlocks the store and the backpressure test fails after 5 s

## Open questions

- `scan(start, end)` waits for week 5: a merge iterator across memtables and SSTables that skips tombstones and hides older versions
- Seqs aren't checked across segments at open, only inside each one (replay). A segment starting below the previous one's last seq would go unnoticed
- Zero-copy reads: revisit when the triggers in [BENCHMARKS.md](BENCHMARKS.md) show up
- A get clones the list of SSTable `Arc`s every time. Fine with a handful of files, compaction keeps it short. Measure with the `Mutex` vs `RwLock` benchmark

## See also

[WAL.md](WAL.md) · [MEMTABLE.md](MEMTABLE.md) · [FLUSH.md](FLUSH.md) · [MANIFEST.md](MANIFEST.md) · [CONCURRENCY.md](CONCURRENCY.md) · [CONFIG.md](CONFIG.md) · [RECOVERY.md](RECOVERY.md)
