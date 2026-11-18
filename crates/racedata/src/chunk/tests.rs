use super::*;

fn time(rows: usize, step_us: u32) -> Column {
    Column {
        id: TIME,
        data: ColumnData::U32((0..rows as u32).map(|i| i * step_us).collect()),
    }
}

fn f32_column(id: u16, rows: usize, scale: f32) -> Column {
    Column {
        id,
        data: ColumnData::F32((0..rows).map(|i| i as f32 * scale).collect()),
    }
}

// 512 rows of iRacing telemetry at 60 Hz: 9 columns, 33 bytes a row
fn iracing_telemetry(rows: usize) -> Chunk {
    Chunk::new(
        44_750_000,
        vec![
            time(rows, 16_667),
            f32_column(LAP_DIST, rows, 1.2),
            f32_column(LAP_DIST_PCT, rows, 0.0002),
            f32_column(SPEED, rows, 0.1),
            f32_column(THROTTLE, rows, 0.001),
            f32_column(BRAKE, rows, 0.0005),
            f32_column(STEERING, rows, -0.01),
            Column {
                id: GEAR,
                data: ColumnData::I8((0..rows).map(|i| (i % 8) as i8 - 1).collect()),
            },
            f32_column(RPM, rows, 20.0),
        ],
    )
    .unwrap()
}

fn iracing_position(rows: usize) -> Chunk {
    Chunk::new(
        44_750_000,
        vec![
            time(rows, 16_667),
            Column {
                id: LAT,
                data: ColumnData::F64((0..rows).map(|i| 50.445 + i as f64 * 1e-6).collect()),
            },
            Column {
                id: LON,
                data: ColumnData::F64((0..rows).map(|i| 5.964 - i as f64 * 1e-6).collect()),
            },
            f32_column(ALT, rows, 0.01),
        ],
    )
    .unwrap()
}

#[test]
fn round_trip_iracing_telemetry_and_position() {
    for chunk in [
        iracing_telemetry(512),
        iracing_position(512),
        iracing_telemetry(1),
    ] {
        let bytes: Vec<u8> = chunk.encode();
        assert_eq!(bytes.len(), chunk.encoded_len());
        assert_eq!(Chunk::decode(&bytes).unwrap(), chunk);
    }
}

#[test]
fn sizes_match_the_design() {
    // DESIGN.md section 6: 33 bytes a row, 24 + 3 x 9 header, ~17 KB for 512 rows
    let telemetry: Chunk = iracing_telemetry(512);
    assert_eq!(telemetry.encoded_len(), 24 + 3 * 9 + 512 * 33);
    let position: Chunk = iracing_position(512);
    assert_eq!(position.encoded_len(), 24 + 3 * 4 + 512 * 24);
}

#[test]
fn every_column_type_round_trips() {
    let chunk: Chunk = Chunk::new(
        7,
        vec![
            time(3, 1),
            Column {
                id: 100,
                data: ColumnData::U8(vec![0, 1, 255]),
            },
            Column {
                id: 101,
                data: ColumnData::I8(vec![-128, 0, 127]),
            },
            Column {
                id: 102,
                data: ColumnData::U16(vec![0, 1, u16::MAX]),
            },
            Column {
                id: 103,
                data: ColumnData::I32(vec![i32::MIN, 0, i32::MAX]),
            },
            Column {
                id: 104,
                data: ColumnData::F32(vec![-1.5, 0.0, f32::MAX]),
            },
            Column {
                id: 105,
                data: ColumnData::F64(vec![f64::MIN_POSITIVE, -0.0, 1e300]),
            },
        ],
    )
    .unwrap();

    assert_eq!(Chunk::decode(&chunk.encode()).unwrap(), chunk);
}

#[test]
fn layout_byte_for_byte() {
    let chunk: Chunk = Chunk::new(
        0x0102_0304_0506_0708,
        vec![
            time(2, 0x10),
            Column {
                id: GEAR,
                data: ColumnData::I8(vec![-1, 3]),
            },
        ],
    )
    .unwrap();

    let mut expected: Vec<u8> = Vec::new();
    expected.extend_from_slice(&30u16.to_le_bytes()); // header_len = 24 + 2 x 3
    expected.extend_from_slice(&1u16.to_le_bytes()); // version
    expected.extend_from_slice(&0x0102_0304_0506_0708u64.to_le_bytes()); // start
    expected.extend_from_slice(&0x0102_0304_0506_0718u64.to_le_bytes()); // end = start + 0x10
    expected.extend_from_slice(&2u16.to_le_bytes()); // count
    expected.extend_from_slice(&2u16.to_le_bytes()); // column_count
    expected.extend_from_slice(&[1, 0, 4]); // time, u32
    expected.extend_from_slice(&[8, 0, 2]); // gear, i8
    expected.extend_from_slice(&[0, 0, 0, 0, 0x10, 0, 0, 0]); // time column
    expected.extend_from_slice(&[0xFF, 3]); // gear column
    assert_eq!(chunk.encode(), expected);
}

