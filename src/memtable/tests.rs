use super::*;

// 12-byte telemetry key from the design: namespace | session | car | lap | sector | seq
fn telemetry_key(lap: u16, sector: u8, seq: u16) -> Vec<u8> {
    let mut key: Vec<u8> = vec![0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05];
    key.extend_from_slice(&lap.to_be_bytes());
    key.push(sector);
    key.extend_from_slice(&seq.to_be_bytes());
    key
}

#[test]
fn new_memtable_is_empty() {
    let memtable: Memtable = Memtable::new();

    assert!(memtable.is_empty());
    assert_eq!(memtable.len(), 0);
    assert_eq!(memtable.approximate_size(), 0);
}

#[test]
fn put_then_get_found() {
    let mut memtable: Memtable = Memtable::new();
    let key: Vec<u8> = telemetry_key(12, 3, 1);

    memtable.put(key.clone(), vec![0xAB; 2000]);

    assert_eq!(memtable.get(&key), Lookup::Found(&[0xAB; 2000]));
}

#[test]
fn get_missing_key_not_found() {
    let memtable: Memtable = Memtable::new();

    assert_eq!(memtable.get(&telemetry_key(12, 3, 1)), Lookup::NotFound);
}

#[test]
fn overwrite_returns_newest_value() {
    let mut memtable: Memtable = Memtable::new();
    let key: Vec<u8> = telemetry_key(12, 3, 1);

    memtable.put(key.clone(), vec![0x01; 10]);
    memtable.put(key.clone(), vec![0x02; 10]);

    assert_eq!(memtable.get(&key), Lookup::Found(&[0x02; 10]));
    // one entry per key, the overwrite replaced it
    assert_eq!(memtable.len(), 1);
}

#[test]
fn put_then_delete_returns_deleted() {
    let mut memtable: Memtable = Memtable::new();
    let key: Vec<u8> = telemetry_key(12, 3, 1);

    memtable.put(key.clone(), vec![0xAB; 2000]);
    memtable.delete(key.clone());

    // Deleted, not NotFound: the search must stop here
    assert_eq!(memtable.get(&key), Lookup::Deleted);
}

#[test]
fn delete_missing_key_still_writes_tombstone() {
    let mut memtable: Memtable = Memtable::new();
    let key: Vec<u8> = telemetry_key(12, 3, 1);

    // the key may only exist in an older SSTable, the tombstone has to hide it
    memtable.delete(key.clone());

    assert_eq!(memtable.get(&key), Lookup::Deleted);
    assert_eq!(memtable.len(), 1);
}

#[test]
fn put_after_delete_returns_found() {
    let mut memtable: Memtable = Memtable::new();
    let key: Vec<u8> = telemetry_key(12, 3, 1);

    memtable.delete(key.clone());
    memtable.put(key.clone(), vec![0x07; 5]);

    assert_eq!(memtable.get(&key), Lookup::Found(&[0x07; 5]));
}

#[test]
fn iter_is_sorted_and_includes_tombstones() {
    let mut memtable: Memtable = Memtable::new();

    // inserted out of order on purpose
    memtable.put(telemetry_key(13, 1, 1), vec![3]);
    memtable.delete(telemetry_key(12, 2, 1));
    memtable.put(telemetry_key(12, 1, 1), vec![1]);

    let keys: Vec<&[u8]> = memtable.iter().map(|(key, _)| key).collect();
    assert_eq!(
        keys,
        vec![
            telemetry_key(12, 1, 1).as_slice(),
            telemetry_key(12, 2, 1).as_slice(),
            telemetry_key(13, 1, 1).as_slice(),
        ]
    );

    let ops: Vec<&Op> = memtable.iter().map(|(_, op)| op).collect();
    assert_eq!(ops[1], &Op::Delete);
}

