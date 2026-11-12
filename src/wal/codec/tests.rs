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

// Recompute the CRC after a test edits the bytes, so decode gets past the
// CRC check and reaches the check the test is aiming at
fn reseal_crc(encoded_record: &mut [u8]) {
    let checksum = crc32fast::hash(&encoded_record[4..]);
    encoded_record[0..4].copy_from_slice(&checksum.to_le_bytes());
}

#[test]
fn decode_put_base_case() {
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
    let (decoded_record, bytes_used): (Record, usize) = decode(&encoded_record).unwrap().unwrap();

    assert_eq!(decoded_record, record);
    assert_eq!(bytes_used, 2022);
}

#[test]
fn decode_delete_base_case() {
    let record_key: Vec<u8> = vec![
        0x02, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05, 0x00, 0x0C, 0x03, 0x00, 0x01,
    ];

    let record = Record {
        key: record_key.clone(),
        op: Op::Delete,
    };

    let encoded_record: Vec<u8> = encode(&record).unwrap();
    let (decoded_record, bytes_used): (Record, usize) = decode(&encoded_record).unwrap().unwrap();

    assert_eq!(decoded_record, record);
    assert_eq!(bytes_used, 22);
}

#[test]
fn decode_key_and_value_max() {
    let record_key = vec![0u8; MAX_KEY_LEN]; // 16 bytes; the contents don't matter

    let record = Record {
        key: record_key.clone(),
        op: Op::Put {
            value: vec![0xAB; MAX_VALUE_LEN],
        },
    };

    let encoded_record: Vec<u8> = encode(&record).unwrap();
    let (decoded_record, bytes_used): (Record, usize) = decode(&encoded_record).unwrap().unwrap();

    assert_eq!(decoded_record, record);
    assert_eq!(bytes_used, HEADER_LEN + MAX_KEY_LEN + MAX_VALUE_LEN);
}

#[test]
fn decode_every_truncation() {
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

    // Every possible crash point: inside the header (n < 10) and inside the body (n >= 10)
    for n in 0..encoded_record.len() {
        let result = decode(&encoded_record[..n]);
        assert!(
            matches!(result, Ok(None)),
            "cut at {n} bytes gave {result:?}"
        );
    }
}

#[test]
fn decode_key_len_too_big() {
    let record_key: Vec<u8> = vec![
        0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05, 0x00, 0x0C, 0x03, 0x00, 0x01,
    ];

    let record = Record {
        key: record_key.clone(),
        op: Op::Put {
            value: vec![0xAB; 2000],
        },
    };

    let mut encoded_record: Vec<u8> = encode(&record).unwrap();
    encoded_record[5] = (MAX_KEY_LEN + 1) as u8;

    // The limit check runs before the CRC check, so no reseal is needed
    let result = decode(&encoded_record);

    assert!(matches!(
        result,
        Err(WalError::Corruption {
            reason: CorruptionReason::LengthLimit(SizeField::Key),
        })
    ));
}

#[test]
fn decode_value_len_too_big() {
    let record_key: Vec<u8> = vec![
        0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05, 0x00, 0x0C, 0x03, 0x00, 0x01,
    ];

    let record = Record {
        key: record_key.clone(),
        op: Op::Put {
            value: vec![0xAB; 2000],
        },
    };

    let mut encoded_record: Vec<u8> = encode(&record).unwrap();
    encoded_record[6..10].copy_from_slice(&((MAX_VALUE_LEN + 1) as u32).to_le_bytes());

    let result = decode(&encoded_record);

    assert!(matches!(
        result,
        Err(WalError::Corruption {
            reason: CorruptionReason::LengthLimit(SizeField::Value),
        })
    ));
}

#[test]
fn decode_unknown_type_byte() {
    let record_key: Vec<u8> = vec![
        0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05, 0x00, 0x0C, 0x03, 0x00, 0x01,
    ];

    let record = Record {
        key: record_key.clone(),
        op: Op::Put {
            value: vec![0xAB; 2000],
        },
    };

    let mut encoded_record: Vec<u8> = encode(&record).unwrap();
    encoded_record[4] = 0x03;
    reseal_crc(&mut encoded_record);

    let result = decode(&encoded_record);

    assert!(matches!(
        result,
        Err(WalError::Corruption {
            reason: CorruptionReason::UnknownType,
        })
    ));
}

