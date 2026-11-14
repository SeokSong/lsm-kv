use super::*;
use crate::wal::Record;

// 12-byte telemetry key from the design: namespace | session | car | lap | sector | seq
fn telemetry_key(lap: u16, sector: u8, seq: u16) -> Vec<u8> {
    let mut key: Vec<u8> = vec![0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05];
    key.extend_from_slice(&lap.to_be_bytes());
    key.push(sector);
    key.extend_from_slice(&seq.to_be_bytes());
    key
}

// lap 1 sector 1, lap 1 sector 2, ... in key order, every 5th one a tombstone.
// seqs go down while keys go up: an SSTable is sorted by key, never by seq
fn sample_entries(count: usize) -> Vec<Record> {
    (0..count)
        .map(|i| {
            let key: Vec<u8> = telemetry_key((i / 3) as u16 + 1, (i % 3) as u8 + 1, 1);
            let op: Op = if i % 5 == 4 {
                Op::Delete
            } else {
                Op::Put {
                    value: vec![i as u8; 30],
                }
            };
            Record {
                seq: 1_000 - i as u64,
                key,
                op,
            }
        })
        .collect()
}

fn build_block(entries: &[Record], restart_interval: usize) -> Vec<u8> {
    let mut builder: BlockBuilder = BlockBuilder::new(restart_interval);
    for record in entries {
        builder.add(&record.key, record.seq, &record.op);
    }
    builder.finish()
}

#[test]
fn entries_round_trip() {
    let entries: Vec<Record> = sample_entries(20);
    let bytes: Vec<u8> = build_block(&entries, 16);

    let block: Block<'_> = Block::parse(&bytes).unwrap();

    assert_eq!(block.entries().unwrap(), entries);
}

#[test]
fn prefix_compression_shrinks_the_block() {
    let entries: Vec<Record> = sample_entries(20);

    // restart every entry = every key stored in full, no compression
    let uncompressed: Vec<u8> = build_block(&entries, 1);
    let compressed: Vec<u8> = build_block(&entries, 16);

    // telemetry keys in one block share most of their 12 bytes
    assert!(compressed.len() < uncompressed.len());
    assert_eq!(
        Block::parse(&compressed).unwrap().entries().unwrap(),
        entries
    );
}

#[test]
fn get_every_key_for_any_restart_interval() {
    let entries: Vec<Record> = sample_entries(40);

    for restart_interval in [1, 2, 3, 16, 100] {
        let bytes: Vec<u8> = build_block(&entries, restart_interval);
        let block: Block<'_> = Block::parse(&bytes).unwrap();

        for record in &entries {
            let expected: BlockLookup = match &record.op {
                Op::Put { value } => BlockLookup::Found(value.clone()),
                Op::Delete => BlockLookup::Deleted,
            };
            assert_eq!(
                block.get(&record.key).unwrap(),
                expected,
                "restart interval {restart_interval}"
            );
        }
    }
}

#[test]
fn get_missing_keys() {
    let entries: Vec<Record> = sample_entries(10);
    let bytes: Vec<u8> = build_block(&entries, 3);
    let block: Block<'_> = Block::parse(&bytes).unwrap();

    // before the first key, between two keys, after the last key
    assert_eq!(
        block.get(&telemetry_key(0, 1, 1)).unwrap(),
        BlockLookup::NotFound
    );
    assert_eq!(
        block.get(&telemetry_key(1, 1, 2)).unwrap(),
        BlockLookup::NotFound
    );
    assert_eq!(
        block.get(&telemetry_key(99, 1, 1)).unwrap(),
        BlockLookup::NotFound
    );
}

#[test]
fn bit_flip_fails_crc() {
    let bytes: Vec<u8> = build_block(&sample_entries(10), 16);

    for index in [0, bytes.len() / 2, bytes.len() - 5] {
        let mut flipped: Vec<u8> = bytes.clone();
        flipped[index] ^= 0x01;

        assert!(matches!(
            Block::parse(&flipped),
            Err(SstCorruption::BlockCrc)
        ));
    }
}

#[test]
fn every_truncation_is_an_error_not_a_panic() {
    let bytes: Vec<u8> = build_block(&sample_entries(10), 3);

    for cut in 0..bytes.len() {
        assert!(Block::parse(&bytes[..cut]).is_err(), "cut at {cut} bytes");
    }
}

#[test]
fn bad_entry_with_valid_crc_is_caught() {
    // a block whose CRC is fine but whose first entry claims a 1000-byte value
    let mut block: Vec<u8> = vec![TYPE_PUT, 0, 1];
    block.extend_from_slice(&1000u32.to_le_bytes());
    block.extend_from_slice(&1u64.to_le_bytes()); // seq
    block.push(0x42);
    block.extend_from_slice(&0u32.to_le_bytes()); // restart at 0
    block.extend_from_slice(&1u32.to_le_bytes()); // 1 restart
    let checksum: u32 = crc32fast::hash(&block);
    block.extend_from_slice(&checksum.to_le_bytes());

    let parsed: Block<'_> = Block::parse(&block).unwrap();

    assert!(matches!(parsed.entries(), Err(SstCorruption::BadEntry)));
    assert!(matches!(parsed.get(&[0x42]), Err(SstCorruption::BadEntry)));
}

#[test]
fn builder_resets_after_finish() {
    let mut builder: BlockBuilder = BlockBuilder::new(16);
    let first: Vec<Record> = sample_entries(5);
    for record in &first {
        builder.add(&record.key, record.seq, &record.op);
    }
    builder.finish();
    assert!(builder.is_empty());

    // the next block starts fresh: its first key is a restart, stored in full
    let second: Vec<Record> = vec![Record {
        seq: 1,
        key: telemetry_key(50, 1, 1),
        op: Op::Put { value: vec![7; 10] },
    }];
    builder.add(&second[0].key, second[0].seq, &second[0].op);
    let bytes: Vec<u8> = builder.finish();

    assert_eq!(Block::parse(&bytes).unwrap().entries().unwrap(), second);
}

#[test]
fn entries_keep_their_seq() {
    let mut builder: BlockBuilder = BlockBuilder::new(16);
    builder.add(&telemetry_key(1, 1, 1), u64::MAX, &Op::Delete);
    builder.add(&telemetry_key(1, 2, 1), 0, &Op::Put { value: vec![1; 5] });
    let bytes: Vec<u8> = builder.finish();

    let seqs: Vec<u64> = Block::parse(&bytes)
        .unwrap()
        .entries()
        .unwrap()
        .iter()
        .map(|record| record.seq)
        .collect();

    assert_eq!(seqs, vec![u64::MAX, 0]);
}
