use super::*;

// 12-byte telemetry key from the design: namespace | session | car | lap | sector | seq
fn telemetry_key(lap: u16, sector: u8, seq: u16) -> Vec<u8> {
    let mut key: Vec<u8> = vec![0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05];
    key.extend_from_slice(&lap.to_be_bytes());
    key.push(sector);
    key.extend_from_slice(&seq.to_be_bytes());
    key
}

// two flushed SSTables (files 2 and 4) and the active WAL segment (file 5)
fn sample_manifest() -> Manifest {
    Manifest {
        next_file_number: 6,
        last_seq: 300,
        wal_segments: vec![5],
        sstables: vec![
            SstMeta {
                file_number: 2,
                file_size: 33_000_000,
                min_seq: 1,
                max_seq: 150,
                first_key: telemetry_key(1, 1, 1),
                last_key: telemetry_key(50, 3, 1),
            },
            SstMeta {
                file_number: 4,
                file_size: 31_000_000,
                min_seq: 151,
                max_seq: 300,
                first_key: telemetry_key(51, 1, 1),
                last_key: telemetry_key(99, 3, 1),
            },
        ],
    }
}

#[test]
fn round_trip() {
    let manifest: Manifest = sample_manifest();

    let encoded: Vec<u8> = encode(&manifest).unwrap();

    assert_eq!(decode(&encoded).unwrap(), manifest);
}

#[test]
fn empty_manifest_round_trip() {
    let manifest: Manifest = Manifest::new();

    let encoded: Vec<u8> = encode(&manifest).unwrap();

    // nothing but the fixed fields
    assert_eq!(encoded.len(), MIN_LEN);
    assert_eq!(decode(&encoded).unwrap(), manifest);
}

#[test]
fn layout_byte_for_byte() {
    let manifest: Manifest = Manifest {
        next_file_number: 3,
        last_seq: 7,
        wal_segments: vec![2],
        sstables: vec![SstMeta {
            file_number: 1,
            file_size: 100,
            min_seq: 1,
            max_seq: 7,
            first_key: vec![0x0A],
            last_key: vec![0x0B, 0x0C],
        }],
    };

    let encoded: Vec<u8> = encode(&manifest).unwrap();

    let mut expected: Vec<u8> = Vec::new();
    expected.extend_from_slice(&[0x4D, 0x4D, 0x53, 0x4C]); // magic "LSMM", little endian
    expected.extend_from_slice(&[0x01, 0x00]); // version 1
    expected.extend_from_slice(&3u64.to_le_bytes());
    expected.extend_from_slice(&7u64.to_le_bytes());
    expected.extend_from_slice(&1u32.to_le_bytes()); // 1 wal segment
    expected.extend_from_slice(&2u64.to_le_bytes());
    expected.extend_from_slice(&1u32.to_le_bytes()); // 1 sstable
    expected.extend_from_slice(&1u64.to_le_bytes());
    expected.extend_from_slice(&100u64.to_le_bytes());
    expected.extend_from_slice(&1u64.to_le_bytes());
    expected.extend_from_slice(&7u64.to_le_bytes());
    expected.extend_from_slice(&[0x01, 0x0A]);
    expected.extend_from_slice(&[0x02, 0x0B, 0x0C]);
    let checksum: u32 = crc32fast::hash(&expected);
    expected.extend_from_slice(&checksum.to_le_bytes());

    assert_eq!(encoded, expected);
}

#[test]
fn bit_flip_fails_crc() {
    let encoded: Vec<u8> = encode(&sample_manifest()).unwrap();

    for index in [0, 10, encoded.len() / 2, encoded.len() - 1] {
        let mut flipped: Vec<u8> = encoded.clone();
        flipped[index] ^= 0x01;

        assert_eq!(
            decode(&flipped),
            Err(ManifestCorruption::Crc),
            "byte {index}"
        );
    }
}

#[test]
fn every_truncation_is_an_error_not_a_panic() {
    let encoded: Vec<u8> = encode(&sample_manifest()).unwrap();

    for cut in 0..encoded.len() {
        assert!(decode(&encoded[..cut]).is_err(), "cut at {cut} bytes");
    }
}

