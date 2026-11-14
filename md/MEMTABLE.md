# Memtable

**Status:** Data structure done and tested (14 tests), entries keep their sequence number, settings in config. Background flush and the frozen memtable queue come next.

## Purpose

<!-- What does the memtable hold, and why is it in memory? -->
- Holds the latest state of each recently written key, sorted by key
- By the time a record reaches the memtable, the write has already happened: it's in the WAL, fsynced, and put returned Ok
- It's the newest part of the database, kept in RAM so `get` can find recent writes without touching disk
- When it's full it gets flushed to disk as one SSTable, then the WAL that covered it can be deleted

| | WAL | Memtable |
|---|---|---|
| Holds | Every operation, in arrival order | The latest state of each key, sorted |
| put(k, A), put(k, B), delete(k) | 3 records | 1 entry: k -> tombstone |
| Read by get? | Never (only at startup) | Yes, first place get looks |
| Purpose | Durability | Fast reads of recent writes, sorted batch to flush |

## Data structure

<!-- Which structure, and why? What does it need to support (sorted order? range scans?) -->
- V1: `BTreeMap<Vec<u8>, Op>` from std, hidden behind the `Memtable` API so it can be swapped later
- Needs sorted order (flush writes the SSTable in key order) and range scans (`scan(start, end)`), so no HashMap
- Background flush doesn't need a concurrent structure: the active memtable has one writer, the frozen one is read-only and can be shared with `Arc`
- A concurrent structure is only needed with multiple writers running at the same time. The Store becomes shareable across threads in week 3, but writes still go one at a time through the WAL lock, so BTreeMap stays

| Option | Concurrent writers? | Effort | When |
|---|---|---|---|
| `BTreeMap` (std) | No, needs a lock | 0 | V1 |
| Hand-written skip list (single writer, arena + indices) | No | 1-2 days | Experiment, benchmark vs BTreeMap |
| `crossbeam-skiplist` `SkipMap` | Yes, lock-free | 0 | If concurrent writers are added |

- Skip list: a sorted linked list with random "express lanes". Inserts only change a few nearby pointers, which is why it can be made lock-free (RocksDB, LevelDB use one)
- Expected: a single-threaded skip list is probably slower than BTreeMap (more cache misses). Measure it, don't assume

## Tombstones

<!-- How is "deleted" represented in the memtable? -->
- Each key maps to `Op` (same type as the WAL): `Op::Put { value }` or `Op::Delete`
- delete always writes a tombstone, even if the key isn't in this memtable, since an older SSTable might still have a value for it
- get returns a `Lookup` enum instead of raw values

| Lookup | Meaning | What the read path does |
|---|---|---|
| `Found(&[u8])` | Key has a value here | Return it |
| `Deleted` | Tombstone | Stop and return None, don't check older SSTables |
| `NotFound` | Key isn't in this memtable | Keep going to the frozen memtable, then SSTables |

- Iteration includes tombstones, flush writes them to the SSTable too

## Size and flush trigger

<!-- How is size measured? What size triggers a flush? -->
- Size = key bytes + value bytes. A tombstone counts its key
- Tracked as a running counter updated on every write, never re-measured
    - New key: add key + value bytes
    - Overwrite: subtract the old value, add the new one (BTreeMap::insert returns the old entry, so this is free)
    - Checking "is it full" is one integer comparison
- Matches the bytes the SSTable will hold, so a 32 MB memtable ~ a 32 MB SSTable
- It's an approximation of RAM use, BTreeMap and Vec add per-entry overhead
- Flush threshold: 32 MiB default, set in `config/*.toml` (`memtable.flush_threshold_bytes`) so benchmarks can try 16 / 32 / 64 MiB, see [CONFIG.md](CONFIG.md)

## Active vs. frozen memtable

<!-- What happens to writes while a memtable is being flushed? -->
- Flush is a background job in v1 (one writer thread + one flush thread)
- When the active memtable passes the threshold
    - It becomes frozen (read-only) and goes into a queue
    - A new empty active memtable takes new writes
    - The flush thread writes the frozen memtable to an SSTable
- get checks: active -> frozen (newest first) -> SSTables newest to oldest
- Backpressure: bounded queue of frozen memtables, `memtable.max_frozen` in the config (default 1)
    - If the queue is full when the active memtable fills up, put waits
    - Count and time the stalls, that's the signal to tune the queue or flush speed
    - Unbounded queue is bad practice: if writes outpace the disk, memory grows until the process dies
    - Same idea as LevelDB (one frozen memtable) and RocksDB (`max_write_buffer_number`, default 2 = 1 active + 1 frozen)
- With our numbers stalls should be rare: fsync limits puts to ~350/s x ~2 KB ~ 0.7 MB/s, so 32 MB takes ~45 s to fill
- Background flush also means each memtable needs its own WAL segment, so a segment can be deleted once its memtable is flushed

## Interface

<!-- The functions this module exposes (names and signatures, no bodies). -->
- `Memtable::new() -> Memtable`
- `put(&mut self, seq: u64, key: Vec<u8>, value: Vec<u8>)`
- `delete(&mut self, seq: u64, key: Vec<u8>)`: writes a tombstone
- The store hands out seqs, the memtable just keeps them. Each key stores `(seq, Op)` of its newest write (a debug assert checks a new seq is higher than the last one)
- `apply(&mut self, record: Record)`: takes ownership, used by replay
- `get(&self, key: &[u8]) -> Lookup<'_>`
- `iter(&self) -> impl Iterator<Item = (&[u8], u64, &Op)>`: key order with seq, tombstones included
- `range(&self, start: &[u8], end: &[u8]) -> impl Iterator<Item = (&[u8], u64, &Op)>`
    - start inclusive, end exclusive, same as the lap scan design
    - start > end returns nothing instead of panicking
    - The iterator only borrows the memtable, start and end can be dropped right after the call
- `max_seq(&self) -> Option<u64>`: newest write in this memtable, the read path orders memtables and SSTables by it
- `approximate_size(&self) -> usize`, `len(&self) -> usize`, `is_empty(&self) -> bool`
- The memtable doesn't know about the WAL, files, or flushing. put in the store writes the WAL first, then the memtable

## Tests

- `src/memtable/tests.rs`: 14 tests, done
    - New memtable is empty
    - Put then get returns Found, missing key returns NotFound
    - Overwrite returns the newest value, still one entry
    - Put then delete returns Deleted (not NotFound)
    - Delete of a missing key still writes a tombstone
    - Put after delete returns Found
    - iter is sorted no matter the insert order, and includes tombstones
    - range: lap 12 scan with start inclusive and end exclusive, start > end is empty
    - Size counter on put, overwrite, delete, and a second key
    - Applying replayed records gives the same state as direct writes
    - max_seq is the newest write, not the biggest key
    - Overwrite keeps the newest seq

## Open questions

- Skip list experiment: hand-written single-writer skip list vs BTreeMap, benchmark put and get
- Sequence numbers: added Nov 13. Retried flushes can land out of order, so where a version lives no longer says how new it is. Also needed for compaction and snapshots later

## See also

DESIGN.md, section 3 · [FLUSH.md](FLUSH.md) · [WAL.md](WAL.md)
