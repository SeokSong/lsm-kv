# Manifest

**Status:** Designed (Nov 13), built in the next PR.

## Purpose

<!-- What question does the manifest answer? Why can't you just list the directory? -->
- Answers "which files are live?" on startup: which SSTables count, which WAL segments still need replaying
- Listing the directory breaks with compaction: it swaps several SSTables for one, a crash halfway leaves old and new files that all look live
- Also tells startup which files are orphans (a finished SSTable from a flush that crashed before the manifest update)

## Contents

<!-- Every field: live SSTables, WAL segment number, anything else? -->
- Live SSTables: file number, min / max seq, first / last key
- Live WAL segments, oldest first
- Next file number (one counter for WAL segments and SSTables)
- Last seq that's in an SSTable, so the next seq after restart = max(this, last replayed seq) + 1

## File format

<!-- Byte layout, versioning, checksum. -->
- Rewritten as a whole file on every change, not an append-only edit log like LevelDB. It's small (a few hundred bytes), so rewriting costs nothing and there's no log to replay
- Magic number + format version + CRC, byte layout decided in the PR

## Updating it safely

<!-- The exact steps (temp file, fsync, rename, fsync directory) and why each one is needed. -->
1. Write `MANIFEST.tmp`
2. fsync it, so the bytes are on disk before it gets the real name
3. Rename to `MANIFEST`, atomic: a crash leaves the old one or the new one, never half of each
4. fsync the directory, so the rename itself survives a crash
- Same steps as an SSTable. Compaction will reuse it to swap files in one step

## Interface

## Tests

## Open questions

## See also

[FLUSH.md](FLUSH.md) · [COMPACTION.md](COMPACTION.md) · [RECOVERY.md](RECOVERY.md)
