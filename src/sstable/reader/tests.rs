use super::*;
use crate::config::SstableConfig;
use crate::memtable::Memtable;
use crate::sstable::{SstInfo, SstWriter};
use crate::wal::{Op, Record};
use std::fs;
use tempfile::TempDir;

// 12-byte telemetry key from the design: namespace | session | car | lap | sector | seq
fn telemetry_key(lap: u16, sector: u8, seq: u16) -> Vec<u8> {
    let mut key: Vec<u8> = vec![0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05];
    key.extend_from_slice(&lap.to_be_bytes());
    key.push(sector);
    key.extend_from_slice(&seq.to_be_bytes());
    key
}

// small blocks so a few hundred entries span many blocks
fn small_block_config() -> SstableConfig {
    SstableConfig {
        block_size_bytes: 1024,
        restart_interval: 4,
        bloom_bits_per_key: 6,
    }
}

// laps 1.., 3 sectors each, every 7th entry a tombstone. written newest key first,
// so seqs run the opposite way to key order
fn sample_memtable(count: usize, value_len: usize) -> Memtable {
    let mut memtable: Memtable = Memtable::new();
    for (seq, i) in (0..count).rev().enumerate() {
        let seq: u64 = seq as u64 + 1;
        let key: Vec<u8> = telemetry_key((i / 3) as u16 + 1, (i % 3) as u8 + 1, 1);
        if i % 7 == 6 {
            memtable.delete(seq, key);
        } else {
            memtable.put(seq, key, vec![(i % 251) as u8; value_len]);
        }
    }
    memtable
}

fn write_memtable(path: &Path, memtable: &Memtable, config: &SstableConfig) -> SstInfo {
    let mut writer: SstWriter = SstWriter::create(path, config).unwrap();
    for (key, seq, op) in memtable.iter() {
        writer.add(key, seq, op).unwrap();
    }
    writer.finish().unwrap()
}

#[test]
fn write_then_get_every_key() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("000001.sst");
    let memtable: Memtable = sample_memtable(300, 100);

    let info: SstInfo = write_memtable(&path, &memtable, &small_block_config());
    let reader: SstReader = SstReader::open(&path).unwrap();

    assert_eq!(info.entry_count, 300);
    assert_eq!(info.file_size, fs::metadata(&path).unwrap().len());
    assert!(reader.block_count() > 10);

    for (key, _, op) in memtable.iter() {
        let expected: SstLookup = match op {
            Op::Put { value } => SstLookup::Found(value.clone()),
            Op::Delete => SstLookup::Deleted,
        };
        assert_eq!(reader.get(key).unwrap(), expected);
    }
}

#[test]
fn get_missing_keys() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("000001.sst");
    write_memtable(&path, &sample_memtable(300, 100), &small_block_config());
    let reader: SstReader = SstReader::open(&path).unwrap();

    // before the first key, between keys (seq 2 is never written), after the last key
    assert_eq!(
        reader.get(&telemetry_key(0, 1, 1)).unwrap(),
        SstLookup::NotFound
    );
    assert_eq!(
        reader.get(&telemetry_key(5, 1, 2)).unwrap(),
        SstLookup::NotFound
    );
    assert_eq!(
        reader.get(&telemetry_key(500, 1, 1)).unwrap(),
        SstLookup::NotFound
    );
}

#[test]
fn iter_returns_everything_in_order() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("000001.sst");
    let memtable: Memtable = sample_memtable(300, 100);
    write_memtable(&path, &memtable, &small_block_config());
    let reader: SstReader = SstReader::open(&path).unwrap();

    let from_file: Vec<Record> = reader.iter().map(|entry| entry.unwrap()).collect();
    let from_memtable: Vec<Record> = memtable
        .iter()
        .map(|(key, seq, op)| Record {
            seq,
            key: key.to_vec(),
            op: op.clone(),
        })
        .collect();

    assert_eq!(from_file, from_memtable);
}

#[test]
fn info_has_first_and_last_key() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("000001.sst");
    let memtable: Memtable = sample_memtable(30, 10);

    let info: SstInfo = write_memtable(&path, &memtable, &small_block_config());

    assert_eq!(info.first_key, memtable.iter().next().unwrap().0);
    assert_eq!(info.last_key, memtable.iter().last().unwrap().0);
    assert_eq!(info.path, path);
    // written newest key first: the first key has the highest seq
    assert_eq!(info.min_seq, 1);
    assert_eq!(info.max_seq, 30);
    assert_eq!(info.max_seq, memtable.max_seq().unwrap());
}

#[test]
fn value_bigger_than_a_block_gets_its_own_block() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("000001.sst");
    let mut memtable: Memtable = Memtable::new();
    memtable.put(1, telemetry_key(1, 1, 1), vec![1; 50]);
    // 64 KiB value with a 1 KiB block target: soft target, never split
    memtable.put(2, telemetry_key(1, 2, 1), vec![2; 65_536]);
    memtable.put(3, telemetry_key(1, 3, 1), vec![3; 50]);

    write_memtable(&path, &memtable, &small_block_config());
    let reader: SstReader = SstReader::open(&path).unwrap();

    assert_eq!(
        reader.get(&telemetry_key(1, 2, 1)).unwrap(),
        SstLookup::Found(vec![2; 65_536])
    );
    assert_eq!(
        reader.get(&telemetry_key(1, 3, 1)).unwrap(),
        SstLookup::Found(vec![3; 50])
    );
}