#[test]
fn columns_sit_where_the_formulas_say() {
    // start(c) = header_len + count x (w0 + ... + w(c-1)), sample(c,i) = start(c) + i x w(c)
    let chunk: Chunk = iracing_telemetry(100);
    let bytes: Vec<u8> = chunk.encode();
    let header_len: usize = 24 + 3 * 9;
    let widths: [usize; 9] = [4, 4, 4, 4, 4, 4, 4, 1, 4];

    for (c, column) in chunk.columns().iter().enumerate() {
        let start: usize = header_len + 100 * widths[..c].iter().sum::<usize>();
        for i in [0, 1, 57, 99] {
            let at: usize = start + i * widths[c];
            match &column.data {
                ColumnData::F32(values) => assert_eq!(
                    f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()),
                    values[i]
                ),
                ColumnData::U32(values) => assert_eq!(
                    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()),
                    values[i]
                ),
                ColumnData::I8(values) => assert_eq!(bytes[at] as i8, values[i]),
                other => panic!("unexpected column {other:?}"),
            }
        }
    }
}

#[test]
fn times_and_accessors() {
    let chunk: Chunk = iracing_telemetry(60);
    assert_eq!(chunk.row_count(), 60);
    assert_eq!(chunk.start_us(), 44_750_000);
    assert_eq!(chunk.end_us(), 44_750_000 + 59 * 16_667);
    assert_eq!(chunk.time()[1], 16_667);
    assert!(matches!(chunk.column(SPEED), Some(ColumnData::F32(_))));
    assert!(chunk.column(DRS).is_none());
}

#[test]
fn max_rows_keeps_chunks_under_the_value_limit() {
    let telemetry: Vec<ColumnType> = iracing_telemetry(1)
        .columns()
        .iter()
        .map(|column| column.data.column_type())
        .collect();
    assert_eq!(max_rows(&telemetry), 512);

    // 40 f64 columns: 320 bytes a row, 512 rows would be 160 KB
    let wide: Vec<ColumnType> = vec![ColumnType::F64; 40];
    let rows: usize = max_rows(&wide);
    assert!(rows < 512);
    let mut columns: Vec<Column> = vec![time(rows, 1)];
    for id in 0..39 {
        columns.push(Column {
            id: 200 + id,
            data: ColumnData::F64(vec![1.0; rows]),
        });
    }
    let chunk: Chunk = Chunk::new(0, columns).unwrap();
    assert!(chunk.encoded_len() <= MAX_VALUE_LEN);
}

#[test]
fn unknown_columns_are_kept() {
    // a newer app's channel (e.g. ABS) still round trips through an older reader
    let chunk: Chunk = Chunk::new(
        0,
        vec![
            time(2, 5),
            Column {
                id: 4_000,
                data: ColumnData::U8(vec![0, 1]),
            },
        ],
    )
    .unwrap();
    let decoded: Chunk = Chunk::decode(&chunk.encode()).unwrap();
    assert_eq!(decoded.column(4_000), Some(&ColumnData::U8(vec![0, 1])));
}

#[test]
fn a_longer_header_from_a_newer_version_is_skipped() {
    let chunk: Chunk = iracing_position(3);
    let bytes: Vec<u8> = chunk.encode();
    let header_len: usize = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;

    // 4 extra header bytes before the first column
    let mut longer: Vec<u8> = bytes[..header_len].to_vec();
    longer.extend_from_slice(&[0xAB; 4]);
    longer.extend_from_slice(&bytes[header_len..]);
    longer[0..2].copy_from_slice(&((header_len + 4) as u16).to_le_bytes());

    assert_eq!(Chunk::decode(&longer).unwrap(), chunk);
}

#[test]
fn new_refuses_bad_chunks() {
    let bad = |columns: Vec<Column>| Chunk::new(0, columns).unwrap_err();

    assert_eq!(bad(Vec::new()), ChunkError::NoColumns);
    assert_eq!(
        bad(vec![f32_column(SPEED, 3, 1.0)]),
        ChunkError::MissingTime
    );
    assert_eq!(
        bad(vec![time(3, 1), time(3, 1)]),
        ChunkError::DuplicateColumn(TIME)
    );
    assert_eq!(
        bad(vec![time(3, 1), f32_column(SPEED, 2, 1.0)]),
        ChunkError::LengthMismatch
    );
    assert_eq!(bad(vec![time(0, 1)]), ChunkError::BadRowCount(0));
    assert_eq!(bad(vec![time(513, 1)]), ChunkError::BadRowCount(513));
    // a catalog column with the wrong type
    assert_eq!(
        bad(vec![
            time(2, 1),
            Column {
                id: SPEED,
                data: ColumnData::F64(vec![1.0, 2.0]),
            }
        ]),
        ChunkError::WrongType(SPEED)
    );
    assert_eq!(
        bad(vec![Column {
            id: TIME,
            data: ColumnData::F32(vec![0.0]),
        }]),
        ChunkError::WrongType(TIME)
    );
    assert_eq!(
        bad(vec![Column {
            id: TIME,
            data: ColumnData::U32(vec![5, 6]),
        }]),
        ChunkError::TimeNotFromZero
    );
    assert_eq!(
        bad(vec![Column {
            id: TIME,
            data: ColumnData::U32(vec![0, 10, 9]),
        }]),
        ChunkError::TimeGoesBackwards
    );
    // equal times are fine: OpenF1 can repeat a timestamp
    assert!(
        Chunk::new(
            0,
            vec![Column {
                id: TIME,
                data: ColumnData::U32(vec![0, 10, 10]),
            }]
        )
        .is_ok()
    );
    // end would overflow u64
    assert_eq!(
        Chunk::new(u64::MAX, vec![time(2, 1)]).unwrap_err(),
        ChunkError::EndMismatch
    );
    // 512 rows of 40 f64 columns: over 64 KiB
    let mut columns: Vec<Column> = vec![time(512, 1)];
    for id in 0..39 {
        columns.push(Column {
            id: 200 + id,
            data: ColumnData::F64(vec![1.0; 512]),
        });
    }
    assert!(matches!(bad(columns), ChunkError::TooLarge(_)));
}

