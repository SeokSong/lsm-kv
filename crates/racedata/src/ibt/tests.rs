use super::*;
use crate::track::{self, SectorSource};
use std::path::{Path, PathBuf};

// ---------- a synthetic .ibt writer: known motion, so lap and sector times are exact ----------

struct Row {
    time: f64,
    lap: i32,
    pct: f32,
    on_track: bool,
    pit: bool,
}

const SESSION_INFO: &str = "---
WeekendInfo:
 TrackName: spa 2024 up
 TrackID: 523
 TrackLength: 6.9293 km
 TrackDisplayName: Circuit de Spa-Francorchamps
 TrackConfigName: Grand Prix
DriverInfo:
 DriverCarIdx: 1
 DriverUserID: 4242
 Drivers:
 - CarIdx: 0
   UserName: Somebody Else
   UserID: 999
   CarScreenName: Ford Mustang GT3
   CarID: 185
 - CarIdx: 1
   UserName: Me: The Driver
   UserID: 4242
   CarScreenName: Mercedes-AMG W13 E Performance
   CarID: 172
SplitTimeInfo:
 Sectors:
 - SectorNum: 0
   SectorStartPct: 0.000000
 - SectorNum: 1
   SectorStartPct: 0.313076
 - SectorNum: 2
   SectorStartPct: 0.522864
 - SectorNum: 3
   SectorStartPct: 0.723114
...
";

// channels: SessionTime double, Lap int, LapDistPct float, IsOnTrack bool, OnPitRoad bool,
// and LapCurrentLapTime float when a lap clock is given (one value per row)
fn ibt_bytes(rows: &[Row], session_info: &str) -> Vec<u8> {
    ibt_bytes_with_clock(rows, session_info, None)
}

fn ibt_bytes_with_clock(rows: &[Row], session_info: &str, clock: Option<&[f32]>) -> Vec<u8> {
    let mut vars: Vec<(&str, i32, i32)> = vec![
        ("SessionTime", 5, 0),
        ("Lap", 2, 8),
        ("LapDistPct", 4, 12),
        ("IsOnTrack", 1, 16),
        ("OnPitRoad", 1, 17),
    ];
    if clock.is_some() {
        vars.push(("LapCurrentLapTime", 4, 18));
    }
    let row_len: i32 = if clock.is_some() { 22 } else { 18 };
    let var_offset: i32 = 144;
    let info_offset: i32 = var_offset + 144 * vars.len() as i32;
    let info: &[u8] = session_info.as_bytes();
    let first_row: i32 = info_offset + info.len() as i32;

    let mut out: Vec<u8> = vec![0; first_row as usize];
    let mut put = |at: usize, bytes: &[u8]| out[at..at + bytes.len()].copy_from_slice(bytes);
    put(0, &2i32.to_le_bytes());
    put(8, &60i32.to_le_bytes());
    put(16, &(info.len() as i32).to_le_bytes());
    put(20, &info_offset.to_le_bytes());
    put(24, &(vars.len() as i32).to_le_bytes());
    put(28, &var_offset.to_le_bytes());
    put(32, &1i32.to_le_bytes());
    put(36, &row_len.to_le_bytes());
    put(52, &first_row.to_le_bytes());
    put(112, &1_794_869_519i64.to_le_bytes());
    put(140, &(rows.len() as i32).to_le_bytes());
    for (i, (name, code, offset)) in vars.iter().enumerate() {
        let at: usize = var_offset as usize + i * 144;
        put(at, &code.to_le_bytes());
        put(at + 4, &offset.to_le_bytes());
        put(at + 8, &1i32.to_le_bytes());
        put(at + 16, name.as_bytes());
    }
    put(info_offset as usize, info);
    for (i, row) in rows.iter().enumerate() {
        out.extend_from_slice(&row.time.to_le_bytes());
        out.extend_from_slice(&row.lap.to_le_bytes());
        out.extend_from_slice(&row.pct.to_le_bytes());
        out.push(row.on_track as u8);
        out.push(row.pit as u8);
        if let Some(clock) = clock {
            out.extend_from_slice(&clock[i].to_le_bytes());
        }
    }
    out
}

