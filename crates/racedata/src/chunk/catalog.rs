// the channel catalog: every column id a chunk can hold, with its type and unit.
// ids are part of the on-disk format: never change or reuse one, only add new ones.
// 1-19 telemetry stream, 20-39 position stream, 40+ car specific (ABS, TC, ERS, ...)

// how a column's values are stored. the byte is what's written in the chunk header
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    U8,
    I8,
    U16,
    U32,
    I32,
    F32,
    F64,
}

impl ColumnType {
    pub fn byte(self) -> u8 {
        match self {
            ColumnType::U8 => 1,
            ColumnType::I8 => 2,
            ColumnType::U16 => 3,
            ColumnType::U32 => 4,
            ColumnType::I32 => 5,
            ColumnType::F32 => 6,
            ColumnType::F64 => 7,
        }
    }

    pub fn from_byte(byte: u8) -> Option<ColumnType> {
        match byte {
            1 => Some(ColumnType::U8),
            2 => Some(ColumnType::I8),
            3 => Some(ColumnType::U16),
            4 => Some(ColumnType::U32),
            5 => Some(ColumnType::I32),
            6 => Some(ColumnType::F32),
            7 => Some(ColumnType::F64),
            _ => None,
        }
    }

    // bytes per value
    pub fn width(self) -> usize {
        match self {
            ColumnType::U8 | ColumnType::I8 => 1,
            ColumnType::U16 => 2,
            ColumnType::U32 | ColumnType::I32 | ColumnType::F32 => 4,
            ColumnType::F64 => 8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Channel {
    pub id: u16,
    pub name: &'static str,
    pub column_type: ColumnType,
    pub unit: &'static str,
}

pub const TIME: u16 = 1;
pub const LAP_DIST: u16 = 2;
pub const LAP_DIST_PCT: u16 = 3;
pub const SPEED: u16 = 4;
pub const THROTTLE: u16 = 5;
pub const BRAKE: u16 = 6;
pub const STEERING: u16 = 7;
pub const GEAR: u16 = 8;
pub const RPM: u16 = 9;
pub const DRS: u16 = 10;
pub const LAT: u16 = 20;
pub const LON: u16 = 21;
pub const ALT: u16 = 22;
pub const X: u16 = 23;
pub const Y: u16 = 24;
pub const Z: u16 = 25;

pub const CATALOG: &[Channel] = &[
    channel(TIME, "time", ColumnType::U32, "us since chunk start"),
    channel(LAP_DIST, "lap_dist", ColumnType::F32, "m"),
    channel(LAP_DIST_PCT, "lap_dist_pct", ColumnType::F32, "0..1"),
    channel(SPEED, "speed", ColumnType::F32, "m/s"),
    channel(THROTTLE, "throttle", ColumnType::F32, "0..1"),
    channel(BRAKE, "brake", ColumnType::F32, "0..1"),
    channel(STEERING, "steering", ColumnType::F32, "rad"),
    channel(GEAR, "gear", ColumnType::I8, "-1 reverse, 0 neutral"),
    channel(RPM, "rpm", ColumnType::F32, "rpm"),
    channel(DRS, "drs", ColumnType::U8, "source value"),
    // f64: f32 rounds to ~0.5 m at Spa's latitude, too coarse next to a 2 m racing line gap
    channel(LAT, "lat", ColumnType::F64, "degrees"),
    channel(LON, "lon", ColumnType::F64, "degrees"),
    channel(ALT, "alt", ColumnType::F32, "m"),
    channel(X, "x", ColumnType::F32, "source units"),
    channel(Y, "y", ColumnType::F32, "source units"),
    channel(Z, "z", ColumnType::F32, "source units"),
];

const fn channel(
    id: u16,
    name: &'static str,
    column_type: ColumnType,
    unit: &'static str,
) -> Channel {
    Channel {
        id,
        name,
        column_type,
        unit,
    }
}

// None for an id this version doesn't know (a newer app's export): readers keep it as is
pub fn lookup(id: u16) -> Option<&'static Channel> {
    CATALOG.iter().find(|channel| channel.id == id)
}
