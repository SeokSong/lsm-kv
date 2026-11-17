# Column chunks

**Status:** Done (Nov 17): `src/chunk.rs`, 15 tests. Roadmap milestone 1, step 1. The format and the reasons are in [DESIGN.md](DESIGN.md#value-format-column-chunks-decided-nov-16), this doc is the code side.

## Purpose

- The value format for telemetry and position data: a run of samples stored as columns
- Self describing: each chunk lists its columns, so any car or source carries its own channels

## Layout (little endian)

```
| header_len u16 | version u16 | start u64 | end u64 | count u16 | column_count u16 |
| column_count x (id u16, type u8) | column 0's rows | column 1's rows | ... |

start(c)    = header_len + count x (w0 + w1 + ... + w(c-1))
sample(c,i) = start(c) + i x w(c)
```

- start / end: session time of the first / last row in microseconds. end is checked against start + the last time offset on decode
- The `time` column (id 1, u32) holds microseconds since start, from 0, never going backwards (equal is fine, OpenF1 repeats timestamps)
- At most 512 rows, fewer when rows are wide (`max_rows`), and never over the 64 KiB value limit

## Design

| Decision | Choice | Why |
|---|---|---|
| Checked type | `Chunk::new` and `Chunk::decode` refuse anything off format | Every `Chunk` encodes to a valid value. Decoded values may come from a friend's export |
| Column values | `ColumnData` enum, one `Vec` per type | Typed access without unsafe casts |
| Unknown column ids | Kept as they are | An older app reading a newer friend's export keeps channels it doesn't know |
| Longer header | Allowed (`header_len` bigger than needed) | A later version can add header fields, older readers skip them |
| Decoding numbers | `as_chunks::<N>()` | Fixed-size arrays, no length check can fail |
| No CRC in the value | The WAL and SSTable CRCs cover values | Same as DESIGN.md |

## Channel catalog

`src/chunk/catalog.rs`. Ids are part of the format: never change or reuse one.

| Id | Column | Type | Unit |
|---|---|---|---|
| 1 | time | u32 | us since chunk start |
| 2 | lap_dist | f32 | m |
| 3 | lap_dist_pct | f32 | 0..1 |
| 4 | speed | f32 | m/s |
| 5 | throttle | f32 | 0..1 |
| 6 | brake | f32 | 0..1 |
| 7 | steering | f32 | rad |
| 8 | gear | i8 | -1 reverse, 0 neutral |
| 9 | rpm | f32 | rpm |
| 10 | drs | u8 | source value |
| 20 | lat | f64 | degrees |
| 21 | lon | f64 | degrees |
| 22 | alt | f32 | m |
| 23-25 | x, y, z | f32 | source units |

## Errors

| `ChunkError` | When |
|---|---|
| `TooShort` | Shorter than the header, or than the rows it describes |
| `UnsupportedVersion(v)` | Not version 1 |
| `BadHeaderLen` | `header_len` smaller than the fixed header + column list |
| `NoColumns`, `MissingTime` | No columns, or no u32 time column |
| `DuplicateColumn(id)` | The same column id twice |
| `UnknownType(byte)` | A type byte that isn't 1-7 |
| `WrongType(id)` | A catalog column stored with another type |
| `BadRowCount(n)` | 0 rows, or more than 512 |
| `LengthMismatch` | Columns of different lengths, or bytes left after the last column |
| `TooLarge(len)` | Over 64 KiB |
| `TimeNotFromZero`, `TimeGoesBackwards` | The time column breaks its rules |
| `EndMismatch` | `end` isn't start + the last time offset |

## Interface

- `Chunk::new(start_us, columns: Vec<Column>) -> Result<Chunk, ChunkError>`
- `chunk.encode() -> Vec<u8>`, `Chunk::decode(&[u8]) -> Result<Chunk, ChunkError>`
- `start_us()`, `end_us()`, `row_count()`, `columns()`, `column(id)`, `time()`, `encoded_len()`
- `max_rows(&[ColumnType]) -> usize`: rows that fit in one chunk
- `Column { id, data: ColumnData }`, `ColumnData::{U8, I8, U16, U32, I32, F32, F64}(Vec<_>)`
- `catalog`: `CATALOG`, `lookup(id)`, constants `TIME`, `SPEED`, `LAT`, ...
- `wal::MAX_VALUE_LEN` is public now, so the chunk code sizes itself against the real limit

## Tests

`src/chunk/tests.rs` (15):
- Round trip: iRacing telemetry and position at 512 rows and 1 row, every column type at its limits
- Sizes match DESIGN.md (24 + 3 x 9 header, 33 bytes a row), layout byte for byte, every column where the `start(c)` / `sample(c,i)` formulas say
- `max_rows` keeps a 40-column f64 chunk under 64 KiB
- Forward compatibility: unknown column ids kept, a longer header skipped
- Every `new` and `decode` error, every truncation is an error, every single bit flip either errors or decodes to a chunk that round trips (never panics)
- Catalog: ids unique, pinned names never change
- Checked that the tests can fail: decoding every column from the same offset fails 5 tests, accepting trailing bytes fails 1

## See also

[DESIGN.md](DESIGN.md) · [IBT.md](IBT.md) · [ROADMAP.md](ROADMAP.md)
