//! prints an .ibt file's session, laps, and sector times: the official F1 lines (or iRacing's
//! own on non-F1 tracks) and iRacing's split points, side by side.
//!
//! run with: cargo run --example ibt_laps -- data/<file>.ibt [more files]

use std::path::Path;

use lsm_kv::ibt::{IbtFile, Lap, LapChannels, SessionInfo, sector_times, split_laps};
use lsm_kv::track::{self, SectorSource};

fn main() {
    for path in std::env::args().skip(1) {
        if let Err(error) = show(Path::new(&path)) {
            eprintln!("{path}: {error:?}");
        }
    }
}

fn show(path: &Path) -> Result<(), lsm_kv::ibt::IbtError> {
    let file: IbtFile = IbtFile::open(path)?;
    let info: SessionInfo = SessionInfo::read(&file)?;
    let channels: LapChannels = LapChannels::read(&file)?;
    let laps: Vec<Lap> = split_laps(&channels);
    let key_sectors = track::key_sectors(info.track_id, &info.iracing_splits);
    let iracing_lines: Vec<f64> = info.iracing_splits[1..].to_vec();

    println!("{}", path.display());
    println!(
        "  {} ({}), track id {}, {:.0} m. car: {}. {} rows at {} Hz",
        info.track_display_name,
        info.track_config,
        info.track_id,
        info.track_length_m,
        info.car,
        file.rows,
        info.tick_rate
    );
    let source: &str = match key_sectors.source {
        SectorSource::F1 => "official F1",
        SectorSource::IRacing => "iRacing (not an F1 track in tracks.toml)",
    };
    println!("  key sectors: {source}, lines {:?}", key_sectors.lines);
    println!("  iRacing splits: {:?}", iracing_lines);

    for lap in &laps {
        let state: &str = match (lap.complete, lap.on_pit_road) {
            (true, false) => "complete",
            (true, true) => "complete, pit lap",
            (false, _) => "incomplete",
        };
        print!(
            "  lap {:>2}: {:>6} rows, {state}",
            lap.number,
            lap.rows.len()
        );
        if let Some(time) = lap.lap_time() {
            print!(", {}", clock(time));
        }
        println!();
        if let Some(times) = sector_times(&channels, lap, &key_sectors.lines) {
            println!("      key sectors:  {}", list(&times));
        }
        if let Some(times) = sector_times(&channels, lap, &iracing_lines) {
            println!("      iRacing:      {}", list(&times));
        }
    }
    Ok(())
}

fn clock(seconds: f64) -> String {
    format!("{}:{:06.3}", (seconds / 60.0) as u64, seconds % 60.0)
}

fn list(times: &[f64]) -> String {
    times
        .iter()
        .enumerate()
        .map(|(i, time)| format!("S{} {:.3}", i + 1, time))
        .collect::<Vec<_>>()
        .join("  ")
}
