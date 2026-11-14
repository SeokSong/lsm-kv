use super::*;

fn key(i: u32) -> Vec<u8> {
    let mut key: Vec<u8> = vec![0x01, 0x00, 0x2C, 0xFD, 0xDC];
    key.extend_from_slice(&i.to_be_bytes());
    key
}

fn build(count: u32, bits_per_key: usize) -> BloomFilter {
    let hashes: Vec<u64> = (0..count).map(|i| stable_hash(&key(i))).collect();
    BloomFilter::build(&hashes, bits_per_key)
}

#[test]
fn stable_hash_never_changes() {
    // bloom bits are saved on disk. if these change, every existing SSTable's filter breaks.
    // expected values computed separately from the FNV-1a + splitmix64 definition
    assert_eq!(stable_hash(b""), 0xf52a_15e9_a9b5_e89b);
    assert_eq!(stable_hash(b"lsm-kv"), 0xd606_63a8_6dd0_86db);
    assert_eq!(
        stable_hash(&[
            0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05, 0x00, 0x0C, 0x03, 0x00, 0x01
        ]),
        0xc2a3_3ac2_a99d_1f9c
    );
}

#[test]
fn no_false_negatives() {
    let filter: BloomFilter = build(10_000, 6);

    for i in 0..10_000 {
        assert!(filter.may_contain(&key(i)), "key {i} was added");
    }
}

#[test]
fn false_positive_rate_matches_bits_per_key() {
    // expected rate: ~5.6% at 6 bits/key, ~0.8% at 10 bits/key
    for (bits_per_key, max_rate) in [(6, 0.08), (10, 0.015)] {
        let filter: BloomFilter = build(10_000, bits_per_key);

        let false_positives: usize = (10_000..30_000)
            .filter(|&i| filter.may_contain(&key(i)))
            .count();
        let rate: f64 = false_positives as f64 / 20_000.0;

        assert!(rate < max_rate, "{bits_per_key} bits/key: rate {rate}");
    }
}

#[test]
fn num_hashes_follows_bits_per_key() {
    // k = bits_per_key * ln(2), rounded
    assert_eq!(num_hashes_for(6), 4);
    assert_eq!(num_hashes_for(10), 7);
    assert_eq!(num_hashes_for(1), 1);
}

#[test]
fn encode_decode_round_trip() {
    let filter: BloomFilter = build(500, 6);

    let decoded: BloomFilter = BloomFilter::decode(&filter.encode()).unwrap();

    assert_eq!(decoded.num_hashes, filter.num_hashes);
    assert_eq!(decoded.bits, filter.bits);
    for i in 0..500 {
        assert!(decoded.may_contain(&key(i)));
    }
}

#[test]
fn bit_flip_fails_crc() {
    let mut encoded: Vec<u8> = build(500, 6).encode();
    encoded[10] ^= 0x01;

    assert!(matches!(
        BloomFilter::decode(&encoded),
        Err(SstCorruption::BloomCrc)
    ));
}

#[test]
fn every_truncation_is_an_error_not_a_panic() {
    let encoded: Vec<u8> = build(50, 6).encode();

    for cut in 0..encoded.len() {
        assert!(
            BloomFilter::decode(&encoded[..cut]).is_err(),
            "cut at {cut} bytes"
        );
    }
}
