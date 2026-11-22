# Merge iterator

**Status:** Done in PR 4b (Nov 16): `crates/lsm-kv/src/merge.rs`, 11 tests plus 4 for `SstReader::range`, criterion benchmark. Used by scan (done Nov 22, [SCAN.md](SCAN.md)) and compaction (4c).

## Purpose

- Reads several sorted sources (memtables, SSTables) as one sorted stream
- One entry per key: the one with the highest seq. Older versions are dropped
- Tombstones are yielded too. The caller decides: compaction keeps them above the bottom level, scan skips them

```
memtable (newest)  : a@9  c@12        e@15
SSTable 2          : a@5  b@7   c@8
SSTable 1 (oldest) : b@2        d@3   e@4
merged             : a@9  b@7   c@12  d@3  e@15
```

- Source order doesn't matter, only seqs. A retried memtable flushed after a newer one still loses to the newer one

## Design

| Decision | Choice | Why |
|---|---|---|
| Input type | `Source<'a>` = `Box<dyn Iterator<Item = Result<Record, SstError>> + 'a>` | One type for every source. `SstIter` already is one, memtables go through an adapter |
| Memtable adapter | Copies each entry out as a `Record` | The copy is what `get` does already, benchmarked as unmeasurable next to everything else. A borrowed type would spread lifetimes everywhere for no gain |
| Algorithm | `BinaryHeap` of each source's current entry | O(log k) per entry. Standard, easy to defend. Linear scan would be O(k) |
| Heap order | Smallest key first, same key: highest seq first | The newest version of the smallest key is always on top |
| Tombstones | Yielded | Compaction needs them, scan filters them with one line |
| Visibility | `pub` | Matches memtable / sstable / wal, lets the benchmark use it. Not a security boundary, see below |

- Each step: pop the top (newest version of the smallest key), then pop and skip every other entry with the same key (older versions)
- Sources are refilled at the start of the next call, not right away. A read error then comes after the entries before it, instead of in place of the last good one
    - Safe to wait: a source's next entry always has a bigger key, so it can't be another version of the key just returned

## Data contract

The merge is only right if its inputs follow these. They're checked as it runs, in release builds too, at one key comparison per entry.

| Rule | Broken means | Error |
|---|---|---|
| Each source strictly increasing by key | Unsorted source, or the same key twice | `OutOfOrder { source }` |
| Seqs unique across sources | Two sources claim the same write for a key, which one is real is unknown | `DuplicateSeq { seq }` |

- `DuplicateSeq` is only checked for the same key. A seq reused across different keys isn't caught here
- Foreign data mostly can't get in: `Record` has no public constructor (fields are `pub(crate)`, read-only getters), so code outside the crate can only pass records from our own memtables and SSTables
- `pub` vs `pub(crate)` is a compile-time API rule, not a security boundary. Nothing runs over a network, there's no `unsafe`, so bad input can only give a wrong answer, which these checks turn into an error

## Errors

| `MergeError` | When |
|---|---|
| `Sst(SstError)` | A source failed to read (I/O, corruption) |
| `OutOfOrder { source }` | Source at that index gave a key not greater than its previous one |
| `DuplicateSeq { seq }` | Same key and seq in two sources |

- After any error the iterator yields nothing more. Carrying on would leave keys out of a compaction's output without anyone noticing. A compaction that hits one fails without deleting its inputs

## SstReader::range

- `range(start, end)`: entries with `start <= key < end`, same as `Memtable::range`
- Starts at the first block whose last key is `>= start` (index lookup), so earlier blocks are never read. Skips keys below `start` in that first block only, stops at the first key `>= end`
- `end <= start` is an empty range, not an error
- `iter()` is the same iterator with no bounds

## Interface

- `merge::merge(sources: Vec<Source<'_>>) -> MergeIter<'_>`, an `Iterator<Item = Result<Record, MergeError>>`
- `merge::memtable_source(&Memtable) -> Source<'_>`
- `merge::memtable_range_source(&Memtable, start, end) -> Source<'_>`
- `Box::new(reader.iter())` or `Box::new(reader.range(start, end))` for SSTables
- `Record::seq()`, `Record::key()`, `Record::op()`: read-only getters, new in 4b

## Tests

- `crates/lsm-kv/src/merge/tests.rs` (11)
    - Newest seq wins whatever source it's in, tombstone hides older values and is yielded
    - No sources, empty sources, sources that don't overlap
    - Memtables and real SSTables mixed, ranges of both
    - Model check: 20 rounds of up to 400 random writes over 1-6 sources (about half become SSTables), compared with a `BTreeMap` that applies every write in seq order
    - Errors: a source error is yielded once then stops, unsorted source, same key twice in one source, same key and seq in two sources. Good entries before the bad one still come out
- `crates/lsm-kv/src/sstable/reader/tests.rs` (4 new): range matches a filtered `iter` for every pair of 24 bounds across many blocks, bounds that aren't keys, before / after the file, empty and backwards ranges, a one-key range with a tombstone
- Checked that the tests can fail: making the oldest seq win fails 5 merge tests, making `range` ignore `start` fails 3 range tests

## Benchmark

`crates/lsm-kv/benchmark/merge.rs`, `cargo bench --bench merge`. 4 SSTables (~8 MiB each, 20,800 entries, 16,000 distinct keys, every 10th key in all 4), Linux dev container, short run (3 s per case):

| Case | Time | Throughput |
|---|---|---|
| Read the 4 files one after another | ~8.3 ms | ~4.7 GiB/s |
| Merge the 4 files | ~10.2 ms | ~3.8 GiB/s |

- The merge adds ~20% on top of reading the files. For compaction the cost is in writing the output (a 32 MiB SSTable write + fsync is 50-160 ms), not in merging

## Open questions

- Scan API: decided Nov 22, `Store::scan` returns a `Vec` (built inside the function, so nothing outlives the lock or the borrows). An iterator the caller keeps would need `SstIter` to own an `Arc` of its reader. Only if a caller ever needs streaming. See [SCAN.md](SCAN.md)
- Linear scan vs heap for small k: only worth measuring if the merge ever shows up in a compaction profile

## See also

[COMPACTION.md](COMPACTION.md) · [SSTABLE.md](SSTABLE.md) · [MEMTABLE.md](MEMTABLE.md) · [BENCHMARKS.md](BENCHMARKS.md)
