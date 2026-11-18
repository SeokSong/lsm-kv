// a value holding a run of samples as columns (structure of arrays). layout, little endian:
// | header_len u16 | version u16 | start u64 | end u64 | count u16 | column_count u16 |
// | column_count x (id u16, type u8) | column 0's rows | column 1's rows | ... |
//
// start(c)    = header_len + count x (w0 + w1 + ... + w(c-1))
// sample(c,i) = start(c) + i x w(c)
//
// self describing: the header lists its columns, so any car or source can carry its own
// channels. md/DESIGN.md (section 6) has the format and the reasons
mod catalog;
mod error;

use lsm_kv::wal::MAX_VALUE_LEN;

pub use catalog::{
    ALT, BRAKE, CATALOG, Channel, ColumnType, DRS, GEAR, LAP_DIST, LAP_DIST_PCT, LAT, LON, RPM,
    SPEED, STEERING, THROTTLE, TIME, X, Y, Z, lookup,
};
pub use error::ChunkError;

pub const VERSION: u16 = 1;
// header_len + version + start + end + count + column_count
pub const FIXED_HEADER_LEN: usize = 2 + 2 + 8 + 8 + 2 + 2;
// id + type per column
pub const COLUMN_DESC_LEN: usize = 2 + 1;
// writers put at most this many rows in a chunk, fewer when rows are wide (max_rows)
pub const MAX_ROWS: usize = 512;

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnData {
    U8(Vec<u8>),
    I8(Vec<i8>),
    U16(Vec<u16>),
    U32(Vec<u32>),
    I32(Vec<i32>),
    F32(Vec<f32>),
    F64(Vec<f64>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub id: u16,
    pub data: ColumnData,
}

// a checked chunk: new and decode refuse anything that breaks the format, so every Chunk
// encodes to a valid value
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    start_us: u64,
    columns: Vec<Column>,
}

impl Chunk {
    // start_us: session time of the first row. columns must include TIME (u32 offsets from
    // start_us, starting at 0, never going backwards), all the same length
    pub fn new(start_us: u64, columns: Vec<Column>) -> Result<Chunk, ChunkError> {
        let chunk: Chunk = Chunk { start_us, columns };
        chunk.validate()?;
        Ok(chunk)
    }

    pub fn start_us(&self) -> u64 {
        self.start_us
    }

    // session time of the last row
    pub fn end_us(&self) -> u64 {
        self.start_us + *self.time().last().unwrap() as u64
    }

    pub fn row_count(&self) -> usize {
        self.columns[0].data.len()
    }

    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    pub fn column(&self, id: u16) -> Option<&ColumnData> {
        self.columns
            .iter()
            .find(|column| column.id == id)
            .map(|column| &column.data)
    }

    // offsets from start_us, in microseconds
    pub fn time(&self) -> &[u32] {
        match self.column(TIME) {
            Some(ColumnData::U32(time)) => time,
            // validate guarantees a u32 time column
            _ => unreachable!("chunk without a u32 time column"),
        }
    }

    pub fn encoded_len(&self) -> usize {
        header_len(self.columns.len()) + self.row_count() * row_bytes(&self.column_types())
    }

