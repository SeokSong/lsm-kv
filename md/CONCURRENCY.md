# Concurrency

**Status:** Designed (Nov 13). Built in PR 3b (background flush + Store shareable across threads). PR 3a has no threads.

## Threads

- Caller threads: any number, sharing one `Arc<Store>`. Each one calls `put` / `delete` / `get`
- One flush thread, started by `Store::open`, stopped by `close`
- Every `Store` method takes `&self`, `Store: Send + Sync`

## Shared state and locks

| Lock | What's inside | Held during | Taken by |
|---|---|---|---|
| `Mutex<WalState>` | WAL writer (active segment), next seq | A whole put: append, fsync, memtable insert | Writers only |
| `Mutex<Manifest>` | The manifest (live files, next file number, last seq) | Change + `save` (file I/O, a few ms) | Writer at a swap, flush thread |
| `Mutex<State>` + `Condvar` | Active memtable, `frozen: Vec<Arc<Memtable>>`, `sstables: Vec<Arc<SstReader>>`, failed flag, closing flag | Short memory-only steps: insert, lookup, swap | Writers, readers, flush thread |

- Stats (stall count / time, flushes, failed flushes) are atomics, no lock to read them
- The `JoinHandle` of the flush thread sits in a `Mutex<Option<_>>` that only `close` touches

## The problem: where does the manifest go?

Two threads save the manifest:
- The writer at a swap: the new WAL segment must be listed before any put goes into it
- The flush thread after step 1: add the SSTable, drop the old segment

A save is a couple of fsyncs, so whatever lock covers it is held for milliseconds.

| Put the manifest in | What breaks |
|---|---|
| `State` | Every get takes State. Gets would stall on the manifest's fsyncs |
| `WalState` (keeps 2 locks) | Deadlock with backpressure, see below |
| Its own `Mutex<Manifest>` | Works, if the writer waits for queue space before taking it |

### The deadlock

The queue is full (`max_frozen = 1`), the flush thread is writing the frozen memtable to an SSTable, and a put needs to swap.

If the writer takes the manifest lock (or the manifest lives under the WAL lock) **before** it waits:

```
writer                                   flush thread
------                                   ------------
lock WAL
lock Manifest       <- too early
queue full -> wait for a free slot       ...writing SSTable (step 1)...
   (sleeping, still holds Manifest)      step 2: lock Manifest -> BLOCKED
   waiting for flush thread              waiting for writer
```

- The writer waits for the flush thread to free a slot
- The flush thread waits for the writer to release the manifest lock
- Neither moves. The store hangs forever, no error, no crash

### The fix: wait first, then take the manifest lock

```
writer                                   flush thread
------                                   ------------
lock WAL
lock State: queue full -> condvar wait   ...writing SSTable (step 1)...
   (sleeping, holds only WAL,            step 2: lock Manifest (free) -> save -> unlock
    the wait released State)             step 3: delete old WAL segment
                                         step 4: lock State -> remove frozen memtable,
                                                 add SstReader -> notify_all -> unlock
wakes up holding State, slot is free
unlock State
lock Manifest -> new segment listed, save -> unlock
lock State -> active memtable into frozen queue -> unlock
append to WAL (fsync), lock State -> insert -> unlock
unlock WAL
```

