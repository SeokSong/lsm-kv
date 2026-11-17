# Bulk load

**Status:** Done in PR 4e (Nov 16): `src/store/ingest.rs`, 19 tests, criterion benchmark. Built before compaction (4c): ingested files join the normal SSTable list for now, and go to the bottom level once levels exist.

## Purpose

- Load a session that's complete before it's written, straight into SSTables. Skips the WAL, the memtable, and (later) compaction
- What it's for: an iRacing session from a `.ibt` file, a friend's export, a historical F1 race
- Live data keeps using `put`

```rust
let mut ingest = store.begin_ingest()?;
for (key, value) in session {        // keys strictly increasing
    ingest.put(key, value)?;
}
let info = ingest.commit()?;         // IngestInfo { seq, files, entries, bytes }
```

## Design

| Decision | Choice | Why |
|---|---|---|
| Who writes the files | The store (`begin_ingest` / `put` / `commit`) | The store controls seqs, file numbers, and names. Every entry is checked on the way in, which imports from other PCs need anyway. Rejected: callers write files and the store overrides seqs (RocksDB's global seqno). Only pays off for huge files built elsewhere |
| Seqs | One seq for the whole ingest, reserved at `begin_ingest` | Keys are unique inside an ingest and nothing existing overlaps it, so one seq is enough. The import counts as one write |
| Atomicity | `commit` lists every file in one manifest save | Never half a session: a crash before the save leaves no trace, after it the whole session is there |
| Overlaps | Refused at commit: active memtable, frozen memtables, SSTables | Importing the same session twice fails with `Overlaps` instead of mixing two copies |
| File splits | New file when the first `split_prefix_len` key bytes change (5 = namespace + session), or at `target_file_bytes` (32 MiB) | Each session gets its own files, ready to export. The compaction design's split rule |
| Puts only | No tombstones | Nothing existing can overlap an ingest, so a tombstone would never hide anything |

## Steps

1. `begin_ingest`: WAL lock (briefly), refuse if the store failed or is closing, reserve seq `S` (`next_seq += 1`)
2. `put`: check sizes (same limits as a WAL record: 16 B key, 64 KiB value), check the key is greater than the last one, start a new file at a prefix change or the size target, write the entry with seq `S`. No lock, except the manifest lock for a moment to get a new file number
3. `commit`
    1. Finish the last file (tmp, fsync, rename, fsync dir), open every file and check its size. No lock
    2. Take the WAL lock and keep it to the end: no put can land in the ingest's key range between the check and the install
    3. State lock: refuse if failed / closing, refuse if anything overlaps any file's key range
    4. Manifest lock: change a copy (files in, `last_seq` at least `S`), save, keep it
    5. State lock: insert the files into the read list (newest `max_seq` first), `notify_all`
4. Release the WAL lock

- Puts keep going during steps 1-3.1. Only commit holds them up, for one manifest save (a few ms)
- Lock order WAL -> Manifest -> State, no waiting on the flush thread while holding a lock: the rules in [CONCURRENCY.md](CONCURRENCY.md) hold

## Failures

| What happens | Result |
|---|---|
| A bad entry (out of order, duplicate, too big) | `put` returns the error, everything written so far is deleted, every later call returns `IngestError::Aborted` |
| `commit` with nothing added | `IngestError::Empty` |
| Overlap at commit | `IngestError::Overlaps`, the files are deleted |
| Store failed or closed | `StoreError::Failed` / `Closed`, the files are deleted |
| The ingest is dropped without commit | The files are deleted |
| Crash before the manifest save | The files are unlisted, the next open deletes them (`obsolete_files`). Seq `S` was never saved, so it's handed out again, which is fine |
| Manifest save fails | Like a failed flush save: the new manifest may already be on disk, so the files are kept and the store is marked failed. A reopen sees whichever manifest is on disk |

## Errors

| `IngestError` (in `StoreError::Ingest`) | When |
|---|---|
| `OutOfOrder` | A key isn't greater than the previous one (that includes the same key twice) |
| `Empty` | `commit` with nothing added |
| `Overlaps` | Existing data inside the ingest's key range |
| `Aborted` | An earlier `put` on this ingest failed |

- Too-big keys or values come back as `StoreError::Wal(WalError::Size)`, the same error a put gives

## Config

| Setting | Default | |
|---|---|---|
| `sstable.target_file_bytes` | 32 MiB | New file once one reaches this many key + value bytes |
| `sstable.split_prefix_len` | 5 | New file when the first N key bytes change. 0 turns it off |

- The store knows nothing about F1 keys: the split is by prefix length only

## Tests

`src/store/ingest/tests.rs` (19):
- Basics: every key readable, nothing went through the WAL or memtable, one seq for the whole ingest (puts before and after get the seqs around it), survives a reopen and seqs continue
- Splits: one file per namespace + session, at the size target (files never overlap), `split_prefix_len = 0` keeps one file
- Overlaps refused: active memtable, frozen memtable, the same session twice, two ingests of the same range at once. Data right next to the range isn't an overlap
- Refused input: empty, out of order (after several files were written: all deleted), duplicate key, too-big key, too-big value
- Cleanup: dropping without commit leaves nothing, a crash before commit (`mem::forget`) leaves no trace after reopen and the session can be ingested again
- Concurrency: puts with swaps and flushes keep going during an ingest, a failed or closed store refuses
- Checked that the tests can fail: turning the overlap check off fails 4 tests, turning prefix splits off fails 1. 30 repeated runs of the store tests: no failures

## Benchmark

`benchmark/ingest.rs`, `cargo bench --bench ingest`. Fake data shaped like the 2023 Belgian GP: 44 laps x 3 sectors x 2 streams per car, 2,000-byte chunks. Linux dev container, short run (5 s per case):

| Case | Entries | Time | Per entry |
|---|---|---|---|
| Puts, one car's race | 264 (0.5 MB) | ~80 ms | ~300 us (one fsync each) |
| Bulk load, one car's race | 264 | ~3.2 ms | ~12 us |
| Bulk load, full race (20 cars) | 5,280 (10.6 MB) | ~15 ms | ~3 us |

- ~25x faster for one car in the container. The container's fsync is much faster than real disks (see [BENCHMARKS.md](BENCHMARKS.md)), so the real gap is bigger: at the Mac's ~3 ms per fsync, puts for a full race would take ~16 s, bulk load stays at a few fsyncs
- Bytes written: puts write every chunk twice (WAL, then the flushed SSTable), bulk load once
- Real numbers: rerun on Linux hardware (the `Bench` GitHub Actions workflow with `bench = ingest`), and with real 2023 Spa data once the value encoding exists

## Open questions

- Export (next PR): write one session's data to a file a friend can import. With session-split files that's close to copying the files
- With levels (4c): ingested files go to the bottom level, so compaction never rewrites them
- Ingest of a session that's partly live already (same session started with puts): refused today. Fine for imports, revisit if live collection needs it

## See also

[STORE.md](STORE.md) · [COMPACTION.md](COMPACTION.md) · [CONCURRENCY.md](CONCURRENCY.md) · [PRODUCT.md](PRODUCT.md)
