# Concurrency

**Status:** Built in PR 3b (Nov 13). Every rule below is in `src/store.rs` and tested, including a check that breaking rule 3 makes the deadlock test fail.

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
| flush thread, one memtable | State (oldest frozen `Arc` that isn't waiting out a retry) -> unlock -> Manifest (new file number) -> unlock -> write + open SSTable, no lock -> Manifest (change a copy, save, keep it) -> unlock -> delete segment, no lock -> State (swap in reader, `notify_all`) -> unlock |
| flush fails | State (count the attempt, set its retry time) -> unlock. Nothing ready: condvar `wait_timeout` until the first retry is due, State released while waiting. After max attempts, or any manifest save failure: State (failed = true, `notify_all`) |
| close | State (closing = true, `notify_all`) -> unlock -> join flush thread, no lock |

- In every row the locks go WAL -> Manifest -> State, and no row holds two of them while doing I/O or sleeping

## How it's built

- `Store { inner: Arc<Inner>, flush_thread: Mutex<Option<JoinHandle>> }`. The flush thread gets its own `Arc<Inner>`
- `Inner` holds `wal: Mutex<WalState>`, `manifest: Mutex<Manifest>`, `state: Mutex<State>`, one `Condvar` (`state_changed`), and the stats as atomics
- One condvar for every wait: puts waiting for a slot, the flush thread waiting for work. Every change to the queue, failed, or closing does `notify_all`
- Retries don't move a memtable to the back of the queue. Each slot gets a `retry_at` time and the flush thread takes the oldest slot that's due. Same effect (newer memtables flush meanwhile), but the queue stays in age order, so a get can search it newest first without sorting
- The manifest is changed on a copy and only kept once `save` worked. A failed save never leaves the in-memory manifest ahead of the one on disk
- A failed manifest save is never retried: it's unknown whether the new manifest reached the disk (the rename may have worked and the directory fsync failed). Deleting the SSTable for a retry could delete a file the on-disk manifest lists. Only SSTable write failures are retried
- The SSTable is opened right after it's written (step 1), before the manifest lists it, so a bad file is still a retryable failure

## Tests

In `src/store/tests.rs`. Everything that could hang runs under `within(seconds, ...)`, so a deadlock fails the test instead of freezing CI.

| Test | Proves |
|---|---|
| `store_is_send_and_sync` | `Arc<Store>` can go to other threads (compile time) |
| `flush_runs_in_the_background` | The swapping put returns right away, the frozen memtable is readable until the flush finishes |
| `put_waits_while_the_queue_is_full_and_reads_keep_going` | Backpressure: put waits (and the stall is counted), reads don't wait behind it, the put goes through once the flush frees the slot |
| `waiting_put_wakes_up_when_the_store_fails` | Rule 7: a waiting put returns `Failed` instead of waiting forever |
| `waiting_put_wakes_up_on_close` | Rule 7: a waiting put returns `Closed` |
| `close_finishes_queued_flushes_and_refuses_writes` | close flushes the queue, later puts get `Closed`, close twice is fine |
| `failed_flush_is_retried` / `store_fails_after_max_flush_attempts` | Retries clean up after themselves, the store fails after `max_attempts`, close returns the error |
| `out_of_order_flush_still_reads_the_newest_value` | A retried memtable flushes after a newer one, gets still return the newest value (seq order, not file order) |
| `many_threads_put_and_get_at_once` | 4 writers + 2 readers with constant swaps, stalls, and flushes, every key right after and after reopen |
| `dropping_the_last_arc_stops_the_flush_thread` | Drop runs close, no hang |

- Test hooks (`cfg(test)` only): pause the flush thread before it writes an SSTable, make the next N SSTable writes fail
- Ran the store tests 25 times in a row: no failures, no hangs
- Checked the deadlock test can fail: holding the manifest lock while waiting for queue space (breaking rule 3) deadlocks the store, and `put_waits_while_the_queue_is_full_and_reads_keep_going` fails after 5 s with "put never woke up"

## Open questions

- `RwLock<State>` vs `Mutex<State>`: Mutex first, benchmark with 1 / 2 / 4 / 8 reader threads (see [BENCHMARKS.md](BENCHMARKS.md)). `Condvar` only works with `Mutex`, so the queue wait would need its own `Mutex` + `Condvar`, and the rules above must still hold
- Group commit (week 6): writers waiting on the WAL lock could share one fsync. Changes who holds the WAL lock during the fsync, recheck rules 3 and 8

## See also

[FLUSH.md](FLUSH.md) · [MANIFEST.md](MANIFEST.md) · [STORE.md](STORE.md)