// constant speed: one lap takes lap_seconds, sampled at 60 Hz, starting at start_pct of lap 0
fn drive(laps: i32, lap_seconds: f64, start_pct: f64) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let mut row: usize = 0;
    loop {
        let time: f64 = 10.0 + row as f64 / 60.0;
        let distance: f64 = start_pct + (time - 10.0) / lap_seconds;
        let lap: i32 = distance.floor() as i32;
        if lap > laps {
            break;
        }
        rows.push(Row {
            time,
            lap,
            pct: (distance - lap as f64) as f32,
            on_track: true,
            pit: false,
        });
        row += 1;
    }
    rows
}

fn read(rows: &[Row]) -> (IbtFile, LapChannels, Vec<Lap>) {
    let file: IbtFile = IbtFile::parse(ibt_bytes(rows, SESSION_INFO)).unwrap();
    let channels: LapChannels = LapChannels::read(&file).unwrap();
    let laps: Vec<Lap> = split_laps(&channels);
    (file, channels, laps)
}

// ---------- the file format ----------

#[test]
fn parses_header_channels_and_rows() {
    let rows: Vec<Row> = drive(1, 100.0, 0.5);
    let (file, _, _) = read(&rows);

    assert_eq!(file.tick_rate, 60);
    assert_eq!(file.rows, rows.len());
    assert_eq!(file.start_date, 1_794_869_519);
    assert_eq!(file.vars.len(), 5);
    assert_eq!(file.var("Lap").unwrap().var_type, VarType::Int);
    assert_eq!(file.column_f64("SessionTime").unwrap()[3], rows[3].time);
    assert_eq!(
        file.column_f64("LapDistPct").unwrap()[3],
        rows[3].pct as f64
    );
    assert!(file.column_bool("IsOnTrack").unwrap()[0]);
    assert!(file.column_f64("Speed").is_none());
}

#[test]
fn every_truncation_is_an_error_or_fewer_rows() {
    let bytes: Vec<u8> = ibt_bytes(&drive(0, 100.0, 0.9), SESSION_INFO);
    let full: usize = IbtFile::parse(bytes.clone()).unwrap().rows;
    for len in 0..bytes.len() {
        // cut inside the headers or session info: an error. cut inside the rows: the whole
        // rows before the cut, like a file the sim stopped writing mid-row
        match IbtFile::parse(bytes[..len].to_vec()) {
            Ok(file) => assert!(file.rows < full, "length {len}"),
            Err(IbtError::TooShort) => {}
            Err(other) => panic!("length {len}: {other:?}"),
        }
    }
}

#[test]
fn bad_headers_are_errors() {
    let bytes: Vec<u8> = ibt_bytes(&drive(0, 100.0, 0.9), SESSION_INFO);
    let with = |at: usize, value: i32| {
        let mut bad: Vec<u8> = bytes.clone();
        bad[at..at + 4].copy_from_slice(&value.to_le_bytes());
        IbtFile::parse(bad)
    };
    assert!(matches!(with(8, 0), Err(IbtError::BadHeader("tick rate"))));
    assert!(matches!(with(24, -1), Err(IbtError::BadHeader(_))));
    assert!(matches!(with(24, 1_000_000), Err(IbtError::TooShort)));
    assert!(matches!(
        with(36, 0),
        Err(IbtError::BadHeader("row length"))
    ));
    // a channel pointing outside the row
    assert!(matches!(
        with(144 + 4, 100),
        Err(IbtError::BadHeader("channel outside the row"))
    ));
    // an unknown channel type
    assert!(matches!(
        with(144, 9),
        Err(IbtError::BadHeader("channel type"))
    ));
}