    pub fn encode(&self) -> Vec<u8> {
        let header_len: usize = header_len(self.columns.len());
        let mut out: Vec<u8> = Vec::with_capacity(self.encoded_len());
        out.extend_from_slice(&(header_len as u16).to_le_bytes());
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&self.start_us.to_le_bytes());
        out.extend_from_slice(&self.end_us().to_le_bytes());
        out.extend_from_slice(&(self.row_count() as u16).to_le_bytes());
        out.extend_from_slice(&(self.columns.len() as u16).to_le_bytes());
        for column in &self.columns {
            out.extend_from_slice(&column.id.to_le_bytes());
            out.push(column.data.column_type().byte());
        }
        for column in &self.columns {
            column.data.write_to(&mut out);
        }
        debug_assert_eq!(out.len(), self.encoded_len());
        out
    }

    // never panics on any input: every length is checked before it's used
    pub fn decode(bytes: &[u8]) -> Result<Chunk, ChunkError> {
        if bytes.len() < FIXED_HEADER_LEN {
            return Err(ChunkError::TooShort);
        }
        let header_len: usize = u16_at(bytes, 0) as usize;
        let version: u16 = u16_at(bytes, 2);
        if version != VERSION {
            return Err(ChunkError::UnsupportedVersion(version));
        }
        let start_us: u64 = u64_at(bytes, 4);
        let end_us: u64 = u64_at(bytes, 12);
        let count: usize = u16_at(bytes, 20) as usize;
        let column_count: usize = u16_at(bytes, 22) as usize;

        // a newer version may add header fields: header_len can be bigger, never smaller
        if header_len < self::header_len(column_count) {
            return Err(ChunkError::BadHeaderLen);
        }
        if bytes.len() < header_len {
            return Err(ChunkError::TooShort);
        }

        let mut descriptors: Vec<(u16, ColumnType)> = Vec::with_capacity(column_count);
        for i in 0..column_count {
            let at: usize = FIXED_HEADER_LEN + i * COLUMN_DESC_LEN;
            let type_byte: u8 = bytes[at + 2];
            let column_type: ColumnType =
                ColumnType::from_byte(type_byte).ok_or(ChunkError::UnknownType(type_byte))?;
            descriptors.push((u16_at(bytes, at), column_type));
        }

        let types: Vec<ColumnType> = descriptors
            .iter()
            .map(|(_, column_type)| *column_type)
            .collect();
        // at most 65,535 rows x 65,535 columns x 8 bytes: fits in usize, no overflow
        let expected: usize = header_len + count * row_bytes(&types);
        if bytes.len() < expected {
            return Err(ChunkError::TooShort);
        }
        if bytes.len() > expected {
            return Err(ChunkError::LengthMismatch);
        }

        let mut columns: Vec<Column> = Vec::with_capacity(column_count);
        let mut at: usize = header_len;
        for (id, column_type) in descriptors {
            let len: usize = count * column_type.width();
            columns.push(Column {
                id,
                data: ColumnData::read(column_type, &bytes[at..at + len]),
            });
            at += len;
        }

        let chunk: Chunk = Chunk::new(start_us, columns)?;
        if chunk.end_us() != end_us {
            return Err(ChunkError::EndMismatch);
        }
        Ok(chunk)
    }

    fn column_types(&self) -> Vec<ColumnType> {
        self.columns
            .iter()
            .map(|column| column.data.column_type())
            .collect()
    }

    fn validate(&self) -> Result<(), ChunkError> {
        if self.columns.is_empty() {
            return Err(ChunkError::NoColumns);
        }
        for (i, column) in self.columns.iter().enumerate() {
            if self.columns[..i].iter().any(|other| other.id == column.id) {
                return Err(ChunkError::DuplicateColumn(column.id));
            }
            // unknown ids are kept as they are (a newer app's channels), known ones must match
            if let Some(channel) = lookup(column.id)
                && channel.column_type != column.data.column_type()
            {
                return Err(ChunkError::WrongType(column.id));
            }
        }

        let rows: usize = self.columns[0].data.len();
        if self.columns.iter().any(|column| column.data.len() != rows) {
            return Err(ChunkError::LengthMismatch);
        }
        if rows == 0 || rows > MAX_ROWS {
            return Err(ChunkError::BadRowCount(rows));
        }

        let time: &[u32] = match self.column(TIME) {
            Some(ColumnData::U32(time)) => time,
            _ => return Err(ChunkError::MissingTime),
        };
        if time[0] != 0 {
            return Err(ChunkError::TimeNotFromZero);
        }
        if time.windows(2).any(|pair| pair[1] < pair[0]) {
            return Err(ChunkError::TimeGoesBackwards);
        }
        if self
            .start_us
            .checked_add(*time.last().unwrap() as u64)
            .is_none()
        {
            return Err(ChunkError::EndMismatch);
        }

        let len: usize = self.encoded_len();
        if len > MAX_VALUE_LEN {
            return Err(ChunkError::TooLarge(len));
        }
        Ok(())
    }
}

