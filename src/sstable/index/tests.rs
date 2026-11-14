use super::*;

fn sample_index() -> Vec<IndexEntry> {
    vec![
        IndexEntry {
            last_key: vec![0x01, 0x10],
            offset: 0,
            len: 100,
        },
        IndexEntry {
            last_key: vec![0x01, 0x20],
            offset: 100,
            len: 120,
        },
        IndexEntry {
            last_key: vec![0x01, 0x30],
            offset: 220,
            len: 90,
        },
    ]
}

#[test]
fn index_round_trip() {
    let index: Vec<IndexEntry> = sample_index();

    assert_eq!(decode_index(&encode_index(&index)).unwrap(), index);
}

#[test]
fn find_block_uses_last_keys() {
    let index: Vec<IndexEntry> = sample_index();

    // smaller than every last key: first block
    assert_eq!(find_block(&index, &[0x00]).unwrap().offset, 0);
    // equal to a block's last key: that block
    assert_eq!(find_block(&index, &[0x01, 0x10]).unwrap().offset, 0);
    // between two last keys: the later block, the only one that could hold it
    assert_eq!(find_block(&index, &[0x01, 0x15]).unwrap().offset, 100);
    assert_eq!(find_block(&index, &[0x01, 0x30]).unwrap().offset, 220);
    // past the last block's last key: not in this file
    assert!(find_block(&index, &[0x01, 0x31]).is_none());
}

#[test]
fn index_bit_flip_fails_crc() {
    let mut encoded: Vec<u8> = encode_index(&sample_index());
    encoded[3] ^= 0x01;

    assert!(matches!(
        decode_index(&encoded),
        Err(SstCorruption::IndexCrc)
    ));
}

#[test]
fn index_every_truncation_is_an_error_not_a_panic() {
    let encoded: Vec<u8> = encode_index(&sample_index());

    for cut in 0..encoded.len() {
        assert!(decode_index(&encoded[..cut]).is_err(), "cut at {cut} bytes");
    }
}

#[test]
fn footer_round_trip() {
    let footer: Footer = Footer {
        index_offset: 5000,
        bloom_offset: 4000,
        index_len: 300,
        bloom_len: 1000,
    };
    let file_len: u64 = 5300 + FOOTER_LEN as u64;

    let encoded: [u8; FOOTER_LEN] = footer.encode();

    assert_eq!(FOOTER_LEN, 22);
    assert_eq!(Footer::decode(&encoded, file_len).unwrap(), footer);
}

#[test]
fn footer_bad_magic_and_version() {
    let footer: Footer = Footer {
        index_offset: 0,
        bloom_offset: 0,
        index_len: 10,
        bloom_len: 0,
    };
    let file_len: u64 = 10 + FOOTER_LEN as u64;

    let mut bad_magic: [u8; FOOTER_LEN] = footer.encode();
    bad_magic[21] ^= 0xFF;
    assert!(matches!(
        Footer::decode(&bad_magic, file_len),
        Err(SstCorruption::BadMagic)
    ));

    let mut bad_version: [u8; FOOTER_LEN] = footer.encode();
    bad_version[16..18].copy_from_slice(&9u16.to_le_bytes());
    assert!(matches!(
        Footer::decode(&bad_version, file_len),
        Err(SstCorruption::UnsupportedVersion(9))
    ));
}

#[test]
fn footer_sections_must_fit_before_the_footer() {
    let footer: Footer = Footer {
        index_offset: 100,
        bloom_offset: 0,
        index_len: 50,
        bloom_len: 0,
    };

    // index ends at 150 but the footer starts at 120
    let file_len: u64 = 120 + FOOTER_LEN as u64;

    assert!(matches!(
        Footer::decode(&footer.encode(), file_len),
        Err(SstCorruption::SectionOutOfBounds)
    ));
}
