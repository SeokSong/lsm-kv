# Scan

**Status:** Designed Nov 21, being written (branch `scan`). ROADMAP milestone 1, step 3.

## What it is

- `Store::scan(start, end)`: every live key in `[start, end)` with its newest value, sorted
- Only the app calls it (`racedata`, later the Tauri backend), with key ranges it builds itself
- The app scans one lap at a time: a lap is one contiguous range (`0x05 | session | driver | lap`), ~0.5-0.7 MB for an iRacing lap, ~20-30 KB for an F1 lap
- Loading a comparison: `get` the session metadata (`0x07`, every lap's time), then scan the fastest lap and lap 1 for both drivers (telemetry `0x05` + position `0x06`). Other laps are scanned when the user picks them

## How it works

```
scan(start, end):
  under the State lock (short, memory only):
    1. copy the active memtable's entries in [start, end)   (it keeps changing, can't be borrowed)
    2. clone the Arcs of the frozen memtables and SSTables   (they never change, safe to read later)
  lock released, then:
    3. one source per memtable / SSTable, each seeking to `start` once
    4. merge(sources): newest seq per key, sorted
    5. skip tombstones, collect (key, value)
```

- Consistent snapshot without seq filtering: steps 1 and 2 happen under one lock. Frozen memtables and SSTables never change. A put after the lock lands in the live memtable, not the copy. A flush or ingest after it installs a new SSTable in the store's list, not in the cloned one
- Not N gets: one seek per source, then reading forward. Each block is read once. Gets would also need the keys in advance (how many chunks a sector has)
- For compaction later: the cloned `Arc<LiveSst>` keeps the file open. Deleting a file a scan still reads works on Linux, fails on Windows

## Decisions

| # | Decision | Choice | Rejected |
|---|---|---|---|
| D1 | What scan returns | `Result<Vec<(Vec<u8>, Vec<u8>)>, StoreError>`, no memory cap. Ranges are lap-sized, the app is the only caller | Callback (awkward for callers). Iterator struct (self-referential: `SstIter` borrows the `SstReader` in an `Arc` it would own). Pages (each page a new snapshot). A cap that errors: cheap to add later if anything bigger than a session is ever scanned |
| D2 | Active memtable | Copy its range into a `Vec<Record>` under the State lock. Memory only, rule 4 holds. Worst case a full 32 MiB memtable, a few ms of puts waiting | Cloning the whole memtable (copies keys outside the range). A concurrent skiplist (rewrites the memtable) |
| D3 | Range | `[start, end)`: start included, end excluded, like `Memtable::range` and `SstReader::range`. `end <= start` is an empty result, not an error | A prefix helper in the store: goes in `racedata` next to the code that builds keys |
| D4 | Errors | New `StoreError::Merge(MergeError)` + `impl From<MergeError>` | Mapping into `Sst` + a new `Corruption` variant (loses which source / seq) |
| D5 | SSTables outside the range | Skipped by key range, no disk read | Reading every file: `SstReader::range` on a file after the range reads one block before it notices, one wasted read per session in the store |

### D5: the overlap check

A file's range is `[first_key, last_key]`, both included (real keys in the file). The scan's is `[start, end)`. A file is skipped when it's entirely before or entirely after the scan:

```
file A: [10 ... 19]    file B: [20 ... 29]    file C: [30 ... 39]
scan:                  [20 ............ 30)
```

| File | Before? `last_key < start` | After? `first_key >= end` | |
|---|---|---|---|
| A | 19 < 20 yes | | skip |
| B | no | no | read |
| C | no | 30 >= 30 yes (key 30 isn't in the scan) | skip |

```
overlaps = last_key >= start && first_key < end
```

## Merge errors

`MergeError` (`src/merge/error.rs`) already has every way a merge can fail. Scan wraps it, no new variants:

| Variant | When | Likely cause |
|---|---|---|
| `Sst(SstError)` | An SSTable source failed to read | Disk I/O error, or corruption: a block's CRC doesn't match, a block won't parse |
| `OutOfOrder { source }` | A source gave a key not greater than its previous one | A bug, or corruption the CRC missed. Never expected |
| `DuplicateSeq { seq }` | Two sources have the same key with the same seq | A bug (seqs are unique per write). Never expected |

Not errors:
- An empty source (an SSTable or memtable with nothing in the range): it just ends right away
- Stopping early: the caller drops the iterator, there's nothing to report
- "The merge failed" on its own: a merge only fails for one of the three reasons above
- After any error the iterator yields nothing more: carrying on would leave keys out without anyone noticing

## Tests

| Test | Proves |
|---|---|
| Empty store, and `end <= start` | Empty result, no panic |
| Keys only in the active memtable | Sorted, `start` included, `end` excluded |
| Keys spread over active, frozen, and an SSTable | Every layer is read, merged in order |
| A key overwritten in a newer layer | The newest value wins |
| A key deleted in a newer layer, old value in an SSTable | Tombstones hide older values and never show up |
| An SSTable with `first_key == end`, one with `last_key == start` | D5's edges: the first is skipped, the second read |
| Scan, put, scan again | The first result doesn't change, the second has the put |
| One lap's sector out of several laps | The real use: only that sector's chunks come back |

## See also

[MERGE.md](MERGE.md) · [STORE.md](STORE.md) · [CONCURRENCY.md](CONCURRENCY.md) · [SSTABLE.md](SSTABLE.md)