#[test]
fn decode_refuses_bad_values() {
    let good: Vec<u8> = iracing_position(4).encode();

    let mut version: Vec<u8> = good.clone();
    version[2] = 2;
    assert_eq!(
        Chunk::decode(&version).unwrap_err(),
        ChunkError::UnsupportedVersion(2)
    );

    let mut header: Vec<u8> = good.clone();
    header[0..2].copy_from_slice(&10u16.to_le_bytes());
    assert_eq!(
        Chunk::decode(&header).unwrap_err(),
        ChunkError::BadHeaderLen
    );

    let mut column_type: Vec<u8> = good.clone();
    column_type[24 + 2] = 99;
    assert_eq!(
        Chunk::decode(&column_type).unwrap_err(),
        ChunkError::UnknownType(99)
    );

    let mut end: Vec<u8> = good.clone();
    end[12] ^= 1;
    assert_eq!(Chunk::decode(&end).unwrap_err(), ChunkError::EndMismatch);

    let mut trailing: Vec<u8> = good.clone();
    trailing.push(0);
    assert_eq!(
        Chunk::decode(&trailing).unwrap_err(),
        ChunkError::LengthMismatch
    );

    assert_eq!(
        Chunk::decode(&good[..good.len() - 1]).unwrap_err(),
        ChunkError::TooShort
    );
}

#[test]
fn every_truncation_is_an_error_not_a_panic() {
    let bytes: Vec<u8> = iracing_telemetry(20).encode();
    for len in 0..bytes.len() {
        assert!(Chunk::decode(&bytes[..len]).is_err(), "length {len}");
    }
}

#[test]
fn bit_flips_never_panic() {
    // no CRC in a value (the WAL and SSTable CRCs cover it), so a flip can decode to
    // different numbers. it must never panic or decode to something invalid
    let bytes: Vec<u8> = iracing_position(10).encode();
    for at in 0..bytes.len() {
        for bit in 0..8 {
            let mut flipped: Vec<u8> = bytes.clone();
            flipped[at] ^= 1 << bit;
            if let Ok(chunk) = Chunk::decode(&flipped) {
                assert_eq!(Chunk::decode(&chunk.encode()).unwrap(), chunk);
            }
        }
    }
}

#[test]
fn catalog_ids_are_unique_and_types_have_unique_bytes() {
    for (i, channel) in CATALOG.iter().enumerate() {
        assert!(
            CATALOG[..i].iter().all(|other| other.id != channel.id),
            "id {} twice",
            channel.id
        );
        assert_eq!(lookup(channel.id), Some(channel));
    }
    let types: [ColumnType; 7] = [
        ColumnType::U8,
        ColumnType::I8,
        ColumnType::U16,
        ColumnType::U32,
        ColumnType::I32,
        ColumnType::F32,
        ColumnType::F64,
    ];
    for column_type in types {
        assert_eq!(ColumnType::from_byte(column_type.byte()), Some(column_type));
    }
    assert_eq!(ColumnType::from_byte(0), None);
    assert_eq!(ColumnType::from_byte(8), None);
}

#[test]
fn catalog_ids_never_change() {
    // ids are written to disk: changing one breaks every stored chunk and every export
    let pinned: [(u16, &str); 16] = [
        (1, "time"),
        (2, "lap_dist"),
        (3, "lap_dist_pct"),
        (4, "speed"),
        (5, "throttle"),
        (6, "brake"),
        (7, "steering"),
        (8, "gear"),
        (9, "rpm"),
        (10, "drs"),
        (20, "lat"),
        (21, "lon"),
        (22, "alt"),
        (23, "x"),
        (24, "y"),
        (25, "z"),
    ];
    for (id, name) in pinned {
        assert_eq!(lookup(id).unwrap().name, name);
    }
}