#[test]
fn range_start_inclusive_end_exclusive() {
    let mut memtable: Memtable = Memtable::new();
    memtable.put(telemetry_key(11, 3, 9), vec![11]);
    memtable.put(telemetry_key(12, 1, 1), vec![12]);
    memtable.put(telemetry_key(12, 3, 2), vec![12]);
    memtable.put(telemetry_key(13, 1, 1), vec![13]);

    // scan lap 12: start = lap 12 with a zeroed tail, end = lap 13 with a zeroed tail
    let start: Vec<u8> = telemetry_key(12, 0, 0);
    let end: Vec<u8> = telemetry_key(13, 0, 0);
    let keys: Vec<&[u8]> = memtable.range(&start, &end).map(|(key, _)| key).collect();

    assert_eq!(
        keys,
        vec![
            telemetry_key(12, 1, 1).as_slice(),
            telemetry_key(12, 3, 2).as_slice()
        ]
    );

    // a key exactly equal to start is included, one equal to end is not
    let keys: Vec<&[u8]> = memtable
        .range(&telemetry_key(12, 1, 1), &telemetry_key(13, 1, 1))
        .map(|(key, _)| key)
        .collect();
    assert_eq!(
        keys,
        vec![
            telemetry_key(12, 1, 1).as_slice(),
            telemetry_key(12, 3, 2).as_slice()
        ]
    );
}

#[test]
fn range_start_after_end_is_empty() {
    let mut memtable: Memtable = Memtable::new();
    memtable.put(telemetry_key(12, 1, 1), vec![12]);

    // backwards range returns nothing instead of panicking
    let count: usize = memtable
        .range(&telemetry_key(13, 0, 0), &telemetry_key(12, 0, 0))
        .count();

    assert_eq!(count, 0);
}

#[test]
fn size_tracks_put_overwrite_and_delete() {
    let mut memtable: Memtable = Memtable::new();
    let key: Vec<u8> = telemetry_key(12, 3, 1);

    memtable.put(key.clone(), vec![0xAB; 2000]);
    assert_eq!(memtable.approximate_size(), 12 + 2000);

    // overwrite: the old value's bytes come off, the new value's go on
    memtable.put(key.clone(), vec![0xAB; 500]);
    assert_eq!(memtable.approximate_size(), 12 + 500);

    // tombstone counts only its key
    memtable.delete(key.clone());
    assert_eq!(memtable.approximate_size(), 12);

    // a second key adds its own bytes
    memtable.put(telemetry_key(12, 3, 2), vec![0xAB; 100]);
    assert_eq!(memtable.approximate_size(), 12 + 12 + 100);
}

#[test]
fn apply_replayed_records_matches_direct_writes() {
    let key_a: Vec<u8> = telemetry_key(12, 1, 1);
    let key_b: Vec<u8> = telemetry_key(12, 2, 1);

    let mut direct: Memtable = Memtable::new();
    direct.put(key_a.clone(), vec![1; 10]);
    direct.put(key_b.clone(), vec![2; 10]);
    direct.put(key_a.clone(), vec![3; 10]);
    direct.delete(key_b.clone());

    // the same writes, as replay would hand them back from the WAL
    let records: Vec<Record> = vec![
        Record {
            key: key_a.clone(),
            op: Op::Put { value: vec![1; 10] },
        },
        Record {
            key: key_b.clone(),
            op: Op::Put { value: vec![2; 10] },
        },
        Record {
            key: key_a.clone(),
            op: Op::Put { value: vec![3; 10] },
        },
        Record {
            key: key_b.clone(),
            op: Op::Delete,
        },
    ];
    let mut replayed: Memtable = Memtable::new();
    for record in records {
        replayed.apply(record);
    }

    let direct_entries: Vec<(&[u8], &Op)> = direct.iter().collect();
    let replayed_entries: Vec<(&[u8], &Op)> = replayed.iter().collect();
    assert_eq!(replayed_entries, direct_entries);
    assert_eq!(replayed.approximate_size(), direct.approximate_size());
}
