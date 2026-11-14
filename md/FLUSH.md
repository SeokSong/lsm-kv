# Flush

**Status:** Designed (Nov 13). Sequence numbers and the manifest are done. PR 3a is done: WAL segments, manifest in the Store, the four flush steps, crash tests at every step, flush runs inline inside put. PR 3b (flush thread, retries, Store shareable across threads) is next.

## Purpose

<!-- What does a flush do? How is it different from compaction? -->
- Writes one frozen memtable to one new SSTable, then lets its WAL segment go
- Runs on a background thread, so puts keep going into a new memtable while it runs
- Compaction is different: it merges existing SSTables into new ones. A flush only ever turns memory into a file

## Trigger

<!-- When does a flush start? -->
- Checked before a put goes in, not after
    - If `size + key + value > memtable.flush_threshold_bytes`, the active memtable is swapped out first and the put goes into the new one
    - A memtable never goes over the threshold, so SSTable sizes stay predictable
    - An empty memtable always takes the put, even one bigger than the threshold (only possible with a tiny test config), otherwise it would swap forever
- The swap
    1. If the frozen queue is full (`memtable.max_frozen`, default 1), wait on the condvar. Count and time the stall
    2. Create a new WAL segment (fsync the directory so it survives a crash)
    3. Save the manifest with the new segment listed. Only after this can a put go into it, so a segment the manifest doesn't list never holds an acked write
    4. Move the active memtable into the frozen queue (`Arc<Memtable>`), start an empty one
    5. Wake the flush thread
- The put then goes to the new segment + new memtable as usual

## Steps

<!-- Numbered, in order. -->
The flush thread takes the oldest frozen memtable and:

1. Writes the SSTable: `<n>.sst.tmp`, fsync, rename to `<n>.sst`, fsync the directory (already built in `SstWriter`)
2. Writes a new manifest: adds the SSTable with its seq range, drops the WAL segment from the live list
3. Deletes the old WAL segment
4. Under the state lock: removes the frozen memtable, adds the open `SstReader`, wakes any put waiting for queue space

- The rule: **never delete a WAL segment until the manifest says its data is in an SSTable**
- Reads switch over in step 4, so the data is always in memory or in a live SSTable, never neither

## Crash at each step

<!-- For each step above: what's on disk if you crash right there, and what does recovery do? -->

| Crash after step | What's on disk | What recovery does |
|---|---|---|
| Before 1 finishes | `.tmp` file, WAL segment | Delete the `.tmp`, replay the segment |
| 1 | Finished SSTable not in the manifest, WAL segment | SSTable is an orphan: delete it, replay the segment. Nothing lost |
| 2 | SSTable in the manifest, old segment still on disk | Manifest doesn't list the segment: delete it, don't replay it |
| 3 | Clean | Nothing to do |

- Replaying a segment whose data is already in an SSTable would also be safe (same seqs, same values), but deleting is cleaner and keeps startup fast

## Writes during a flush

<!-- Where do new puts go while the flush runs? -->
- New puts go to the new active memtable and the new WAL segment
- Gets check, newest first:
    1. active memtable
    2. frozen memtables
    3. SSTables
- Order is by `max_seq`, not by list position or flush order. Stop at the first `Found` or `Deleted`
- Until compaction, every memtable and SSTable covers its own seq range (no overlap), so newest `max_seq` first gives the right answer

## Flush failure and retries

- A failed attempt (disk full, I/O error)
    - Clean up: delete the `.tmp` and any orphan SSTable from that attempt
    - The memtable goes to the back of the frozen queue, so one bad flush doesn't block the ones behind it
    - Short wait before the next attempt so it doesn't spin
- After `flush.max_attempts` failures (default 3): mark the store failed, same as a failed fsync
    - Every later put / delete returns an error
    - Gets keep working, the data is still in memory and in the WAL
    - Restart replays the WAL, nothing acked is lost
- Retries can finish out of order (memtable A fails, newer B flushes first). Safe because reads order by seq, which is why seqs were added before this
- With `max_frozen = 1` the back of the queue is the same memtable, so it's a plain retry

## Threads and locks

- The Store becomes shareable across threads: callers use `Arc<Store>`, every method takes `&self`, `Store: Send + Sync`
- Three locks. Full write-up (the deadlock, the fix, the 10 rules) in [CONCURRENCY.md](CONCURRENCY.md)