- While the writer sleeps it holds only the WAL lock, and the flush thread never takes the WAL lock
- So the flush thread can always finish steps 2-4, free the slot, and wake the writer
- `Condvar::wait` releases the State lock while sleeping and takes it back on wake, so readers and the flush thread keep using State
- Holding the WAL lock while asleep is on purpose: other writers queue behind it (they'd wait for a slot anyway), and puts stay in the same order in the WAL and the memtable

### Why this is enough

A deadlock needs all four of these at once (Coffman conditions). Breaking any one prevents it:

| Condition | Here |
|---|---|
| Mutual exclusion | Kept: these are mutexes |
| Hold and wait | Broken for the flush thread's locks: a writer only holds the WAL lock while it waits, and nobody waiting for the writer needs it |
| No preemption | Kept: a lock can't be taken away |
| Circular wait | Broken: every thread takes locks in the same order, WAL -> Manifest -> State |

## The rules

Every change to the Store's threading must follow these. A review that breaks one is a bug.

1. **Lock order is WAL -> Manifest -> State.** Skipping one is fine (a reader takes only State). Taking them backwards is never fine, e.g. never lock Manifest while holding State
2. **The flush thread never takes the WAL lock.** That's what makes rule 3 safe
3. **Never wait for the flush thread while holding Manifest or State.** The only lock allowed while waiting for queue space is WAL. The wait is `Condvar::wait` on State, which releases State while sleeping
4. **No file I/O under the State lock.** Memory only: insert, lookup, clone `Arc`s, swap lists. fsyncs, SSTable reads, manifest saves, deletes all happen outside it
5. **Readers never take WAL or Manifest.** A get locks State, looks in the active memtable, copies the value out, clones the frozen and SSTable `Arc`s, unlocks, then searches frozen memtables and SSTables with no lock held
6. **Wait in a loop.** `while queue_full && !failed && !closing { wait }`. A condvar can wake up with nothing changed (spurious wakeup), and another writer may have taken the slot first
7. **Every waiter has a way out.** When the store is marked failed or starts closing, `notify_all`. A waiting put wakes, sees the flag, and returns `Failed` / `Closed` instead of waiting forever for a flush that will never come
8. **Hold the WAL lock across append and memtable insert.** Not about deadlock, about correctness: two puts to the same key land in the WAL and the memtable in the same order, so replay rebuilds the winner readers saw
9. **Join the flush thread with no lock held.** `close` sets the closing flag under State, `notify_all`, unlocks, then joins. Joining while holding State would block the flush thread's step 4 and hang `close`
10. **A poisoned lock means the store is failed.** If a thread panics while holding a lock, the next `lock()` returns a poison error. Treat it as `Failed` instead of `unwrap()` panicking every other thread

## Lock sequence per operation

| Operation | Sequence |
|---|---|
| get | State (lookup active, clone `Arc`s) -> unlock -> search frozen + SSTables, no lock |
| put, no swap | WAL -> append + fsync -> State (insert) -> unlock State -> unlock WAL |
| put, swap | WAL -> State (wait while full, rule 6) -> unlock State -> Manifest (new segment, save) -> unlock -> State (swap) -> unlock -> append + fsync -> State (insert) -> unlock -> unlock WAL |
| flush thread, one memtable | State (take oldest frozen `Arc`) -> unlock -> write SSTable, no lock -> Manifest (save) -> unlock -> delete segment, no lock -> State (swap in reader, `notify_all`) -> unlock |
| flush fails | State (move memtable to the back, count the attempt) -> unlock -> sleep with no lock. After max attempts: State (failed = true, `notify_all`) |
| close | State (closing = true, `notify_all`) -> unlock -> join flush thread, no lock |

- In every row the locks go WAL -> Manifest -> State, and no row holds two of them while doing I/O or sleeping

## Tests (PR 3b)

- Deadlock test: `max_frozen = 1`, a test hook makes the flush slow, several threads put until the queue is full. Must finish within a timeout
- Failed flush while a put is waiting: the put returns `Failed`, doesn't hang
- `close` while a put is waiting: the put returns `Closed`
- Reads keep working while a writer is stuck waiting for queue space
- Many threads putting and getting at once, every key checked at the end
- Every test that could hang runs under a timeout, so a deadlock fails the test instead of freezing CI

## Open questions

- `RwLock<State>` vs `Mutex<State>`: Mutex first, benchmark with 1 / 2 / 4 / 8 reader threads (see [BENCHMARKS.md](BENCHMARKS.md)). `Condvar` only works with `Mutex`, so the queue wait would need its own `Mutex` + `Condvar`, and the rules above must still hold
- Group commit (week 6): writers waiting on the WAL lock could share one fsync. Changes who holds the WAL lock during the fsync, recheck rules 3 and 8

## See also

[FLUSH.md](FLUSH.md) · [MANIFEST.md](MANIFEST.md) · [STORE.md](STORE.md)
