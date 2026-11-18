// the .ibt container: header, channel descriptions, session info text, fixed-size rows.
// layout (irsdk, little endian), see md/IBT.md:
//   0    header, 112 bytes: version, tick rate, session info offset / length, channel count,
//        channel header offset, row length, first row offset (in the first var buffer)
//   112  disk sub header, 32 bytes: session start date (unix seconds), start / end time,
//        lap count, record count
//   ...  channel headers, 144 bytes each
//   ...  session info, YAML in Latin-1
//   ...  rows
use std::path::Path;

use super::IbtError;

const HEADER_LEN: usize = 112;
const DISK_HEADER_LEN: usize = 32;
const VAR_HEADER_LEN: usize = 144;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarType {
    Char,
    Bool,
    Int,
    Bitfield,
    Float,
    Double,
}

impl VarType {
    fn from_code(code: i32) -> Option<VarType> {
        match code {
            0 => Some(VarType::Char),
            1 => Some(VarType::Bool),
            2 => Some(VarType::Int),
            3 => Some(VarType::Bitfield),
            4 => Some(VarType::Float),
            5 => Some(VarType::Double),
            _ => None,
        }
    }

    pub fn width(self) -> usize {
        match self {
            VarType::Char | VarType::Bool => 1,
            VarType::Int | VarType::Bitfield | VarType::Float => 4,
            VarType::Double => 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Var {
    pub name: String,
    pub var_type: VarType,
    // byte offset inside a row
    pub offset: usize,
    // values per row (1 for every channel we read)
    pub count: usize,
    pub unit: String,
}

pub struct IbtFile {
    data: Vec<u8>,
    pub tick_rate: u32,
    // session start, unix seconds (the sim session id)
    pub start_date: i64,
    pub vars: Vec<Var>,
    pub session_info: String,
    row_len: usize,
    first_row: usize,
    pub rows: usize,
}

impl IbtFile {
    pub fn open(path: &Path) -> Result<IbtFile, IbtError> {
        IbtFile::parse(std::fs::read(path)?)
    }

    // every offset and length is checked against the file before it's used
    pub fn parse(data: Vec<u8>) -> Result<IbtFile, IbtError> {
        if data.len() < HEADER_LEN + DISK_HEADER_LEN {
            return Err(IbtError::TooShort);
        }
        let int = |at: usize| i32::from_le_bytes(data[at..at + 4].try_into().unwrap());
        let tick_rate: i32 = int(8);
        let session_info_len: i32 = int(16);
        let session_info_offset: i32 = int(20);
        let num_vars: i32 = int(24);
        let var_header_offset: i32 = int(28);
        let row_len: i32 = int(36);
        // first var buffer: tick count at 48, row offset at 52
        let first_row: i32 = int(52);
        let start_date: i64 = i64::from_le_bytes(data[112..120].try_into().unwrap());
        let record_count: i32 = int(140);

        let to_usize = |value: i32, field: &'static str| -> Result<usize, IbtError> {
            usize::try_from(value).map_err(|_| IbtError::BadHeader(field))
        };
        let tick_rate: usize = to_usize(tick_rate, "tick rate")?;
        let session_info_len: usize = to_usize(session_info_len, "session info length")?;
        let session_info_offset: usize = to_usize(session_info_offset, "session info offset")?;
        let num_vars: usize = to_usize(num_vars, "channel count")?;
        let var_header_offset: usize = to_usize(var_header_offset, "channel header offset")?;
        let row_len: usize = to_usize(row_len, "row length")?;
        let first_row: usize = to_usize(first_row, "first row offset")?;
        if tick_rate == 0 {
            return Err(IbtError::BadHeader("tick rate"));
        }
        if row_len == 0 {
            return Err(IbtError::BadHeader("row length"));
        }

        let vars_end: usize = num_vars
            .checked_mul(VAR_HEADER_LEN)
            .and_then(|len| len.checked_add(var_header_offset))
            .ok_or(IbtError::BadHeader("channel count"))?;
        let info_end: usize = session_info_offset
            .checked_add(session_info_len)
            .ok_or(IbtError::BadHeader("session info length"))?;
        if vars_end > data.len() || info_end > data.len() || first_row > data.len() {
            return Err(IbtError::TooShort);
        }

        let mut vars: Vec<Var> = Vec::with_capacity(num_vars);
        for i in 0..num_vars {
            let at: usize = var_header_offset + i * VAR_HEADER_LEN;
            let code: i32 = int(at);
            let var_type: VarType =
                VarType::from_code(code).ok_or(IbtError::BadHeader("channel type"))?;
            let offset: usize = to_usize(int(at + 4), "channel offset")?;
            let count: usize = to_usize(int(at + 8), "channel count")?;
            if offset + var_type.width() * count > row_len {
                return Err(IbtError::BadHeader("channel outside the row"));
            }
            vars.push(Var {
                name: text(&data[at + 16..at + 48]),
                var_type,
                offset,
                count,
                unit: text(&data[at + 112..at + 144]),
            });
        }

        // the text ends at the first zero byte, the rest of the block is padding
        let session_info: String = text(&data[session_info_offset..info_end]);

        // whole rows only: a file cut off mid-row (sim crash) keeps every complete row
        let mut rows: usize = (data.len() - first_row) / row_len;
        if record_count > 0 {
            rows = rows.min(record_count as usize);
        }

        Ok(IbtFile {
            data,
            tick_rate: tick_rate as u32,
            start_date,
            vars,
            session_info,
            row_len,
            first_row,
            rows,
        })
    }

    pub fn var(&self, name: &str) -> Option<&Var> {
        self.vars.iter().find(|var| var.name == name)
    }

    // every row's value of a single-value channel, as f64 (bools are 0 / 1). None if the
    // channel isn't in this file
    pub fn column_f64(&self, name: &str) -> Option<Vec<f64>> {
        let var: &Var = self.var(name)?;
        Some((0..self.rows).map(|row| self.value(var, row)).collect())
    }

    pub fn column_i32(&self, name: &str) -> Option<Vec<i32>> {
        let var: &Var = self.var(name)?;
        Some(
            (0..self.rows)
                .map(|row| self.value(var, row) as i32)
                .collect(),
        )
    }

    pub fn column_bool(&self, name: &str) -> Option<Vec<bool>> {
        let var: &Var = self.var(name)?;
        Some(
            (0..self.rows)
                .map(|row| self.value(var, row) != 0.0)
                .collect(),
        )
    }

    fn value(&self, var: &Var, row: usize) -> f64 {
        let at: usize = self.first_row + row * self.row_len + var.offset;
        let bytes: &[u8] = &self.data[at..at + var.var_type.width()];
        match var.var_type {
            VarType::Char | VarType::Bool => bytes[0] as f64,
            VarType::Int => i32::from_le_bytes(bytes.try_into().unwrap()) as f64,
            VarType::Bitfield => u32::from_le_bytes(bytes.try_into().unwrap()) as f64,
            VarType::Float => f32::from_le_bytes(bytes.try_into().unwrap()) as f64,
            VarType::Double => f64::from_le_bytes(bytes.try_into().unwrap()),
        }
    }
}

// zero-terminated Latin-1 text (iRacing's session info says Encoding: ISO_8859_1).
// every Latin-1 byte is the unicode code point with the same number
fn text(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as char)
        .collect()
}
