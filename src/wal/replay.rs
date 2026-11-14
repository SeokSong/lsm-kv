use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::path::Path;

use super::codec::decode;
use super::error::{CorruptionReason, WalError};
use super::record::Record;

// reads every valid record in the WAL, in the order they were written. seqs must strictly increase.
// run this before WalWriter::open so a torn tail is cut off before new records are appended
pub fn replay(path: &Path) -> Result<Vec<Record>, WalError> {
    // no WAL yet means a first startup, not an error
    let buffer: Vec<u8> = match fs::read(path) {
        Ok(buffer) => buffer,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(WalError::Io(error)),
    };

    let mut records: Vec<Record> = Vec::new();
    let mut position: usize = 0;
    let mut last_seq: Option<u64> = None;

    while position < buffer.len() {
        match decode(&buffer[position..]) {
            Ok(Some((record, bytes_used))) => {
                if let Some(last_seq) = last_seq
                    && record.seq <= last_seq
                {
                    return Err(WalError::Corruption {
                        offset: position as u64,
                        reason: CorruptionReason::SeqOutOfOrder,
                    });
                }
                last_seq = Some(record.seq);
                records.push(record);
                position += bytes_used;
            }
            // torn tail: the crash happened mid-append, that write was never acked
            Ok(None) => {
                truncate(path, position as u64)?;
                break;
            }
            // leave the file as is so the bad bytes can be inspected
            Err(reason) => {
                return Err(WalError::Corruption {
                    offset: position as u64,
                    reason,
                });
            }
        }
    }

    Ok(records)
}

// cuts the file off at the end of the last valid record
fn truncate(path: &Path, length: u64) -> Result<(), WalError> {
    let file: File = OpenOptions::new().write(true).open(path)?;
    file.set_len(length)?;
    // sync_all since the change is to the file length (metadata), not new data
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests;