// ---------- session info ----------

#[test]
fn session_info_is_the_own_car_only() {
    let (file, _, _) = read(&drive(0, 100.0, 0.9));
    let info: SessionInfo = SessionInfo::read(&file).unwrap();

    assert_eq!(info.session_id, 1_794_869_519);
    assert_eq!(info.driver_id, 4242);
    assert_eq!(info.track_id, 523);
    assert_eq!(info.track_name, "spa 2024 up");
    assert_eq!(info.track_display_name, "Circuit de Spa-Francorchamps");
    assert_eq!(info.track_config, "Grand Prix");
    assert!((info.track_length_m - 6929.3).abs() < 1e-6);
    // the other car (idx 0) is never picked
    assert_eq!(info.car, "Mercedes-AMG W13 E Performance");
    assert_eq!(info.car_id, 172);
    assert_eq!(info.iracing_splits, vec![0.0, 0.313076, 0.522864, 0.723114]);
}

#[test]
fn missing_session_info_is_an_error() {
    let file: IbtFile = IbtFile::parse(ibt_bytes(
        &drive(0, 100.0, 0.9),
        "WeekendInfo:\n TrackID: 523\n",
    ))
    .unwrap();
    assert!(matches!(
        SessionInfo::read(&file),
        Err(IbtError::MissingSessionInfo(_))
    ));
}

#[test]
fn yaml_subset() {
    let root: Node = parse_yaml(SESSION_INFO);
    assert_eq!(root.str("WeekendInfo.TrackID"), Some("523"));
    // list items at the same indent as their parent key
    assert_eq!(root.list("SplitTimeInfo.Sectors").len(), 4);
    assert_eq!(root.list("DriverInfo.Drivers").len(), 2);
    // a value with a colon in it stays whole
    assert_eq!(
        root.list("DriverInfo.Drivers")[1].str("UserName"),
        Some("Me: The Driver")
    );
    assert_eq!(root.str("Nope.Missing"), None);

    // never fails: odd lines are skipped, quotes come off
    let odd: Node = parse_yaml("A:\n B: \"quoted\"\n not a key\n      deep: 1\nC: 2\n");
    assert_eq!(odd.str("A.B"), Some("quoted"));
    assert_eq!(odd.str("C"), Some("2"));
}

// ---------- laps ----------

#[test]
fn laps_split_at_the_line_with_exact_times() {
    // start mid lap 0, drive to lap 3: laps 1 and 2 are complete, 100 s each
    let (_, channels, laps) = read(&drive(3, 100.0, 0.5));
    let numbers: Vec<u16> = laps.iter().map(|lap| lap.number).collect();
    assert_eq!(numbers, vec![0, 1, 2, 3]);

    // lap 0 starts mid lap (pit box), lap 3 never ends: not complete
    assert!(!laps[0].complete);
    assert!(!laps[3].complete);
    for lap in &laps[1..3] {
        assert!(lap.complete, "lap {}", lap.number);
        assert!((lap.lap_time().unwrap() - 100.0).abs() < 1e-3);
    }
    // the line crossing is interpolated between rows, not snapped to one
    assert!((laps[1].start_time.unwrap() - 60.0).abs() < 1e-3);
    assert_eq!(laps[0].lap_time(), None);
    let _ = channels;
}

