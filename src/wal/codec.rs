use super::error::{CorruptionReason, SizeField, WalError};
use super::record::{Op, Record};

const MAX_KEY_LEN: usize = 16;
const MAX_VALUE_LEN: usize = 65_536;
const TOMBSTONE_VALUE_LEN: usize = 0;
const _: () = assert!(MAX_KEY_LEN <= u8::MAX as usize);
const _: () = assert!(MAX_VALUE_LEN <= u32::MAX as usize);
const PUT_BYTE_TYPE: u8 = 1;
const DELETE_BYTE_TYPE: u8 = 2;
// crc + seq + type + keyLen + valueLen
const HEADER_LEN: usize = 4 + 8 + 1 + 1 + 4;

// checks the key and value fit the record format, without encoding anything.
// the store runs this before a memtable swap so a rejected put can't trigger a flush
pub fn validate(record: &Record) -> Result<(), WalError> {
    let key_len = record.key.len();
    if key_len > MAX_KEY_LEN {
        return Err(WalError::Size {
            incorrect_size: key_len,
            correct_max_size: MAX_KEY_LEN,
            which_field: SizeField::Key,
        });
    }
    if let Op::Put { ref value } = record.op
        && value.len() > MAX_VALUE_LEN
    {
        return Err(WalError::Size {
            incorrect_size: value.len(),
            correct_max_size: MAX_VALUE_LEN,
            which_field: SizeField::Value,
        });
    }
    Ok(())
}

pub fn encode(record: &Record) -> Result<Vec<u8>, WalError> {
    validate(record)?;
    let key_len = record.key.len();
    //get type byte
    let (byte_type, value): (u8, &[u8]) = match record.op {
        Op::Put { ref value } => (PUT_BYTE_TYPE, value),
        Op::Delete => (DELETE_BYTE_TYPE, &[]),
    };
    let value_len = value.len();

    let mut encoded_record: Vec<u8> = Vec::with_capacity(HEADER_LEN + key_len + value_len);

    let key_len = key_len as u8;
    let value_len = value_len as u32;

    encoded_record.extend_from_slice(&[0u8; 4]);

    encoded_record.extend_from_slice(&record.seq.to_le_bytes());
    encoded_record.extend_from_slice(&[byte_type]);
    encoded_record.extend_from_slice(&key_len.to_le_bytes());
    encoded_record.extend_from_slice(&value_len.to_le_bytes());
    encoded_record.extend_from_slice(&record.key);
    encoded_record.extend_from_slice(value);
    let checksum = crc32fast::hash(&encoded_record[4..]);
    encoded_record[0..4].copy_from_slice(&checksum.to_le_bytes());
    Ok(encoded_record)
}

pub fn decode(encoded_record: &[u8]) -> Result<Option<(Record, usize)>, CorruptionReason> {
    //Check for torn writes
    if encoded_record.len() < HEADER_LEN {
        return Ok(None);
    }

    //Check the header
    let crc: u32 = u32::from_le_bytes(encoded_record[0..4].try_into().unwrap());
    let seq: u64 = u64::from_le_bytes(encoded_record[4..12].try_into().unwrap());
    let byte_type: u8 = encoded_record[12];
    let key_len: u8 = encoded_record[13];
    let value_len: u32 = u32::from_le_bytes(encoded_record[14..18].try_into().unwrap());
    let key_len = key_len as usize;
    let value_len = value_len as usize;

    let record_len = HEADER_LEN + key_len + value_len;

    //Check to ensure key_len and value_len are within our MAX_KEY_LEN and MAX_VALUE_LEN
    if key_len > MAX_KEY_LEN {
        return Err(CorruptionReason::LengthLimit(SizeField::Key));
    };
    if value_len > MAX_VALUE_LEN {
        return Err(CorruptionReason::LengthLimit(SizeField::Value));
    };

    //Check for torn writes
    if encoded_record.len() < record_len {
        return Ok(None);
    }

    let crc_check = crc32fast::hash(&encoded_record[4..record_len]);

    if crc_check != crc {
        return Err(CorruptionReason::CrcMismatch);
    }

    //Type byte is only 01 or 02 anything else is Corruption
    let op: Op = match byte_type {
        PUT_BYTE_TYPE => {
            let value: Vec<u8> = encoded_record[HEADER_LEN + key_len..record_len].to_vec();
            Op::Put { value }
        }
        DELETE_BYTE_TYPE => {
            if value_len != TOMBSTONE_VALUE_LEN {
                return Err(CorruptionReason::TombstoneValue);
            };
            Op::Delete
        }
        _ => {
            return Err(CorruptionReason::UnknownType);
        }
    };

    let key = &encoded_record[HEADER_LEN..HEADER_LEN + key_len];

    let record: Record = Record {
        seq,
        key: key.to_vec(),
        op,
    };
    Ok(Some((record, record_len)))
}

#[cfg(test)]
mod tests;