#[test]
fn works_without_a_bloom_filter() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("000001.sst");
    let memtable: Memtable = sample_memtable(100, 50);
    let config: SstableConfig = SstableConfig {
        bloom_bits_per_key: 0,
        ..small_block_config()
    };

    write_memtable(&path, &memtable, &config);
    let reader: SstReader = SstReader::open(&path).unwrap();

    assert!(reader.bloom.is_none());
    for (key, _, op) in memtable.iter() {
        assert_ne!(reader.get(key).unwrap(), SstLookup::NotFound, "{op:?}");
    }
    assert_eq!(
        reader.get(&telemetry_key(500, 1, 1)).unwrap(),
        SstLookup::NotFound
    );
}

#[test]
fn bloom_filter_skips_most_missing_keys() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("000001.sst");
    write_memtable(
        &path,
        &sample_memtable(3_000, 10),
        &SstableConfig::default(),
    );
    let reader: SstReader = SstReader::open(&path).unwrap();
    let bloom: &BloomFilter = reader.bloom.as_ref().unwrap();

    // keys inside the file's key range that were never written (seq 2)
    let missing: Vec<Vec<u8>> = (0..3_000)
        .map(|i| telemetry_key((i / 3) as u16 + 1, (i % 3) as u8 + 1, 2))
        .collect();
    let maybe: usize = missing.iter().filter(|key| bloom.may_contain(key)).count();

    // 6 bits per key: ~5-6% false positives
    assert!(
        maybe < 3_000 * 8 / 100,
        "{maybe} of 3000 missing keys passed the bloom filter"
    );
}

#[test]
fn out_of_order_key_is_rejected() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("000001.sst");
    let mut writer: SstWriter = SstWriter::create(&path, &small_block_config()).unwrap();
    let put: Op = Op::Put { value: vec![1; 10] };

    writer.add(&telemetry_key(2, 1, 1), 1, &put).unwrap();

    assert!(matches!(
        writer.add(&telemetry_key(1, 1, 1), 2, &put),
        Err(SstError::OutOfOrder)
    ));
    // a duplicate key is out of order too
    assert!(matches!(
        writer.add(&telemetry_key(2, 1, 1), 3, &put),
        Err(SstError::OutOfOrder)
    ));
}

#[test]
fn empty_sstable_is_rejected_and_leaves_no_files() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("000001.sst");
    let writer: SstWriter = SstWriter::create(&path, &small_block_config()).unwrap();

    assert!(matches!(writer.finish(), Err(SstError::Empty)));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn file_only_appears_after_finish() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("000001.sst");
    let mut writer: SstWriter = SstWriter::create(&path, &small_block_config()).unwrap();
    writer
        .add(&telemetry_key(1, 1, 1), 1, &Op::Put { value: vec![1; 10] })
        .unwrap();

    // while writing, only the .tmp file exists
    assert!(!path.exists());
    assert!(dir.path().join("000001.sst.tmp").exists());

    writer.finish().unwrap();

    assert!(path.exists());
    assert!(!dir.path().join("000001.sst.tmp").exists());
}

#[test]
fn corrupt_block_reports_its_offset() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("000001.sst");
    let memtable: Memtable = sample_memtable(300, 100);
    write_memtable(&path, &memtable, &small_block_config());

    // flip a byte inside the second block
    let reader: SstReader = SstReader::open(&path).unwrap();
    let second_block: IndexEntry = reader.index[1].clone();
    let key_in_second_block: Vec<u8> = second_block.last_key.clone();
    drop(reader);
    let mut file_bytes: Vec<u8> = fs::read(&path).unwrap();
    file_bytes[second_block.offset as usize + 10] ^= 0x01;
    fs::write(&path, &file_bytes).unwrap();

    let reader: SstReader = SstReader::open(&path).unwrap();
    let result = reader.get(&key_in_second_block);

    assert!(matches!(
        result,
        Err(SstError::Corruption {
            offset,
            reason: SstCorruption::BlockCrc,
        }) if offset == second_block.offset as u64
    ));

    // iter stops at the bad block instead of skipping it
    let entries: Vec<Result<Record, SstError>> = reader.iter().collect();
    assert!(entries.last().unwrap().is_err());
}

#[test]
fn bad_magic_fails_open() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("000001.sst");
    write_memtable(&path, &sample_memtable(10, 10), &small_block_config());

    let mut file_bytes: Vec<u8> = fs::read(&path).unwrap();
    let last: usize = file_bytes.len() - 1;
    file_bytes[last] ^= 0xFF;
    fs::write(&path, &file_bytes).unwrap();

    assert!(matches!(
        SstReader::open(&path),
        Err(SstError::Corruption {
            reason: SstCorruption::BadMagic,
            ..
        })
    ));
}

#[test]
fn every_truncation_fails_open_without_panicking() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("000001.sst");
    write_memtable(&path, &sample_memtable(20, 20), &small_block_config());
    let full_file: Vec<u8> = fs::read(&path).unwrap();

    let cut_path: PathBuf = dir.path().join("cut.sst");
    for cut in 0..full_file.len() {
        fs::write(&cut_path, &full_file[..cut]).unwrap();

        assert!(SstReader::open(&cut_path).is_err(), "cut at {cut} bytes");
    }
}

#[test]
fn not_a_sstable_fails_open() {
    let dir: TempDir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("random.sst");
    fs::write(&path, vec![0x5A; 4096]).unwrap();

    assert!(SstReader::open(&path).is_err());
}
