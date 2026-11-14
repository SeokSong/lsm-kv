# Manifest

**Status:** Done and tested (29 tests): format, save / load, file naming, finding obsolete files. The Store starts using it with WAL segments in the flush PR.

## Purpose

<!-- What question does the manifest answer? Why can't you just list the directory? -->
- Answers "which files are live?" on startup: which SSTables count, which WAL segments still need replaying
- Listing the directory breaks with compaction: it swaps several SSTables for one, a crash halfway leaves old and new files that all look live
- Also tells startup which files are orphans (a finished SSTable from a flush that crashed before the manifest update)
- The rule at startup: **anything in the directory the manifest doesn't list is garbage**

## Contents

<!-- Every field: live SSTables, WAL segment number, anything else? -->
- Live SSTables: file number, file size, min / max seq, first / last key
    - File size: checked against the real file at open, cheap catch for a truncated SSTable
    - Seq range: reads go newest `max_seq` first
    - Key range: a get can skip a file that can't hold the key before checking its bloom filter
- Live WAL segments, oldest first, by file number. Every one is replayed at startup
    - Listed one by one, not as LevelDB's single "oldest needed log number". A failed flush is retried after newer ones, so live segments can have gaps (5 still live, 6 flushed, 7 live). One number would make restart replay 6 again
    - The swap saves the manifest with the new segment before any put goes into it. A segment the manifest doesn't list can't hold an acked write, so deleting it is safe
- Next file number (one counter for WAL segments and SSTables)
- Last seq that's in an SSTable, so the next seq after restart = max(this, last replayed seq) + 1

## File format

<!-- Byte layout, versioning, checksum. -->
- Rewritten as a whole file on every change, not an append-only edit log like LevelDB. It's small (a few hundred bytes), so rewriting costs nothing and there's no log to replay
- Little endian, CRC32 over everything before it

```
| magic 4B "LSMM" | version 2B | next_file_number 8B | last_seq 8B |
| wal count 4B | wal file number 8B ... |
| sst count 4B | sst ... |
| CRC32 4B |

one sst:
| file_number 8B | file_size 8B | min_seq 8B | max_seq 8B |
| first keyLen 1B | first key | last keyLen 1B | last key |
```

| Manifest | Size |
|---|---|
| Empty (new store) | 34 bytes |
| Per WAL segment | 8 bytes |
| Per SSTable (12-byte keys) | 58 bytes |
| 1 segment + 10 SSTables | 622 bytes |

- Decode checks, in order: length, CRC, magic, version, every count and length stays inside the file, no bytes left over
- Then the same checks save runs, so a manifest that could never have been written is caught too:
    - every file number listed once (WAL segments and SSTables share the counter)
    - every file number < next_file_number
    - SSTable min_seq <= max_seq <= last_seq
    - first_key <= last_key
- Counts come from the file, so decode never pre-allocates from them: a bad count can't make a huge allocation

## Updating it safely

<!-- The exact steps (temp file, fsync, rename, fsync directory) and why each one is needed. -->
1. Write `MANIFEST.tmp`
2. fsync it, so the bytes are on disk before it gets the real name
3. Rename to `MANIFEST`, atomic: a crash leaves the old one or the new one, never half of each
4. fsync the directory, so the rename itself survives a crash
- Same steps as an SSTable. Compaction will reuse it to swap files in one step
- `save` refuses to write a manifest that `load` would reject (`ManifestError::Invalid`)

## File names

| File | Name |
|---|---|
| WAL segment | `000007.wal` |
| SSTable | `000008.sst` |
| Manifest | `MANIFEST` |
| Unfinished write | `*.tmp` (`000008.sst.tmp`, `MANIFEST.tmp`) |

- Zero padded to 6 digits so the directory lists in file number order, wider past 999,999
- Files with any other name (`wal.log`, `notes.txt`, `1e3.sst`) aren't ours and are never deleted
- A file number is only on disk once the manifest is saved, so after a crash the same number can be handed out again. Safe because startup deletes unlisted files before creating new ones

## Obsolete files

`obsolete_files(dir, &manifest)` returns what startup should delete:

| File | Why it's there | Obsolete? |
|---|---|---|
| `*.tmp` | Crash before a rename | Yes |
| SSTable not listed | Flush finished, crash before the manifest update | Yes, its WAL segment is still listed and gets replayed |
| WAL segment not listed | Flushed, crash before the delete. Or created by a swap that crashed before the manifest save (empty) | Yes |
| Listed SSTable / segment, `MANIFEST` | Live | No |
| Anything else | Not ours | No, left alone |

## Interface

- `Manifest { next_file_number, last_seq, wal_segments: Vec<u64>, sstables: Vec<SstMeta> }`
- `SstMeta { file_number, file_size, min_seq, max_seq, first_key, last_key }`, `SstMeta::new(file_number, &SstInfo)`
- `Manifest::new()`: a new store (next file number 1, last seq 0)
- `new_file_number(&mut self) -> u64`
- `Manifest::load(dir: &Path) -> Result<Option<Manifest>, ManifestError>`: `None` = no manifest yet
- `save(&self, dir: &Path) -> Result<(), ManifestError>`: durable when it returns Ok
- `encode(&Manifest) -> Result<Vec<u8>, ManifestCorruption>`, `decode(&[u8]) -> Result<Manifest, ManifestCorruption>`
- `wal_file_name(n)`, `sst_file_name(n)`, `parse_file_name(name) -> Option<FileKind>`, `obsolete_files(dir, &manifest)`
- `ManifestError { Io, Corruption(reason), Invalid(reason) }`
    - `ManifestCorruption`: `TooShort`, `Crc`, `BadMagic`, `UnsupportedVersion`, `Truncated`, `TrailingBytes`, `DuplicateFileNumber`, `FileNumberNotAllocated`, `BadSeqRange`, `BadKeyRange`, `KeyTooLong`

## Tests

- Codec (`src/manifest/codec/tests.rs`): 15 tests
    - Round trip, empty manifest is 34 bytes, layout byte for byte
    - Bit flips fail the CRC, every truncation is an error not a panic
    - Bad magic, unsupported version, count past the end, trailing bytes
    - Duplicate file number, file number never handed out, bad seq ranges, bad key range, key too long
    - Decode runs the same checks as save
- Files (`src/manifest/files/tests.rs`): 6 tests
    - Names are zero padded, store files parse, other names are ignored (`wal.log`, `1e3.sst`, `+7.wal`, numbers past u64), names round trip
    - Obsolete files: unlisted SSTable and segment, `.tmp` files, others left alone. Nothing obsolete in a clean directory
- Save / load (`src/manifest/tests.rs`): 8 tests
    - Missing manifest is a new store, save then load, save replaces the old one, no `.tmp` left behind
    - Half-written `MANIFEST.tmp` after a crash: the old manifest loads, the tmp is obsolete
    - Invalid manifest is never saved, corrupt manifest fails load
    - File numbers count up from 1, `SstMeta` from a real SSTable

## Open questions

- No manifest but `.sst` files in the directory (someone deleted `MANIFEST`): open must fail, not treat it as a new store and delete every SSTable. Check goes into `Store::open` in the flush PR
- Compaction (week 4) may need a level or tier per SSTable. The version field leaves room for it

## See also

[FLUSH.md](FLUSH.md) · [COMPACTION.md](COMPACTION.md) · [RECOVERY.md](RECOVERY.md)