#[test]
fn decode_tombstone_with_value() {
    let record_key: Vec<u8> = vec![
        0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05, 0x00, 0x0C, 0x03, 0x00, 0x01,
    ];

    let record = Record {
        key: record_key.clone(),
        op: Op::Put {
            value: vec![0xAB; 2000],
        },
    };

    // A Put record relabeled as a Delete still carries its 2000-byte value
    let mut encoded_record: Vec<u8> = encode(&record).unwrap();
    encoded_record[4] = DELETE_BYTE_TYPE;
    reseal_crc(&mut encoded_record);

    let result = decode(&encoded_record);

    assert!(matches!(
        result,
        Err(WalError::Corruption {
            reason: CorruptionReason::TombstoneValue,
        })
    ));
}

#[test]
fn decode_bit_flip() {
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

    // One flip in the key, one in the value
    for index in [12, 1000] {
        let mut flipped_record: Vec<u8> = encoded_record.clone();
        flipped_record[index] ^= 0x01;

        let result = decode(&flipped_record);

        assert!(matches!(
            result,
            Err(WalError::Corruption {
                reason: CorruptionReason::CrcMismatch,
            })
        ));
    }
}

#[test]
fn decode_byte_type_bit_flip() {
    let record_key: Vec<u8> = vec![
        0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05, 0x00, 0x0C, 0x03, 0x00, 0x01,
    ];

    let record = Record {
        key: record_key.clone(),
        op: Op::Put {
            value: vec![0xAB; 2000],
        },
    };

    // Turns Put (01) into Delete (02). The CRC must catch it before decode acts on the type
    let mut encoded_record: Vec<u8> = encode(&record).unwrap();
    encoded_record[4] ^= 0x03;

    let result = decode(&encoded_record);

    assert!(matches!(
        result,
        Err(WalError::Corruption {
            reason: CorruptionReason::CrcMismatch,
        })
    ));
}

#[test]
fn decode_back_to_back_records() {
    let record_key: Vec<u8> = vec![
        0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05, 0x00, 0x0C, 0x03, 0x00, 0x01,
    ];

    let put_record = Record {
        key: record_key.clone(),
        op: Op::Put {
            value: vec![0xAB; 2000],
        },
    };

    let delete_record = Record {
        key: record_key.clone(),
        op: Op::Delete,
    };

    let encoded_put: Vec<u8> = encode(&put_record).unwrap();
    let encoded_delete: Vec<u8> = encode(&delete_record).unwrap();

    // Put, then Delete, then the first 5 bytes of another record (a torn tail)
    let mut buffer: Vec<u8> = Vec::new();
    buffer.extend_from_slice(&encoded_put);
    buffer.extend_from_slice(&encoded_delete);
    buffer.extend_from_slice(&encoded_put[..5]);

    let (first_record, first_len): (Record, usize) = decode(&buffer).unwrap().unwrap();
    assert_eq!(first_record, put_record);
    assert_eq!(first_len, 2022);

    let (second_record, second_len): (Record, usize) =
        decode(&buffer[first_len..]).unwrap().unwrap();
    assert_eq!(second_record, delete_record);
    assert_eq!(second_len, 22);

    let result = decode(&buffer[first_len + second_len..]);
    assert!(matches!(result, Ok(None)));
}

#[test]
fn decode_corrupt_in_range() {
    let record_key: Vec<u8> = vec![
        0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05, 0x00, 0x0C, 0x03, 0x00, 0x01,
    ];

    let record = Record {
        key: record_key.clone(),
        op: Op::Put {
            value: vec![0xAB; 2000],
        },
    };

    // Known limitation: valueLen corrupted from 2000 to 3000 (still under the limit) at the
    // end of the buffer looks exactly like a torn tail, so decode reports Ok(None), not corruption
    let mut encoded_record: Vec<u8> = encode(&record).unwrap();
    encoded_record[6..10].copy_from_slice(&3000u32.to_le_bytes());

    let result = decode(&encoded_record);

    assert!(matches!(result, Ok(None)));
}
