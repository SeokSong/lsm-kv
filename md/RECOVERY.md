# Crash Recovery

**Status:** In progress. WAL replay is done (torn tail truncation, corruption with offset). Manifest load and finding obsolete files are done, the Store uses them in the flush PR.

## Guarantee

<!-- What exactly does the store promise after a crash? -->
- Any put or delete that returned Ok is on disk and comes back after a crash
- A write that never returned Ok (crash before fsync finished) may or may not come back, we never told the caller it succeeded

## Startup steps

<!-- Numbered, in order: manifest, stray files, WAL replay, ... -->
1. Read the manifest (`Manifest::load`, done). None = new store, but only if there are no `.sst` files either
2. Delete everything `obsolete_files` returns: unlisted SSTables and WAL segments, `.tmp` files (done, wired in the flush PR, crash table in [FLUSH.md](FLUSH.md)). Must run before any new file is created, since file numbers can be handed out again after a crash
3. Replay the WAL into an empty memtable (`replay(path)`, done)
    - Missing WAL: nothing to replay
    - Decode records one at a time
    - Torn tail: stop, truncate the file at the end of the last valid record
    - Corruption in the middle: stop and return a corruption error with the file offset, file left untouched
    - Seqs must strictly increase, a lower or repeated one is `SeqOutOfOrder` corruption
    - Next seq = last replayed seq + 1 (later: the higher of that and the manifest's last seq)
4. Open the WAL for new writes (`WalWriter::open(path)`), only after replay so new records don't land after a torn tail
- Steps 3 and 4 are `Store::open`, done and tested (reopen after puts / deletes, after a torn tail, with a corrupt WAL)

## Failure scenarios

| Scenario | What's on disk | What recovery does | Data lost? |
|---|---|---|---|
| Crash in the middle of an append | Last record is partly written | decode returns `Ok(None)`, replay truncates it | Only the write that never returned Ok |
| Crash after write, before fsync | Record may or may not be on disk | Replays it if complete, truncates it if torn | Only the write that never returned Ok |
| Bit flip in the middle of the WAL | All bytes there, CRC fails | Stop with `Corruption` (v1) | Records after it are not replayed |
| Corrupted length under the limit at the end | Looks like a torn tail | Treated as a torn tail (known limitation) | The last record |
| fsync or write fails while running | Unknown | Writer marks itself failed, every later append returns `Failed` | Nothing acked is lost, new writes are refused |
| Valid record with a seq that went backwards | All bytes there, CRC passes | Stop with `Corruption { reason: SeqOutOfOrder }` | Records after it are not replayed |
| New WAL file created then crash | File's directory entry may be missing | Parent directory is fsynced on create, so the file survives | None |

## Why replay is safe

<!-- Idempotency: why is replaying an already-flushed record harmless? -->
- Puts and deletes are idempotent: replaying a record that was already flushed writes the same value again, with the same seq
- decode checks the CRC before trusting the type byte, so a corrupted record is never applied

## Tests

<!-- How will you simulate crashes? Truncation, kill -9, fault injection? -->
- Done: decode at every truncation point returns `Ok(None)` and never panics
- Done: bit flips return `CrcMismatch`, failed writer refuses appends
- Done: replay on real files, cut at every byte offset (records and file length checked), append after a torn tail, corruption offset
- Later: kill -9 during a write loop, then reopen and check every acked write

## Open questions

- If the last record has all its bytes but fails the CRC, is it a torn tail or corruption?
- Should v2 have a recovery mode setting (like RocksDB's WALRecoveryMode)?

## See also

[WAL.md](WAL.md) · [FLUSH.md](FLUSH.md) · [MANIFEST.md](MANIFEST.md)