impl ColumnData {
    pub fn column_type(&self) -> ColumnType {
        match self {
            ColumnData::U8(_) => ColumnType::U8,
            ColumnData::I8(_) => ColumnType::I8,
            ColumnData::U16(_) => ColumnType::U16,
            ColumnData::U32(_) => ColumnType::U32,
            ColumnData::I32(_) => ColumnType::I32,
            ColumnData::F32(_) => ColumnType::F32,
            ColumnData::F64(_) => ColumnType::F64,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            ColumnData::U8(values) => values.len(),
            ColumnData::I8(values) => values.len(),
            ColumnData::U16(values) => values.len(),
            ColumnData::U32(values) => values.len(),
            ColumnData::I32(values) => values.len(),
            ColumnData::F32(values) => values.len(),
            ColumnData::F64(values) => values.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn write_to(&self, out: &mut Vec<u8>) {
        match self {
            ColumnData::U8(values) => out.extend_from_slice(values),
            ColumnData::I8(values) => out.extend(values.iter().map(|v| *v as u8)),
            ColumnData::U16(values) => values
                .iter()
                .for_each(|v| out.extend_from_slice(&v.to_le_bytes())),
            ColumnData::U32(values) => values
                .iter()
                .for_each(|v| out.extend_from_slice(&v.to_le_bytes())),
            ColumnData::I32(values) => values
                .iter()
                .for_each(|v| out.extend_from_slice(&v.to_le_bytes())),
            ColumnData::F32(values) => values
                .iter()
                .for_each(|v| out.extend_from_slice(&v.to_le_bytes())),
            ColumnData::F64(values) => values
                .iter()
                .for_each(|v| out.extend_from_slice(&v.to_le_bytes())),
        }
    }

    // bytes is exactly rows x width long (decode checked it)
    fn read(column_type: ColumnType, bytes: &[u8]) -> ColumnData {
        match column_type {
            ColumnType::U8 => ColumnData::U8(bytes.to_vec()),
            ColumnType::I8 => ColumnData::I8(bytes.iter().map(|b| *b as i8).collect()),
            ColumnType::U16 => ColumnData::U16(read_le(bytes, u16::from_le_bytes)),
            ColumnType::U32 => ColumnData::U32(read_le(bytes, u32::from_le_bytes)),
            ColumnType::I32 => ColumnData::I32(read_le(bytes, i32::from_le_bytes)),
            ColumnType::F32 => ColumnData::F32(read_le(bytes, f32::from_le_bytes)),
            ColumnType::F64 => ColumnData::F64(read_le(bytes, f64::from_le_bytes)),
        }
    }
}

// splits bytes into N-byte values. as_chunks gives fixed-size arrays, so no length checks
// can fail. bytes is always a whole number of values (decode checked the length)
fn read_le<T, const N: usize>(bytes: &[u8], from_le_bytes: fn([u8; N]) -> T) -> Vec<T> {
    let (values, rest): (&[[u8; N]], &[u8]) = bytes.as_chunks::<N>();
    debug_assert!(rest.is_empty());
    values.iter().map(|value| from_le_bytes(*value)).collect()
}

// how many rows of these columns fit in one chunk: MAX_ROWS, or fewer if a row is wide
pub fn max_rows(types: &[ColumnType]) -> usize {
    let row: usize = row_bytes(types).max(1);
    MAX_ROWS.min(MAX_VALUE_LEN.saturating_sub(header_len(types.len())) / row)
}

fn header_len(column_count: usize) -> usize {
    FIXED_HEADER_LEN + column_count * COLUMN_DESC_LEN
}

fn row_bytes(types: &[ColumnType]) -> usize {
    types.iter().map(|column_type| column_type.width()).sum()
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

#[cfg(test)]
mod tests;
