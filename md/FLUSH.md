# Flush

**Status:**

## Purpose

<!-- What does a flush do? How is it different from compaction? -->

## Trigger

<!-- When does a flush start? -->

## Steps

<!-- Numbered, in order. -->

## Crash at each step

<!-- For each step above: what's on disk if you crash right there, and what does recovery do? -->

| Crash after step | What's on disk | What recovery does |
|---|---|---|
| | | |

## Writes during a flush

<!-- Where do new puts go while the flush runs? -->

## Interface

## Tests

## Open questions

## See also

[MEMTABLE.md](MEMTABLE.md) · [WAL.md](WAL.md) · [MANIFEST.md](MANIFEST.md) · [RECOVERY.md](RECOVERY.md)
