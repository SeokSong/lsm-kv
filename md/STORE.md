# Store

**Status:** In progress. open / put / delete / get on top of WAL + memtable are done and tested (12 tests), every write gets a sequence number. Flush, SSTables, and scan come later.

## Purpose

- The public API that ties the WAL and the memtable together
- Owns the data directory, the WAL writer, the active memtable, and the config

## Data directory

| File | What |
|---|---|
| `wal.log` | The WAL (one file for now, segments come with background flush) |
| SSTables, MANIFEST | Later |

## Open

1. Create the directory if it doesn't exist, and fsync its parent so the new directory survives a crash
2. Replay `wal.log` into a new memtable, next seq = last replayed seq + 1 (1 for an empty WAL)
3. Open the WAL writer, only after replay so a torn tail is cut off before new appends
- A corrupt WAL makes open fail with `Corruption { offset, reason }`

## Write path (put / delete)

1. Build a `Record` with the next seq (`Op::Put { value }` or `Op::Delete`)
2. `wal.append(&record)`: validates, writes, fsyncs
3. Only after the WAL succeeded: the seq is used up, then `memtable.apply(record)`
- A rejected write (bad input) doesn't use a seq, so there are no gaps
- If the append fails (bad input, write or fsync error) the memtable is untouched
- Durable when put / delete returns Ok
- delete always writes a tombstone

## Read path (get)

- Memtable `Found` -> `Some(value)`, `Deleted` or `NotFound` -> `None`
- Later: `NotFound` falls through to the frozen memtables, then SSTables newest to oldest
- Returns an owned `Vec<u8>` (one copy per read). Benchmarked: the allocation costs nothing measurable, the copy is ~0.2 us of reading 2 KB from RAM, see [BENCHMARKS.md](BENCHMARKS.md)
- Returns `Result` because reads will hit disk once SSTables exist

## Errors

- `StoreError` is `WalError` for now (`pub type StoreError = WalError`), every store error is currently a WAL error
- `open` takes an already parsed `Config`, so config errors never reach the store
- Becomes its own enum once SSTable and manifest errors exist, the Store signatures stay the same

## Interface

- `Store::open(dir: &Path, config: Config) -> Result<Store, StoreError>`
- `put(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<(), StoreError>`
- `delete(&mut self, key: Vec<u8>) -> Result<(), StoreError>`
- `get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError>`
- `memtable_full(&self) -> bool`: memtable size >= `memtable.flush_threshold_bytes`, used by flush later
- `last_seq(&self) -> u64`: seq of the newest write, 0 before the first one
- `dir(&self) -> &Path`
- Writes take `&mut self` for now. Week 3 makes the Store shareable across threads (`&self` everywhere, `Send + Sync`), see [FLUSH.md](FLUSH.md)

## Tests

- `src/store/tests.rs`: 12 tests, done
    - Open creates the directory and the WAL
    - Put then get, missing key, delete, overwrite
    - Reopen recovers puts, overwrites, and deletes from the WAL
    - Reopen after a torn tail: acked writes survive, the torn one is gone, new writes after recovery are durable
    - Reopen with a corrupt WAL fails with the offset and `CrcMismatch`
    - Rejected put (key too long) changes nothing and the store keeps working
    - `memtable_full` follows the config threshold
    - Seqs count up from 1 for puts and deletes, and continue after a reopen

## Open questions

- `scan(start, end)` waits for week 5: it needs a merge iterator across memtable, frozen memtables, and SSTables that skips tombstones and hides older versions. A memtable-only scan now would be thrown away
- Zero-copy reads: revisit when the triggers in [BENCHMARKS.md](BENCHMARKS.md) show up

## See also

[WAL.md](WAL.md) · [MEMTABLE.md](MEMTABLE.md) · [CONFIG.md](CONFIG.md) · [RECOVERY.md](RECOVERY.md)
