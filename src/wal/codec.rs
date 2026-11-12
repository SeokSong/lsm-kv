use super::error::{SizeField, WalError};
use super::record::{Op, Record};

const MAX_KEY_LEN: usize = 16;
const MAX_VALUE_LEN: usize = 65_536;
const _: () = assert!(MAX_KEY_LEN <= u8::MAX as usize);
const _: () = assert!(MAX_VALUE_LEN <= u32::MAX as usize);
const PUT_BYTE_TYPE: u8 = 1;
const DELETE_BYTE_TYPE: u8 = 2;
const HEADER_LEN: usize = 4 + 1 + 1 + 4; // crc + type + keyLen + valueLen

pub fn encode(record: &Record) -> Result<Vec<u8>, WalError> {
    let key_len = record.key.len();
    if key_len > MAX_KEY_LEN {
        return Err(WalError::Size {
            incorrect_size: key_len,
            correct_max_size: MAX_KEY_LEN,
            which_field: SizeField::Key,
        });
    }
    //get type byte
    let (byte_type, value): (u8, &[u8]) = match record.op {
        Op::Put { ref value } => {
            let value_len = value.len();
            if value_len > MAX_VALUE_LEN {
                return Err(WalError::Size {
                    incorrect_size: value_len,
                    correct_max_size: MAX_VALUE_LEN,
                    which_field: SizeField::Value,
                });
            };
            (PUT_BYTE_TYPE, value)
        }
        Op::Delete => (DELETE_BYTE_TYPE, &[]),
    };
    let value_len = value.len();

    let mut encoded_record: Vec<u8> = Vec::with_capacity(HEADER_LEN + key_len + value_len);

    let key_len = key_len as u8;
    let value_len = value_len as u32;

    encoded_record.extend_from_slice(&[0u8; 4]);

    encoded_record.extend_from_slice(&[byte_type]);
    encoded_record.extend_from_slice(&key_len.to_le_bytes());
    encoded_record.extend_from_slice(&value_len.to_le_bytes());
    encoded_record.extend_from_slice(&record.key);
    encoded_record.extend_from_slice(value);
    let checksum = crc32fast::hash(&encoded_record[4..]);
    encoded_record[0..4].copy_from_slice(&checksum.to_le_bytes());
    Ok(encoded_record)
}

#[cfg(test)]
mod tests;
