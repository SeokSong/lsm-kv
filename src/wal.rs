#[derive(Debug, PartialEq)]
pub struct Record {
    key: Vec<u8>,
    op: Op,
}

#[derive(Debug, PartialEq)]
pub enum Op {
    Put { value: Vec<u8> },
    Delete,
}

#[derive(Debug)]
pub enum SizeField {
    Key,
    Value,
}

#[derive(Debug)]
pub enum WalError {
    Corruption {
        offset: u64,
    },
    Io(std::io::Error),
    Size {
        incorrect_size: usize,
        correct_max_size: usize,
        which_field: SizeField,
    },
}

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
mod tests {
    use super::*;

    #[test]
    fn encode_base_case() {
        const PUT_HEADER: [u8; 6] = [0x01, 0x0C, 0xD0, 0x07, 0x00, 0x00];

        let record_key: Vec<u8> = vec![
            0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05, 0x00, 0x0C, 0x03, 0x00, 0x01,
        ];

        let record = Record {
            key: record_key.clone(),
            op: Op::Put {
                value: vec![0xAB; 2000],
            },
        };

        let encoded_record: Vec<u8> = encode(&record).unwrap();

        assert_eq!(encoded_record.len(), 2022);
        assert_eq!(encoded_record[4..10], PUT_HEADER);
        assert_eq!(encoded_record[10..10 + record_key.len()], record_key);
    }

    #[test]
    fn encode_tombstone() {
        const TOMBSTONE_HEADER: [u8; 6] = [0x02, 0x0C, 0x00, 0x00, 0x00, 0x00];

        let record_key: Vec<u8> = vec![
            0x02, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05, 0x00, 0x0C, 0x03, 0x00, 0x01,
        ];

        let record = Record {
            key: record_key.clone(),
            op: Op::Delete,
        };

        let encoded_record: Vec<u8> = encode(&record).unwrap();

        assert_eq!(encoded_record.len(), 22);
        assert_eq!(encoded_record[4..10], TOMBSTONE_HEADER);
        assert_eq!(encoded_record[10..10 + record_key.len()], record_key);
    }

    #[test]
    fn encode_key_max() {
        const PUT_HEADER: [u8; 6] = [0x01, 0x10, 0xD0, 0x07, 0x00, 0x00];

        let record_key = vec![0u8; MAX_KEY_LEN]; // 16 bytes; the contents don't matter

        let record = Record {
            key: record_key.clone(),
            op: Op::Put {
                value: vec![0xAB; 2000],
            },
        };

        let encoded_record: Vec<u8> = encode(&record).unwrap();

        assert_eq!(encoded_record.len(), 2026);
        assert_eq!(encoded_record[4..10], PUT_HEADER);
        assert_eq!(encoded_record[10..10 + record_key.len()], record_key);
    }

    #[test]
    fn encode_key_too_big() {
        let record_key = vec![0u8; MAX_KEY_LEN + 1]; // 17 bytes; the contents don't matter

        let record = Record {
            key: record_key.clone(),
            op: Op::Put {
                value: vec![0xAB; 2000],
            },
        };

        let result = encode(&record);
        // assert!(matches!(result, Err(WalError::Size { .. })));

        assert!(matches!(
            result,
            Err(WalError::Size {
                which_field: SizeField::Key,
                ..
            })
        ));
    }

    #[test]
    fn encode_value_max() {
        const PUT_HEADER: [u8; 6] = [0x01, 0x0C, 0x00, 0x00, 0x01, 0x00];

        let record_key: Vec<u8> = vec![
            0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05, 0x00, 0x0C, 0x03, 0x00, 0x01,
        ];

        let record = Record {
            key: record_key.clone(),
            op: Op::Put {
                value: vec![0xAB; MAX_VALUE_LEN],
            },
        };

        let encoded_record: Vec<u8> = encode(&record).unwrap();
        assert_eq!(encoded_record.len(), MAX_VALUE_LEN + 22);
        assert_eq!(encoded_record[4..10], PUT_HEADER);
        assert_eq!(encoded_record[10..10 + record_key.len()], record_key);
    }

    #[test]
    fn encode_value_too_big() {
        let record_key: Vec<u8> = vec![
            0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05, 0x00, 0x0C, 0x03, 0x00, 0x01,
        ];

        let record = Record {
            key: record_key.clone(),
            op: Op::Put {
                value: vec![0xAB; MAX_VALUE_LEN + 1],
            },
        };

        let result = encode(&record);

        assert!(matches!(
            result,
            Err(WalError::Size {
                which_field: SizeField::Value,
                ..
            })
        ));
    }
}
