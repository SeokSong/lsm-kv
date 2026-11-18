// splits a session's rows into laps, and measures lap and sector times at the lines.
//
// what the real files show (md/IBT.md):
// - `Lap` goes up in the same row where LapDistPct wraps from ~1.0 to ~0.0
// - lap 0 is the out lap from the pit box, the last lap ends when the driver leaves the car
// - leaving the car writes one glitch row (Lap 0, LapDistPct 0) before the car shows up at
//   the pit box: rows whose Lap is below the highest lap so far are dropped
//
// - iRacing's timing line is ~0.4-0.8 m before the point where LapDistPct wraps, so an
//   interpolated crossing is off by a few ms. but SessionTime - LapCurrentLapTime is iRacing's
//   own lap start, constant to ~10 us across a lap, and the starts subtract to LapLastLapTime
//   exactly. that's what lap times use, the interpolated crossing is only the fallback
//
// a lap is complete when it starts and ends with a line crossing and the car never jumps
// (a reset or tow). only complete laps get lap and sector times
use super::IbtError;
use super::file::IbtFile;

#[derive(Debug, Clone, PartialEq)]
pub struct Lap {
    // iRacing's lap number, 0 is the out lap
    pub number: u16,
    // rows of the file in this lap, in order. a dropped glitch row never appears
    pub rows: Vec<usize>,
    pub complete: bool,
    // any row on pit road (a pit lap is still a complete lap)
    pub on_pit_road: bool,
    // session time at the line: iRacing's own lap clock, or the interpolated crossing
    pub start_time: Option<f64>,
    pub end_time: Option<f64>,
}

impl Lap {
    pub fn lap_time(&self) -> Option<f64> {
        Some(self.end_time? - self.start_time?)
    }
}

// the channels lap splitting needs, one value per row
pub struct LapChannels {
    pub time: Vec<f64>,
    pub lap: Vec<i32>,
    pub pct: Vec<f64>,
    pub on_track: Vec<bool>,
    pub on_pit_road: Vec<bool>,
    // iRacing's lap clock (LapCurrentLapTime), None if the file doesn't have it
    pub lap_clock: Option<Vec<f64>>,
}

impl LapChannels {
    pub fn read(file: &IbtFile) -> Result<LapChannels, IbtError> {
        let need = |name: &'static str| IbtError::MissingChannel(name);
        Ok(LapChannels {
            time: file.column_f64("SessionTime").ok_or(need("SessionTime"))?,
            lap: file.column_i32("Lap").ok_or(need("Lap"))?,
            pct: file.column_f64("LapDistPct").ok_or(need("LapDistPct"))?,
            on_track: file.column_bool("IsOnTrack").ok_or(need("IsOnTrack"))?,
            // older files may not have it: no pit road information then
            on_pit_road: file
                .column_bool("OnPitRoad")
                .unwrap_or_else(|| vec![false; file.rows]),
            lap_clock: file.column_f64("LapCurrentLapTime"),
        })
    }
}

// a lap fraction jump bigger than this between two rows is a reset, not driving:
// 0.02 of Spa is ~140 m in 1/60 s
const MAX_FORWARD_JUMP: f64 = 0.02;
// small backward steps happen (iRacing's track position wobbles), bigger ones are a reset
const MAX_BACKWARD_STEP: f64 = 0.005;
// iRacing's lap start must be this close to the crossing to be trusted (it's ~10 ms off)
const MAX_CLOCK_OFFSET: f64 = 0.1;