// iRacing's timing line sits a little before the point where LapDistPct wraps: its lap clock
// says the lap started 10 ms before the interpolated crossing. the clock wins
#[test]
fn iracing_lap_clock_gives_the_lap_start() {
    let rows: Vec<Row> = drive(3, 100.0, 0.5);
    // true lap starts: 10 ms before the crossing at 60 s, 160 s, 260 s
    let true_start = |lap: i32| 60.0 + 100.0 * (lap - 1) as f64 - 0.010;
    let clock: Vec<f32> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            // the first 3 rows after the line still show the previous lap's clock, as in the
            // real files
            let lap: i32 = if i >= 3 && rows[i - 3].lap < row.lap {
                row.lap - 1
            } else {
                row.lap
            };
            if lap < 1 {
                0.0
            } else {
                (row.time - true_start(lap)) as f32
            }
        })
        .collect();
    let file: IbtFile =
        IbtFile::parse(ibt_bytes_with_clock(&rows, SESSION_INFO, Some(&clock))).unwrap();
    let channels: LapChannels = LapChannels::read(&file).unwrap();
    let laps: Vec<Lap> = split_laps(&channels);

    for lap in &laps[1..3] {
        // f32 clock values: ~10 us of rounding at these times
        assert!(
            (lap.start_time.unwrap() - true_start(lap.number as i32)).abs() < 1e-4,
            "lap {}: {:?}",
            lap.number,
            lap.start_time
        );
        assert!((lap.lap_time().unwrap() - 100.0).abs() < 1e-4);
    }
}

#[test]
fn a_lap_clock_that_disagrees_with_the_crossing_is_ignored() {
    let rows: Vec<Row> = drive(2, 100.0, 0.5);
    // nonsense clock: says every lap started 5 s before the line
    let clock: Vec<f32> = rows.iter().map(|row| (row.time - 55.0) as f32).collect();
    let file: IbtFile =
        IbtFile::parse(ibt_bytes_with_clock(&rows, SESSION_INFO, Some(&clock))).unwrap();
    let laps: Vec<Lap> = split_laps(&LapChannels::read(&file).unwrap());
    assert!((laps[1].start_time.unwrap() - 60.0).abs() < 1e-3);
}

#[test]
fn sector_times_for_any_set_of_lines() {
    let (_, channels, laps) = read(&drive(2, 100.0, 0.5));
    let lap: &Lap = &laps[1];

    // F1 lines at Spa and iRacing's splits: constant speed, so time = fraction x 100 s
    let f1: Vec<f64> = sector_times(&channels, lap, &[0.3231, 0.7244]).unwrap();
    let expected: [f64; 3] = [32.31, 40.13, 27.56];
    for (got, want) in f1.iter().zip(expected) {
        assert!((got - want).abs() < 1e-3, "{got} vs {want}");
    }
    let iracing: Vec<f64> = sector_times(&channels, lap, &[0.313076, 0.522864, 0.723114]).unwrap();
    assert_eq!(iracing.len(), 4);
    let total: f64 = iracing.iter().sum();
    assert!((total - lap.lap_time().unwrap()).abs() < 1e-9);
    // an incomplete lap has no sector times
    assert_eq!(sector_times(&channels, &laps[0], &[0.5]), None);
}

#[test]
fn the_glitch_row_when_leaving_the_car_is_dropped() {
    // as in the real files: Lap 0, pct 0 for one row, then the car is back at the pit box
    let mut rows: Vec<Row> = drive(2, 100.0, 0.5);
    let glitch_at: usize = rows.len() - 60;
    rows[glitch_at].lap = 0;
    rows[glitch_at].pct = 0.0;
    for row in &mut rows[glitch_at + 1..] {
        row.pct = 0.0384;
        row.pit = true;
    }
    let (_, _, laps) = read(&rows);

    let numbers: Vec<u16> = laps.iter().map(|lap| lap.number).collect();
    assert_eq!(numbers, vec![0, 1, 2]);
    assert!(laps[1].complete);
    // the jump to the pit box makes the in lap incomplete, and it was on pit road
    assert!(!laps[2].complete);
    assert!(laps[2].on_pit_road);
    assert!(laps.iter().all(|lap| !lap.rows.contains(&glitch_at)));
}

