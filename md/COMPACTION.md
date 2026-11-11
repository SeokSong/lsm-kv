# Compaction

**Status:**

## Purpose

<!-- What problems does compaction solve? What happens if you never compact? -->

## Strategy

<!-- Size-tiered first? What triggers a compaction? Which SSTables get picked? -->

## Merge

<!-- How do you merge several sorted SSTables? Which version of a key wins? -->

## Tombstones

<!-- When is it safe to drop a tombstone, and why? -->

## Swapping in the result

<!-- How do the new SSTable(s) replace the old ones safely? What if you crash midway? -->

## Write amplification

<!-- How will you measure it? -->

## Interface

## Tests

## Open questions

## See also

[SSTABLE.md](SSTABLE.md) · [MANIFEST.md](MANIFEST.md) · [RECOVERY.md](RECOVERY.md)