// rewrites the CRC so decode gets past it and reaches the check the test is aiming at
fn reseal_crc(bytes: &mut Vec<u8>) {
    let content_len: usize = bytes.len() - CRC_LEN;
    bytes.truncate(content_len);
    let checksum: u32 = crc32fast::hash(bytes);
    bytes.extend_from_slice(&checksum.to_le_bytes());
}

#[test]
fn bad_magic() {
    let mut encoded: Vec<u8> = encode(&sample_manifest()).unwrap();
    encoded[0] ^= 0xFF;
    reseal_crc(&mut encoded);

    assert_eq!(decode(&encoded), Err(ManifestCorruption::BadMagic));
}

#[test]
fn unsupported_version() {
    let mut encoded: Vec<u8> = encode(&sample_manifest()).unwrap();
    encoded[4..6].copy_from_slice(&2u16.to_le_bytes());
    reseal_crc(&mut encoded);

    assert_eq!(
        decode(&encoded),
        Err(ManifestCorruption::UnsupportedVersion(2))
    );
}

#[test]
fn count_past_the_end_is_truncated() {
    let mut encoded: Vec<u8> = encode(&sample_manifest()).unwrap();
    // wal count sits after magic + version + next_file_number + last_seq
    encoded[22..26].copy_from_slice(&u32::MAX.to_le_bytes());
    reseal_crc(&mut encoded);

    assert_eq!(decode(&encoded), Err(ManifestCorruption::Truncated));
}

#[test]
fn trailing_bytes() {
    let mut encoded: Vec<u8> = encode(&sample_manifest()).unwrap();
    let crc_start: usize = encoded.len() - CRC_LEN;
    encoded.insert(crc_start, 0x00);
    reseal_crc(&mut encoded);

    assert_eq!(decode(&encoded), Err(ManifestCorruption::TrailingBytes));
}

#[test]
fn file_number_listed_twice() {
    let mut manifest: Manifest = sample_manifest();
    // file 4 as an SSTable and as a WAL segment
    manifest.wal_segments.push(4);

    assert_eq!(
        encode(&manifest),
        Err(ManifestCorruption::DuplicateFileNumber(4))
    );
}

#[test]
fn file_number_never_handed_out() {
    let mut manifest: Manifest = sample_manifest();
    manifest.wal_segments.push(6);

    assert_eq!(
        encode(&manifest),
        Err(ManifestCorruption::FileNumberNotAllocated(6))
    );
}

#[test]
fn bad_seq_ranges() {
    let mut backwards: Manifest = sample_manifest();
    backwards.sstables[0].min_seq = 200;
    assert_eq!(encode(&backwards), Err(ManifestCorruption::BadSeqRange(2)));

    // an SSTable can't hold a seq newer than last_seq
    let mut past_last_seq: Manifest = sample_manifest();
    past_last_seq.sstables[1].max_seq = 301;
    assert_eq!(
        encode(&past_last_seq),
        Err(ManifestCorruption::BadSeqRange(4))
    );
}

#[test]
fn bad_key_range() {
    let mut manifest: Manifest = sample_manifest();
    manifest.sstables[0].first_key = telemetry_key(60, 1, 1);

    assert_eq!(encode(&manifest), Err(ManifestCorruption::BadKeyRange(2)));
}

#[test]
fn key_too_long() {
    let mut manifest: Manifest = sample_manifest();
    manifest.sstables[0].last_key = vec![0xFF; 256];

    assert_eq!(encode(&manifest), Err(ManifestCorruption::KeyTooLong(2)));
}

#[test]
fn decode_runs_the_same_checks() {
    // build bytes for a manifest encode would refuse, by fixing one up after encoding
    let mut encoded: Vec<u8> = encode(&sample_manifest()).unwrap();
    // next_file_number 6 -> 3, so files 4 and 5 were never handed out
    encoded[6..14].copy_from_slice(&3u64.to_le_bytes());
    reseal_crc(&mut encoded);

    assert_eq!(
        decode(&encoded),
        Err(ManifestCorruption::FileNumberNotAllocated(5))
    );
}