pub fn split_laps(channels: &LapChannels) -> Vec<Lap> {
    // rows kept: on track, and not a glitch row with a lap number below the highest so far
    let mut groups: Vec<Lap> = Vec::new();
    let mut highest: i32 = i32::MIN;
    let mut broken: Vec<bool> = Vec::new();
    for row in 0..channels.time.len() {
        let lap: i32 = channels.lap[row];
        if !channels.on_track[row] || lap < highest || lap < 0 || lap > u16::MAX as i32 {
            continue;
        }
        highest = lap;
        let starts_new: bool = groups.last().is_none_or(|group| group.number as i32 != lap);
        if starts_new {
            groups.push(Lap {
                number: lap as u16,
                rows: Vec::new(),
                complete: false,
                on_pit_road: false,
                start_time: None,
                end_time: None,
            });
            broken.push(false);
        }
        let group: &mut Lap = groups.last_mut().unwrap();
        if let Some(&previous) = group.rows.last() {
            let step: f64 = channels.pct[row] - channels.pct[previous];
            // rows missing inside a lap (off track, dropped) or the car jumped: not clean
            if row != previous + 1 || step > MAX_FORWARD_JUMP || step < -MAX_BACKWARD_STEP {
                *broken.last_mut().unwrap() = true;
            }
        }
        group.on_pit_road |= channels.on_pit_road[row];
        group.rows.push(row);
    }

    // a line crossing between two neighbouring groups: the next lap, the very next row,
    // the fraction wrapping from near 1 to near 0
    let crossing = |before: &Lap, after: &Lap| -> Option<f64> {
        let last: usize = *before.rows.last()?;
        let first: usize = *after.rows.first()?;
        let wraps: bool = channels.pct[last] > 0.9 && channels.pct[first] < 0.1;
        if after.number != before.number + 1 || first != last + 1 || !wraps {
            return None;
        }
        let (p0, p1) = (channels.pct[last], channels.pct[first] + 1.0);
        let (t0, t1) = (channels.time[last], channels.time[first]);
        Some(t0 + (1.0 - p0) / (p1 - p0) * (t1 - t0))
    };

    // when lap i starts: iRacing's clock if it agrees with the crossing, else the crossing
    let start_of = |i: usize| -> Option<f64> {
        let crossed: f64 = crossing(&groups[i.checked_sub(1)?], &groups[i])?;
        match iracing_lap_start(channels, &groups[i]) {
            Some(start) if (start - crossed).abs() < MAX_CLOCK_OFFSET => Some(start),
            _ => Some(crossed),
        }
    };
    let starts: Vec<Option<f64>> = (0..groups.len()).map(start_of).collect();
    // a lap ends where the next one starts
    let ends: Vec<Option<f64>> = (0..groups.len())
        .map(|i| starts.get(i + 1).copied().flatten())
        .collect();
    for (i, group) in groups.iter_mut().enumerate() {
        group.start_time = starts[i];
        group.end_time = ends[i];
        group.complete = starts[i].is_some() && ends[i].is_some() && !broken[i];
    }
    groups
}

// SessionTime - LapCurrentLapTime is the same in every row of a lap: iRacing's lap start.
// the first rows after the line still carry the previous lap's clock, so take the median
fn iracing_lap_start(channels: &LapChannels, lap: &Lap) -> Option<f64> {
    let clock: &[f64] = channels.lap_clock.as_deref()?;
    let mut starts: Vec<f64> = lap
        .rows
        .iter()
        .filter(|row| clock[**row] > 0.0)
        .map(|row| channels.time[*row] - clock[*row])
        .collect();
    if starts.is_empty() {
        return None;
    }
    starts.sort_by(f64::total_cmp);
    Some(starts[starts.len() / 2])
}

// session time when a complete lap crosses a line (a lap fraction between 0 and 1),
// interpolated between the rows either side of it. None if the lap never reaches it
pub fn crossing_time(channels: &LapChannels, lap: &Lap, line: f64) -> Option<f64> {
    lap.rows.windows(2).find_map(|pair| {
        let (a, b) = (pair[0], pair[1]);
        let (pa, pb) = (channels.pct[a], channels.pct[b]);
        if pa < line && line <= pb {
            let (ta, tb) = (channels.time[a], channels.time[b]);
            Some(ta + (line - pa) / (pb - pa) * (tb - ta))
        } else {
            None
        }
    })
}

// one time per sector, for any set of lines (where sector 2, 3, ... start). complete laps only
pub fn sector_times(channels: &LapChannels, lap: &Lap, lines: &[f64]) -> Option<Vec<f64>> {
    if !lap.complete {
        return None;
    }
    let mut boundaries: Vec<f64> = vec![lap.start_time?];
    for line in lines {
        boundaries.push(crossing_time(channels, lap, *line)?);
    }
    boundaries.push(lap.end_time?);
    Some(
        boundaries
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .collect(),
    )
}
