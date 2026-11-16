# iRacing .ibt files

**Status:** Format checked on 4 real files (Nov 16) with `scripts/ibt_inspect.py`. Rust reader not started.

## Where they come from

- iRacing writes one file per stint (getting in the car until getting out) to `Documents\iRacing\telemetry\` on the PC that runs the sim. Telemetry logging has to be turned on in iRacing
- No API or token: the web Data API (paused for third parties) isn't involved
- Local copies for development go in `data/` (git ignored: big files, and they hold other drivers' names and ids)

## Layout (little endian)

| Part | Where | Size |
|---|---|---|
| Header: version, tick rate, session info offset / length, var count, var header offset, bytes per row, first row offset | 0 | 112 B |
| Disk sub header: session start date, start / end time, lap count, record count | 112 | 32 B |
| Var headers, one per channel: type, offset in a row, count, name, description, unit | var header offset | 144 B each |
| Session info: YAML text (track, cars, drivers, sectors) | session info offset | session info length |
| Sample rows, fixed size | first row offset | bytes per row each |

| Var type | Rust |
|---|---|
| 0 char, 1 bool | `u8`, `bool` |
| 2 int, 3 bitfield | `i32`, `u32` |
| 4 float, 5 double | `f32`, `f64` |

- Rows = (file size - first row offset) / bytes per row. The header's lap count isn't reliable (0 in files with laps): count laps from the `Lap` channel

## The 4 sample files

| File | Car | Track (TrackID) | Session | Length | Size | Drivers in session info |
|---|---|---|---|---|---|---|
| `mercedesw13_spa ...` | Mercedes W13 | Spa (523) | Offline testing | 479 s, 4 laps | 32 MB | 1 |
| `fordmustanggt3_spa ...` | Ford Mustang GT3 | Spa (523) | Practice (online) | 664 s | 44 MB | 20 |
| `bmwm4gt3_spielberg ...` | BMW M4 GT3 EVO | Red Bull Ring (403) | Practice (online) | 474 s | 31 MB | 15 |
| `ferrari499p_daytona ...` | Ferrari 499P | Daytona road (192) | Practice (online) | 7 s | 0.5 MB | 6 |

- Every file: 60 Hz, 292 channels, 1,121 bytes per row (W13)
- Raw size ~67 KB per second (~240 MB per hour). Keeping ~15 channels: ~3.6 KB per second
- Two cars at the same track (523): the F1 vs GT3 comparison case

## Channels we need (all present)

| Channel | What | Notes |
|---|---|---|
| `SessionTime` | Seconds since the session started | Row time |
| `Lap`, `LapCurrentLapTime` | Lap number, time into the lap | Split laps on `Lap` changing |
| `LapDist`, `LapDistPct` | Meters / fraction into the lap | What comparisons line up on |
| `Speed` | m/s | |
| `Throttle`, `Brake` | 0..1 | |
| `SteeringWheelAngle` | Radians | Real F1 data has no steering |
| `Gear`, `RPM` | | |
| `Lat`, `Lon` | GPS position | Track map |

## Privacy

- Telemetry channels are only the user's own car: 286 single-value channels and one with 6 values, no per-car (`CarIdx...`) arrays. Those exist in iRacing's live SDK, not in `.ibt` files (checked Nov 16, corrects an earlier note)
- But online sessions list every driver in the session info: name, iRacing id, iRating, license, club, team, car, paint designs, incidents, and some lap times (`ResultsPositions`). 20 drivers in the Mustang file
- So the reader keeps only the user's own car (`DriverCarIdx`) and the track / car info. Nothing about other drivers goes into the store or into an export

### Other drivers' telemetry (idea, not planned)

Seeing a faster driver's telemetry would be useful. Where it could come from, and the catch:

| Source | What you'd get | Concern |
|---|---|---|
| `.ibt` files | Nothing: only identity and some lap times | Their identity is personal data, so it's stripped |
| iRacing live SDK, per-car arrays during a session | Other cars' lap distance, position, and some state (gear, RPM, steering, as far as I know), no throttle or brake | Recording people who never agreed to it. Check iRacing's terms before even trying |
| Faster drivers who choose to share (friends, an opt-in "publish my laps") | Full telemetry | None if they agreed. This is how free sites like Garage 61 do it |
| Paid reference laps (VRS datapacks and others) | Full telemetry | Their content, not ours to copy or redistribute |

- The only clean route is consent: drivers share their own laps on purpose. That fits the file sharing model already
- Names and ids are personal data in privacy law (GDPR in the EU), which is why they never go into the store

## Tool

- `python3 -I scripts/ibt_inspect.py <file.ibt>`: header, row count, length
- `--channels`: every channel with type, count, unit, description
- `--yaml`: the session info
- `--samples N`: N rows spread over the file, the channels above
- Check a Rust reader against it

## Open questions

- Laps: the first and last lap of a stint are partial (out lap, in lap), mark them so they aren't compared
- iRacing's Spa has 4 split points (0, 0.313, 0.523, 0.723 lap distance), F1 has 3 sectors. Use iRacing's for iRacing laps, map to F1 sectors when comparing against F1
- `TrackNumTurns` is 19 in the W13 file and 21 in the Mustang file for the same track: don't rely on it

## See also

[PRODUCT.md](PRODUCT.md) · [OVERVIEW.md](OVERVIEW.md)
