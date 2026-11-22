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
| lsm-kv store (`crates/lsm-kv`) | Done for writes, reads, scan, flush, bulk load ([BULK_LOAD.md](BULK_LOAD.md), [SCAN.md](SCAN.md)). Compaction after the first demo |
| `.ibt` reader (`crates/racedata`) | Reader done (Nov 17), chunks + import next. See [IBT.md](IBT.md) |
| Lap processing | Laps and sector times done in the reader. Distance grid, delta: Python prototype first (notebook, my lap vs Verstappen), then Rust or TypeScript in the app, tested against the prototype |
| OpenF1 fetch + cache | Not started. Spa 2023 is session_key 9141 |
| Export / import | Import side done (bulk load), export next |
| Windows support | Not started |
| UI | Not started. Tauri + uPlot (decided Nov 18): Rust backend calls `racedata` / `lsm-kv` directly, web front end. Requirements below |
| Live collection (iRacing SDK shared memory) | Later |

## UI requirements (Nov 18)

Framework: Tauri + uPlot, with lap processing prototyped in Python first.

| Rejected | Why |
|---|---|
| Python (Qt + pyqtgraph) | One language, but a 100-200 MB installer with a slow cold start, Rust -> Python bindings to read the store, and Qt styling is clunky |
| Electron | Same web front end as Tauri, but ships its own browser (~100 MB+) and its backend is Node, not Rust |

- uPlot gives the shared cursor (`cursor.sync`) and linked zoom out of the box
- Researched against VRS, Coach Dave Delta and lap-comparison guides (Nov 18): they all start from the delta, then speed, then throttle / brake

### First version: the comparison screen, nothing else

Scope rule: v1 shows **what each driver did** at every point of the lap, side by side, so the user can work out what the other driver does differently. The tool gives the data, the driver draws the conclusions: no coaching, no hints, no generated analysis

| Part | v1 | Why it's needed |
|---|---|---|
| Lap picker | Pick two laps from what's in the store (my laps, an imported friend's, an F1 lap) | Nothing works without it |
| Track map (left) | Sim vs sim (me vs a friend): both laps' driven lines, one marker per car on its own line. F1 vs sim: one track outline, both markers placed by lap distance (the coordinates don't match, TRACKS.md) | Where on track this is, and each driver's line |
| Delta time chart | Time gained / lost along the lap, lap B minus lap A at each distance | Finds the biggest losses: every guide starts here |
| Speed chart | Both laps overlaid | First explanation: braking point, minimum corner speed, exit speed |
| Throttle chart | Both laps overlaid | When and how hard they got back on the power |
| Brake chart | Both laps overlaid | Where braking starts and how it's released |
| Gear chart | Both laps overlaid | Every source has it |
| RPM chart | Both laps overlaid | Shift points, how each driver uses the engine |
| Steering chart | Both laps overlaid. Sim laps only, an F1 lap shows no line (OpenF1 has no steering) | How much lock, how early, corrections |
| Inputs widget (under the map) | For each driver at the cursor: steering wheel angle, throttle / brake / clutch bars, gear | What the driver is physically doing at that spot, at a glance |
| Shared cursor | Hover anywhere: a dotted line across every chart, both map markers move, each chart shows both laps' values at that point | Ties the map to the charts |
| Linked zoom | Zoom one chart, all follow | Free in uPlot |
| Lap and sector times | Both laps, official sectors (TRACKS.md), the difference per sector | Sectors are the comparison unit |

- x axis is lap distance (0 to 1 of the lap), never time: the same point on track lines up
- Two colours, one per lap, the same on the map and every chart

### Not in v1

| Feature | Why it waits |
|---|---|
| F1 and sim lines drawn together on the map | iRacing gives lat / lon, OpenF1 a different x / y frame. Overlaying them needs the shape fit (TRACKS.md open question) |
| Line distance (how far off the racing line) | Needs a reference line per track |
| Clutch, handbrake charts | Rarely used mid-lap. Clutch still shows in the inputs widget |
| Tyres, wheel slip, ride height | Car setup detail, and not in F1 data |
| Video, theoretical best lap, corner-by-corner reports, coaching hints | Out of scope: the tool shows data, it doesn't analyse it for the driver |
| Comparing three or more laps | Two is the comparison |

### Limits of the data to know about

- OpenF1 samples at ~3.7 Hz (iRacing 60 Hz): F1 traces are coarse, one point every ~20 m at 280 km/h
- OpenF1's brake is on / off (0 or 100), not pressure: the brake chart shows where an F1 driver braked, not how hard. Shown as-is. No public F1 source has pressure or steering (checked Nov 18: OpenF1, FastF1 and the sites built on them all come from F1's live timing feed). Rejected: estimating "how hard" from deceleration, too unreliable at 3.7 Hz
- OpenF1's throttle is "percentage of maximum engine power being used", not pedal position. Close enough to compare, but not the same thing as iRacing's pedal
- No steering in OpenF1

## Open questions

- iRacing's terms on telemetry: users only handle their own files, but check before friends share exports
- Track identity across sources: iRacing `TrackID` (Spa is 523) vs OpenF1 `circuit_key`, and lap length differs (iRacing Spa 6.929 km, real Spa ~7.004 km). Line laps up by lap distance percentage
- Sectors differ by source: iRacing's Spa has 4 split points, F1 uses 3 sectors
- Installer signing: unsigned `.exe` shows a SmartScreen warning. Fine for friends, a certificate costs money
- A sync server, if the friend group outgrows sending files
- Faster drivers' telemetry (kept in mind, Nov 16): `.ibt` files have none of it, only other drivers' identity. The clean route is consent, drivers who choose to share their laps. Details in [IBT.md](IBT.md#other-drivers-telemetry-idea-not-planned)

## See also

[IBT.md](IBT.md) · [OVERVIEW.md](OVERVIEW.md) · [COMPACTION.md](COMPACTION.md)
