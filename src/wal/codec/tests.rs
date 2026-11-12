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
