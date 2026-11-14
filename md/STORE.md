# Store

**Status:** PR 3a done: WAL segments, manifest, flush to SSTables, get across every layer, crash recovery at each flush step (29 tests). Flush still runs inline inside `put`; PR 3b moves it to a background thread and makes the Store shareable across threads. Scan comes in week 5.

## Purpose

- The public API that ties the WAL, memtables, SSTables, and manifest together
- Owns the data directory, the manifest, the active WAL segment + memtable, the frozen memtables, and the open SSTables

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

- Why 7 instead of replaying everything into one memtable: every memtable keeps exactly one segment, so a flush always deletes exactly one file. And a normal restart (only the active segment live) writes no SSTable

## Write path (put / delete)

1. Refused with `Failed` if an earlier flush or manifest save failed
2. Build a `Record` with the next seq and check its sizes (`wal::validate`). Bad input is rejected here, before it can trigger a swap
3. If it would push the memtable over `memtable.flush_threshold_bytes`: swap, then flush (see below). An empty memtable always takes the put, even an oversized one
4. `wal.append(&record)`: write + fsync to the active segment
5. Only after the WAL succeeded: the seq is used up, then `memtable.apply(record)`
- Durable when put / delete returns Ok. A rejected write doesn't use a seq

## Swap and flush

- Swap
    1. New file number, add it to the manifest's segments, save the manifest
    2. Create the segment file (fsync the directory)
    3. Active memtable + segment go to the back of the frozen queue, an empty memtable takes over
- The manifest lists the segment before the file exists: a listed segment with no file replays as empty, an unlisted one never holds an acked write
- Flush (the oldest frozen memtable)
    1. Write the SSTable: `.tmp`, fsync, rename, fsync dir
    2. Save the manifest: SSTable in (with seq + key range), segment out, last seq updated
    3. Delete the old segment
    4. Open the SSTable, put it in the read list, drop the frozen memtable
- A failure in the swap or the flush marks the store failed. The frozen memtable stays readable. Retries come in 3b
- What a crash at each step leaves behind: [FLUSH.md](FLUSH.md)

## Read path (get)

Newest first, the first `Found` or `Deleted` wins:

1. Active memtable
2. Frozen memtables, newest first
3. SSTables, newest `max_seq` first
    - Skip a file when the key is outside its first / last key, no bloom filter check needed
    - Otherwise bloom filter, index, one block read

- A tombstone stops the search: the key is deleted even if an older SSTable has a value
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
| `Failed` | A swap or flush failed earlier, writes are refused (reads still work) |

- `open` takes an already parsed `Config`, so config errors never reach the store
- 3b adds `Closed`

## Interface

- `Store::open(dir: &Path, config: Config) -> Result<Store, StoreError>`
- `put(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<(), StoreError>`
- `delete(&mut self, key: Vec<u8>) -> Result<(), StoreError>`
- `get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError>`
- `last_seq(&self) -> u64`: seq of the newest write, 0 before the first one
- `sstable_count(&self) -> usize`, `dir(&self) -> &Path`
- `memtable_full` is gone, the swap check happens inside put
- Writes take `&mut self` until 3b, then `&self` everywhere with `Store: Send + Sync`, see [CONCURRENCY.md](CONCURRENCY.md)
- For tests only (`pub(crate)`): `swap_memtable`, `flush_oldest_until(FlushStep)` to stop a flush after any step, `fail_next_flush` to make the next flush fail

## Tests

- `src/store/tests.rs`: 29 tests, done
- Basics: open creates the manifest + first segment, put / get, missing key, delete, overwrite, rejected put changes nothing and uses no seq
- Recovery: reopen after puts / deletes, after a torn tail, corrupt WAL fails with the offset, seqs continue after reopen, and continue from the manifest when every segment was already flushed
- Swap: happens before the put that would overflow (memtable never over the threshold), empty memtable takes an oversized put, a rejected put doesn't trigger a flush
- Flush: leaves one segment + the SSTables on disk, manifest matches
- Read path: overwrite and delete in a newer layer hide older SSTables, same answers after reopen, frozen memtable readable mid-flush
- Crash at each flush step (before step 1, after 1, 2, 3): checks the files and the manifest at the crash, then after reopen nothing is lost, one SSTable, one segment, no garbage
- Failed flush: the put returns the error, later puts get `Failed`, reads still work, reopen recovers everything and takes writes again
- Open refuses: missing manifest with store files (files untouched), old `wal.log`, SSTable with the wrong size
- Cleanup: unlisted `.sst`, `.wal`, `.tmp` files deleted at open, other files left alone, leftover `MANIFEST.tmp` from a crash during the first open
- Checked that the tests can fail: with the cleanup at open turned off, the crash and cleanup tests fail

## Open questions

- `scan(start, end)` waits for week 5: a merge iterator across memtables and SSTables that skips tombstones and hides older versions
- Seqs aren't checked across segments at open, only inside each one (replay). A segment starting below the previous one's last seq would go unnoticed
- Zero-copy reads: revisit when the triggers in [BENCHMARKS.md](BENCHMARKS.md) show up

## See also

[WAL.md](WAL.md) · [MEMTABLE.md](MEMTABLE.md) · [FLUSH.md](FLUSH.md) · [MANIFEST.md](MANIFEST.md) · [CONCURRENCY.md](CONCURRENCY.md) · [CONFIG.md](CONFIG.md) · [RECOVERY.md](RECOVERY.md)
