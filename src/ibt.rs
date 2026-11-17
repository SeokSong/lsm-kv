// reads iRacing .ibt telemetry files: the user's own car only, split into laps, with lap and
// sector times measured at the lines. md/IBT.md has the format and what the real files show
mod file;
mod laps;
mod session;
mod yaml;

pub use file::{IbtFile, Var, VarType};
pub use laps::{Lap, LapChannels, crossing_time, sector_times, split_laps};
pub use session::SessionInfo;
pub use yaml::{Node, parse as parse_yaml};

#[derive(Debug)]
pub enum IbtError {
    Io(std::io::Error),
    // shorter than the headers, or than a section the header points to
    TooShort,
    // a header field that can't be right (negative, zero, out of range)
    BadHeader(&'static str),
    // a session info field we need isn't there
    MissingSessionInfo(&'static str),
    // a channel we need isn't in the file
    MissingChannel(&'static str),
}

impl From<std::io::Error> for IbtError {
    fn from(error: std::io::Error) -> IbtError {
        IbtError::Io(error)
    }
}

#[cfg(test)]
mod tests;