#[test]
fn a_reset_inside_a_lap_makes_it_incomplete() {
    let mut rows: Vec<Row> = drive(3, 100.0, 0.5);
    // in lap 2, the car jumps back 0.1 of the lap (a reset), at constant lap number
    let lap_2: Vec<usize> = (0..rows.len()).filter(|i| rows[*i].lap == 2).collect();
    let at: usize = lap_2[lap_2.len() / 2];
    for row in &mut rows[at..] {
        if row.lap == 2 {
            row.pct = (row.pct - 0.1).max(0.0);
        }
    }
    let (_, _, laps) = read(&rows);
    assert!(laps[1].complete);
    assert!(!laps[2].complete);
}

#[test]
fn off_track_rows_are_left_out_and_break_the_lap() {
    let mut rows: Vec<Row> = drive(3, 100.0, 0.5);
    let lap_2: Vec<usize> = (0..rows.len()).filter(|i| rows[*i].lap == 2).collect();
    for i in &lap_2[100..160] {
        rows[*i].on_track = false;
    }
    let (_, _, laps) = read(&rows);
    assert!(!laps[2].complete);
    assert_eq!(laps[2].rows.len(), lap_2.len() - 60);
    assert!(laps[1].complete);
}

// ---------- the real files in data/ (git ignored, skipped where they're missing) ----------

fn real_file(name: &str) -> Option<PathBuf> {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join(name);
    if path.exists() {
        Some(path)
    } else {
        eprintln!("skipped: {} not found", path.display());
        None
    }
}

// iRacing writes the time of the lap just finished into LapLastLapTime a little later:
// the value in the last row of the next lap is that lap's official time
fn iracing_lap_times(file: &IbtFile, laps: &[Lap]) -> Vec<(u16, f64)> {
    let last_lap_time: Vec<f64> = file.column_f64("LapLastLapTime").unwrap();
    laps.windows(2)
        .filter(|pair| pair[0].complete)
        .map(|pair| (pair[0].number, last_lap_time[*pair[1].rows.last().unwrap()]))
        .collect()
}

fn check_real_file(name: &str, expected_complete: usize, car: &str) {
    let Some(path) = real_file(name) else { return };
    let file: IbtFile = IbtFile::open(&path).unwrap();
    let info: SessionInfo = SessionInfo::read(&file).unwrap();
    let channels: LapChannels = LapChannels::read(&file).unwrap();
    let laps: Vec<Lap> = split_laps(&channels);

    assert_eq!(info.track_id, 523);
    assert_eq!(info.car, car);
    assert_eq!(info.iracing_splits.len(), 4);
    assert_eq!(
        laps.iter().filter(|lap| lap.complete).count(),
        expected_complete
    );

    // our lap times match iRacing's own to within a few ms
    for (number, official) in iracing_lap_times(&file, &laps) {
        let ours: f64 = laps
            .iter()
            .find(|lap| lap.number == number)
            .unwrap()
            .lap_time()
            .unwrap();
        assert!(
            (ours - official).abs() < 0.0005,
            "lap {number}: ours {ours:.4} vs iRacing {official:.4}"
        );
    }

    // sectors from the track table: official F1 lines at Spa, they add up to the lap
    let sectors = track::key_sectors(info.track_id, &info.iracing_splits);
    assert_eq!(sectors.source, SectorSource::F1);
    for lap in laps.iter().filter(|lap| lap.complete) {
        let times: Vec<f64> = sector_times(&channels, lap, &sectors.lines).unwrap();
        assert_eq!(times.len(), 3);
        let total: f64 = times.iter().sum();
        assert!((total - lap.lap_time().unwrap()).abs() < 1e-9);
        assert!(times.iter().all(|time| *time > 10.0));
    }
}

#[test]
fn real_w13_spa() {
    check_real_file(
        "mercedesw13_spa 2024 up 2026-11-16 14-51-59.ibt",
        2,
        "Mercedes-AMG W13 E Performance",
    );
}

#[test]
fn real_mustang_gt3_spa() {
    check_real_file(
        "fordmustanggt3_spa 2024 up 2026-04-10 22-33-08.ibt",
        3,
        "Ford Mustang GT3",
    );
}
