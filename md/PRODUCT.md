# Product

**Status:** Decided Nov 16. Nothing built yet beyond the store and a `.ibt` inspector script.

## North star

- Compare my own sim telemetry against real F1 drivers and against friends who install the app
- Any car against any car: users drive whatever they own (GT3 vs F1 is a valid comparison). Laps line up by track, never by car

## How people use it

- One Windows app (`.exe` installer). Everything runs on the user's PC, there's no server
    - Finds the user's iRacing `.ibt` files (`Documents\iRacing\telemetry`), lists recent drives
    - Reads them and stores the user's laps in lsm-kv, embedded in the app
    - Fetches real F1 laps from OpenF1 when a comparison needs them (2023 and later)
    - Shows the comparison: track map, delta trace, channel charts
- Sharing with friends is by file
    - Export a session (or a lap) to a file, send it however you like (Discord, a drive)
    - The friend imports it, it goes into their store next to their own laps

```
 USER'S WINDOWS PC
 ┌──────────────────────────────────────────────────────────────┐
 │ App (.exe)                                                   │
 │                                                              │
 │  .ibt files ──► .ibt reader ──► the user's laps ──┐          │
 │  (iRacing)      (own car only)                    │          │
 │                                                   ▼          │
 │  OpenF1 ──► fetch on demand, size-capped cache ──► lsm-kv    │
 │  (internet)                                       ▲    │     │
 │                                                   │    ▼     │
 │  friend's export file ──► import (bulk load) ─────┘   UI     │
 │                                                              │
 │  export ◄── one session out of lsm-kv ──► file to send       │
 └──────────────────────────────────────────────────────────────┘
```

## Why this and not a server

| | Local app + file sharing (chosen) | Central server |
|---|---|---|
| Cost | Free, for any number of users | Hosting, or a free tier with limits |
| Comparing with friends | Export a file, friend imports it | Automatic |
| F1 data | Each user reads OpenF1 directly, nobody redistributes it | We'd serve F1's data to others |
| Other drivers in `.ibt` files | Never leaves the PC | Must be stripped before upload |
| What has to keep running | Nothing | Uptime, security, backups, accounts |
| Offline | Works, except fetching new F1 races | Doesn't work |

- Users don't store data they didn't ask for: F1 laps are fetched for the comparison at hand and cached with a size cap, friends' data is only what they imported
- Storage is small anyway: ~15 channels at 60 Hz is ~3.6 KB per second of driving, ~13 MB per hour before compression
- A sync server can come later with the same export format, if manual sharing gets old

## What this means for lsm-kv

- It's an embedded database in a desktop app, like RocksDB inside other apps. Matches the design (single node, Cassandra rejected in DESIGN.md)
- Has to run on Windows: two fixes (positional reads with `seek_read`, no directory fsync), plus a Windows CI job. Small PR, before compaction (4d) adds more file deletes
- Importing a friend's session is bulk load (PR 4e). Session-split SSTables (compaction design) make exporting one session cheap
- Untrusted input: imported files come from other people's PCs. Validate everything on import (the merge and manifest checks are the habit to follow)

## Pieces

| Piece | Status |
|---|---|
| lsm-kv store | Done for writes, reads, flush, bulk load ([BULK_LOAD.md](BULK_LOAD.md)). Compaction, scan next |
| `.ibt` reader | Format checked on 4 real files (`scripts/ibt_inspect.py`), Rust reader next. See [IBT.md](IBT.md) |
| Lap processing | Split laps / sectors, distance grid, best laps, sector times. Not started |
| OpenF1 fetch + cache | Not started. Spa 2023 is session_key 9141 |
| Export / import | Import side done (bulk load), export next |
| Windows support | Not started |
| UI (Tauri, uPlot) | Not started. Screens modelled on VRS / Coach Dave Delta: map with both lines, delta trace, stacked channels, shared cursor, sector table |
| Live collection (iRacing SDK shared memory) | Later |

## Open questions

- iRacing's terms on telemetry: users only handle their own files, but check before friends share exports
- Track identity across sources: iRacing `TrackID` (Spa is 523) vs OpenF1 `circuit_key`, and lap length differs (iRacing Spa 6.929 km, real Spa ~7.004 km). Line laps up by lap distance percentage
- Sectors differ by source: iRacing's Spa has 4 split points, F1 uses 3 sectors
- Installer signing: unsigned `.exe` shows a SmartScreen warning. Fine for friends, a certificate costs money
- A sync server, if the friend group outgrows sending files
- Faster drivers' telemetry (kept in mind, Nov 16): `.ibt` files have none of it, only other drivers' identity. The clean route is consent, drivers who choose to share their laps. Details in [IBT.md](IBT.md#other-drivers-telemetry-idea-not-planned)

## See also

[IBT.md](IBT.md) · [OVERVIEW.md](OVERVIEW.md) · [COMPACTION.md](COMPACTION.md)
