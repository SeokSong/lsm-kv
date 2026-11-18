# Tracks and sectors

**Status:** Decided and built Nov 17: `crates/racedata/tracks/tracks.toml` (Spa, Red Bull Ring), `crates/racedata/src/track.rs`, 4 tests.

## The problem

Sector-to-sector comparison only works if "sector 2" is the same stretch of track for every source. It isn't by default:

| Spa | iRacing split points | Official F1 sector lines (2023) | Gap |
|---|---|---|---|
| Sector 2 starts | 0.3131 | 0.3231 | ~0.010 of the lap, ~70 m |
| (extra) | 0.5229 | none | iRacing only |
| Sector 3 (F1) / 4 (iRacing) starts | 0.7231 | 0.7244 | ~9 m |

| Red Bull Ring | iRacing | Official F1 (2023) | Gap |
|---|---|---|---|
| Sector 2 starts | 0.2719 | 0.2826 | ~45 m |
| Sector 3 starts | 0.6682 | 0.6750 | ~29 m |

- On a real W13 lap at Spa that's 0.9 s of difference in "S1" (34.366 s by iRacing's split vs 35.221 s by the F1 line)
- iRacing's splits belong to the track: the Mustang GT3 and the W13 files have the same ones

## Decision

- The key's sector field is the **official F1 sector** on tracks in the F1 calendar, for every source. "Sector 2" is the same stretch everywhere
- On other tracks (Daytona, ...) it's iRacing's own split points
- Every other set of lines (iRacing's splits on an F1 track) is computed from the samples and stored as derived data, never in keys
- Rejected: each source keyed by its own sectors ("sector 2" would mean different stretches), and segments cut at every line from every source (the key layout would change whenever any source's lines change, and Spa would get a 70 m segment holding 3-4 F1 samples)

## The track table

`crates/racedata/tracks/tracks.toml`, built into the program (`include_str!`). Adding a track needs no code change.

| Field | |
|---|---|
| `name` | Display name |
| `iracing_track_ids` | iRacing's `TrackID`s for this layout (Spa Grand Prix: 523) |
| `openf1_circuit_key` | OpenF1's circuit (Spa: 7) |
| `f1_sector_lines` | Where sector 2, 3, ... start, as lap fractions |

- `track::key_sectors(track_id, iracing_splits)` returns the lines and where they came from (`SectorSource::F1` or `IRacing`)
- `SectorLines::sector_of(lap_dist_pct)`: 1-based sector, a line belongs to the sector it starts

## Measuring the F1 lines

OpenF1 gives each lap's sector times, not where the lines are. `scripts/openf1_sector_lines.py <session> <drivers>`:
1. Cumulative driven distance along each car's x / y trace
2. For each clean lap: where the car was when sector 1 and sector 2 ended, as a fraction of that lap's distance
3. Median over every lap and driver

| Track | Session | Laps | p10-p90 spread |
|---|---|---|---|
| Spa, 2023 Belgian GP | 9141 | 125 (drivers 1, 16, 44) | +-0.001 (~7 m) |
| Red Bull Ring, 2023 Austrian GP | 9118 | 203 | +-0.002 (~9 m) |

## Lap and sector timing in iRacing

- Lap start = `SessionTime - LapCurrentLapTime`: iRacing's own lap clock, constant to ~10 us across a lap. Consecutive starts subtract to iRacing's official lap time exactly (5 laps checked, 0.1 ms)
- Not the interpolated crossing at LapDistPct = 1.0: iRacing's timing line sits ~0.4-0.8 m before that point, a few ms off depending on speed (which is why the error's sign flipped between laps)
- Sector lines: the time where LapDistPct crosses the line, interpolated between the two rows either side (60 Hz)
- F1 laps (OpenF1, next): sector times come from official timing, not from 3.7 Hz samples

## Open questions

- Are the measured F1 lines biased? Both tracks put them later than iRacing's lines, by a varying amount. A bias in OpenF1's position timestamps or in summing the x / y trace can't be ruled out. Check by matching track shapes: fit iRacing's lat / lon lap onto OpenF1's x / y lap (rotation, scale, offset), then read off where the official lines are on the iRacing track directly, in meters
- Lap fraction isn't the same physical point across sims and reality to the meter (iRacing Spa is 6,929 m, the F1 driven line ~6,950 m). The shape fit above also answers this
- More tracks: run the script for each F1 circuit as sessions get imported

## See also

[IBT.md](IBT.md) · [DESIGN.md](DESIGN.md) · [PRODUCT.md](PRODUCT.md)
