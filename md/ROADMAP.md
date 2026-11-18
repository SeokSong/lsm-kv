# Roadmap

**Written:** Nov 16. A short reference for staying on course. Update the status column as things land, don't add detail here (that goes in the component docs).

## North star

Compare my own sim telemetry against real F1 drivers and against friends. One Windows app, lsm-kv embedded, no server, friends share by file ([PRODUCT.md](PRODUCT.md)).

## Off-course check

Before starting something, it should pass at least one:
- Does it get the first demo closer (milestone 1)?
- Is it needed so the data stays correct (durability, recovery, imports from other PCs)?
- Is it the database depth the resume needs (compaction, crash tests)?

If none: it waits. Examples that wait for now: group commit, `Mutex` vs `RwLock`, a sync server, public leaderboards.

## Milestones

| # | Milestone | Done when | Status |
|---|---|---|---|
| 0 | The store | WAL, memtables, flush, SSTables, manifest, recovery, merge iterator, bulk load | Done (Nov 16) |
| 1 | First demo | My W13 lap at Spa against Verstappen's 2023 lap, map + delta + speed / throttle / brake, on screen | Next |
| 2 | Sharing | A friend exports a session, I import it, we compare | |
| 3 | Store depth | Leveled compaction + its thread, `kill -9` crash tests, property tests | |
| 4 | Ship to friends | Windows build + CI, installer, `.ibt` folder picker, session list | |
| 5 | Later | Live collection (iRacing SDK), consent-based sharing of faster drivers' laps, a sync server if needed | |

## Milestone 1, in order

| Step | What | Doc |
|---|---|---|
| 1 | Value encoding: column chunks, the channel catalog (Rust). Done Nov 17 | [CHUNK.md](CHUNK.md) |
| 2 | `.ibt` reader (Rust): own car only, laps, sectors (done Nov 17). Workspace split into `lsm-kv` + `racedata` (Nov 18). Then chunks + import | [IBT.md](IBT.md), [TRACKS.md](TRACKS.md) |
| 3 | `scan(start, end)` | [MERGE.md](MERGE.md) |
| 4 | OpenF1 fetch for one race, with a cache cap. Lap distance computed at ingest | [PRODUCT.md](PRODUCT.md) |
| 5 | Lap processing: both laps on a distance grid, delta, sector times | |
| 6 | Minimal UI (Tauri + uPlot): pick two laps, map, delta trace, 3 channels | [PRODUCT.md](PRODUCT.md) |

## Milestones 2-4, rough order

- Export a session (milestone 2)
- Windows support (`seek_read`, no directory fsync, Windows CI). Before compaction's thread (milestone 3-4)
- Compaction core (4c), then its thread (4d)
- Crash tests, property tests, cross-segment seq check at open
- Installer, folder picker, session list

## Decisions that shape everything

| Decision | Where |
|---|---|
| Local app, no server, sharing by file | [PRODUCT.md](PRODUCT.md) |
| Any car vs any car, line up by track + lap distance pct | [PRODUCT.md](PRODUCT.md) |
| iRacing `.ibt` for my driving, OpenF1 2023+ for F1 | [IBT.md](IBT.md), [OVERVIEW.md](OVERVIEW.md) |
| Keys, sim namespaces, column chunks, raw samples | [DESIGN.md](DESIGN.md) |
| Other drivers' identity never stored, faster drivers only by consent | [IBT.md](IBT.md) |
| Leveled compaction, split by namespace + session | [COMPACTION.md](COMPACTION.md) |
| Development in Docker (Linux), users get a native app | [DEVELOPMENT.md](DEVELOPMENT.md) |