| Lock | Protects | Held during |
|---|---|---|
| `Mutex<WalState>` | WAL writer, next seq | The whole put: append, fsync, memtable insert |
| `Mutex<Manifest>` | The manifest (live files, next file number, last seq) | Change + save, a few ms of fsyncs |
| `Mutex<State>` + `Condvar` | Active memtable, `frozen: Vec<Arc<Memtable>>`, `sstables: Vec<Arc<SstReader>>`, failed + closing flags | Short memory-only steps: insert, lookup, swap |

- The manifest gets its own lock because both the writer (swap) and the flush thread save it
    - Under State: every get would stall on its fsyncs
    - Under WAL: deadlock, a put waiting for queue space holds WAL while the flush thread needs it to save the manifest

- Writers hold the WAL lock across both the WAL append and the memtable insert
    - Two puts to the same key land in the WAL and the memtable in the same order
    - Dropping the lock in between could let replay rebuild a different winner than the one readers saw
- Readers never take the WAL lock, so a get never waits behind a 3 ms fsync
- A get locks State, looks in the active memtable and copies the value out, clones the `Arc`s, unlocks. SSTable reads (the slow part) run with no lock held
    - Works because `get` returns an owned `Vec` and `SstReader` reads take `&self`
- A put waiting for queue space holds only the WAL lock, so other writers wait too (they would anyway). Readers keep going
    - It takes the manifest lock only after it wakes up, so the flush thread can always finish and free the slot
- Lock order is always WAL -> Manifest -> State. The flush thread never takes the WAL lock
- Puts from many threads still run one at a time (~350/s on the Mac). Waiting writers make group commit easy later: one fsync for everyone in line

### Mutex first, then benchmark RwLock

- v1 uses `Mutex<State>`: simple, and std's `Condvar` only works with a `Mutex`
- Cost: concurrent gets take turns for the ~0.3 us memtable lookup + copy
- Benchmark against `RwLock<State>` (gets in parallel, backpressure needs another way to wait, e.g. a separate `Mutex` + `Condvar` for the queue)
    - 1, 2, 4, 8 reader threads + 1 writer
    - get p50 / p99 and total gets/s
    - Switch only if the numbers show contention

## Shutdown

- `close(&self) -> Result<(), StoreError>`: stop accepting writes, finish the queued flushes, join the flush thread, return any flush error
- `Drop` does the same, best effort, errors can't be returned from `Drop`
- The active memtable isn't flushed on close: it's in the WAL already, replay brings it back. Keeps shutdown fast
- The flush thread holds an `Arc` of the shared state, not of the `Store`, so dropping the last `Arc<Store>` still stops it
- The `JoinHandle` sits in a `Mutex<Option<_>>` so `close(&self)` can take it out

## File naming

- One counter for WAL segments and SSTables, LevelDB style: `000007.wal`, `000008.sst`, see [MANIFEST.md](MANIFEST.md)
- File numbers are only names. Newer vs older comes from seqs
- The manifest stores the next file number and the last seq, see [MANIFEST.md](MANIFEST.md)

## Interface

- `Store::open(dir: &Path, config: Config) -> Result<Store, StoreError>`
- `put(&self, key: Vec<u8>, value: Vec<u8>) -> Result<(), StoreError>`
- `delete(&self, key: Vec<u8>) -> Result<(), StoreError>`
- `get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError>`
- `close(&self) -> Result<(), StoreError>`
- Stall count and time exposed for benchmarks
- New config: `flush.max_attempts` (default 3)
- `StoreError` becomes its own enum: WAL, SSTable, manifest, `Failed`

## Tests

- Compile-time check that `Store: Send + Sync`
- Swap happens before the put that would overflow, memtable never over the threshold, empty memtable takes an oversized put
- Put blocks while the queue is full, continues once the flush finishes, stall gets counted
- Get finds keys in active, frozen, and SSTables, tombstones in a newer layer hide older values
- Crash at each flush step (run the steps by hand, stop between them), reopen loses nothing and cleans up orphans
- Failed flush retries at the back of the queue, out-of-order flushes still read the newest value, store marked failed after max attempts, gets still work
- Several threads putting and getting at once, every key checked at the end
- close finishes queued flushes, reopen after close replays only the active segment

## Open questions

- Retry wait: fixed or growing (100 ms, 200 ms, 400 ms)?
- Group commit (week 6): who does the fsync for the waiting writers
- `RwLock` vs `Mutex` for State: benchmark above

## See also

[MEMTABLE.md](MEMTABLE.md) · [WAL.md](WAL.md) · [MANIFEST.md](MANIFEST.md) · [RECOVERY.md](RECOVERY.md)
