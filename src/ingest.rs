enum Key {
    Telemetry(Chunk),
    Position(Chunk),
    SessionMetaData { session_id: u32 },
    DriverMap { session_id: u32, car_idx: u16 },
}

struct Chunk {
    session_id: u32,
    car_idx: u16,
    lap: u16,
    sector: u8,
    sequence: u16,
}
