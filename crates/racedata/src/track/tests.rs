use super::*;

#[test]
fn the_track_table_is_valid() {
    let tracks: Vec<Track> = tracks();
    assert!(!tracks.is_empty());
    for (i, track) in tracks.iter().enumerate() {
        assert!(!track.iracing_track_ids.is_empty(), "{}", track.name);
        // lines strictly increasing, inside the lap
        let mut previous: f64 = 0.0;
        for line in &track.f1_sector_lines {
            assert!(*line > previous && *line < 1.0, "{}: {line}", track.name);
            previous = *line;
        }
        // an iRacing track belongs to one entry only
        for other in &tracks[..i] {
            for id in &track.iracing_track_ids {
                assert!(!other.iracing_track_ids.contains(id), "track id {id} twice");
            }
        }
    }
}

#[test]
fn f1_tracks_use_the_official_lines() {
    let sectors: SectorLines = key_sectors(523, &[0.0, 0.313076, 0.522864, 0.723114]);
    assert_eq!(sectors.source, SectorSource::F1);
    assert_eq!(sectors.lines, vec![0.3231, 0.7244]);
    assert_eq!(sectors.sector_count(), 3);
}

#[test]
fn other_tracks_use_iracing_splits() {
    // Daytona road course: not in the table
    let sectors: SectorLines = key_sectors(192, &[0.0, 0.25, 0.6]);
    assert_eq!(sectors.source, SectorSource::IRacing);
    assert_eq!(sectors.lines, vec![0.25, 0.6]);
}

#[test]
fn sector_of_a_lap_fraction() {
    let sectors: SectorLines = key_sectors(523, &[]);
    assert_eq!(sectors.sector_of(0.0), 1);
    assert_eq!(sectors.sector_of(0.3230), 1);
    // a line belongs to the sector it starts
    assert_eq!(sectors.sector_of(0.3231), 2);
    assert_eq!(sectors.sector_of(0.7243), 2);
    assert_eq!(sectors.sector_of(0.9999), 3);
}
