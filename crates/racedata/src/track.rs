// the track table (tracks/tracks.toml, built into the program): which iRacing tracks and
// OpenF1 circuits are the same place, and where their sectors start. md/TRACKS.md
use serde::Deserialize;

const TRACKS_TOML: &str = include_str!("../tracks/tracks.toml");

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub name: String,
    pub iracing_track_ids: Vec<u32>,
    pub openf1_circuit_key: Option<u32>,
    // where sector 2, 3, ... start, lap fractions. official F1 lines
    pub f1_sector_lines: Vec<f64>,
}

#[derive(Debug, Deserialize)]
struct TrackFile {
    track: Vec<Track>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectorSource {
    // the official F1 lines from the track table
    F1,
    // the split points in the iRacing file: the track isn't an F1 track (in our table)
    IRacing,
}

// the lines a session's chunks are keyed by, and where they came from
#[derive(Debug, Clone, PartialEq)]
pub struct SectorLines {
    // where sector 2, 3, ... start. sector 1 starts at 0
    pub lines: Vec<f64>,
    pub source: SectorSource,
}

impl SectorLines {
    pub fn sector_count(&self) -> usize {
        self.lines.len() + 1
    }

    // 1-based sector of a lap fraction
    pub fn sector_of(&self, lap_dist_pct: f64) -> u8 {
        (self
            .lines
            .iter()
            .filter(|line| lap_dist_pct >= **line)
            .count()
            + 1) as u8
    }
}

pub fn tracks() -> Vec<Track> {
    let file: TrackFile =
        toml::from_str(TRACKS_TOML).expect("tracks/tracks.toml is checked by a test");
    file.track
}

pub fn by_iracing_id(track_id: u32) -> Option<Track> {
    tracks()
        .into_iter()
        .find(|track| track.iracing_track_ids.contains(&track_id))
}

// official F1 sectors on F1 tracks, iRacing's own splits everywhere else (DESIGN.md)
pub fn key_sectors(iracing_track_id: u32, iracing_splits: &[f64]) -> SectorLines {
    match by_iracing_id(iracing_track_id) {
        Some(track) => SectorLines {
            lines: track.f1_sector_lines,
            source: SectorSource::F1,
        },
        None => SectorLines {
            lines: iracing_splits
                .iter()
                .copied()
                .filter(|line| *line > 0.0)
                .collect(),
            source: SectorSource::IRacing,
        },
    }
}

#[cfg(test)]
mod tests;
