// what we keep from the session info: the track, the user's own car, iRacing's split points.
// nothing about the other drivers is read: the file lists them all (names, ids, ratings),
// they never go into the store or an export (md/IBT.md, privacy)
use super::IbtError;
use super::file::IbtFile;
use super::yaml::{self, Node};

#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    // the sim session id: session start, unix seconds (DESIGN.md section 6)
    pub session_id: u32,
    // the user's iRacing customer id, the driver id in sim keys
    pub driver_id: u32,
    pub track_id: u32,
    // iRacing's internal name, e.g. "spa 2024 up"
    pub track_name: String,
    // e.g. "Circuit de Spa-Francorchamps"
    pub track_display_name: String,
    // e.g. "Grand Prix"
    pub track_config: String,
    pub track_length_m: f64,
    // e.g. "Mercedes-AMG W13 E Performance"
    pub car: String,
    pub car_id: u32,
    // where iRacing's sectors start, as lap fractions, the first is 0
    pub iracing_splits: Vec<f64>,
    pub tick_rate: u32,
}

impl SessionInfo {
    pub fn read(file: &IbtFile) -> Result<SessionInfo, IbtError> {
        let root: Node = yaml::parse(&file.session_info);
        let missing = |field: &'static str| IbtError::MissingSessionInfo(field);
        let number = |path: &'static str| -> Result<u32, IbtError> {
            root.str(path)
                .and_then(|value| value.parse::<u32>().ok())
                .ok_or(missing(path))
        };

        let car_idx: String = root
            .str("DriverInfo.DriverCarIdx")
            .ok_or(missing("DriverInfo.DriverCarIdx"))?
            .to_string();
        // only the user's own entry from the driver list
        let own_car: &Node = root
            .list("DriverInfo.Drivers")
            .iter()
            .find(|driver| driver.str("CarIdx") == Some(car_idx.as_str()))
            .ok_or(missing("DriverInfo.Drivers (own car)"))?;

        // "6.9293 km"
        let track_length_m: f64 = root
            .str("WeekendInfo.TrackLength")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|km| km.parse::<f64>().ok())
            .map(|km| km * 1000.0)
            .ok_or(missing("WeekendInfo.TrackLength"))?;

        let mut iracing_splits: Vec<f64> = root
            .list("SplitTimeInfo.Sectors")
            .iter()
            .filter_map(|sector| sector.str("SectorStartPct")?.parse::<f64>().ok())
            .collect();
        iracing_splits.sort_by(f64::total_cmp);
        if iracing_splits.first() != Some(&0.0) {
            iracing_splits.insert(0, 0.0);
        }

        let session_id: u32 = u32::try_from(file.start_date)
            .map_err(|_| IbtError::BadHeader("session start date"))?;

        Ok(SessionInfo {
            session_id,
            driver_id: number("DriverInfo.DriverUserID")?,
            track_id: number("WeekendInfo.TrackID")?,
            track_name: text(&root, "WeekendInfo.TrackName"),
            track_display_name: text(&root, "WeekendInfo.TrackDisplayName"),
            track_config: text(&root, "WeekendInfo.TrackConfigName"),
            track_length_m,
            car: own_car.str("CarScreenName").unwrap_or_default().to_string(),
            car_id: own_car
                .str("CarID")
                .and_then(|id| id.parse().ok())
                .unwrap_or(0),
            iracing_splits,
            tick_rate: file.tick_rate,
        })
    }
}

fn text(root: &Node, path: &str) -> String {
    root.str(path).unwrap_or_default().to_string()
}
