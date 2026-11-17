use super::*;
use crate::config::SstableConfig;
use crate::sstable::{SstReader, SstWriter};
use crate::wal::Op;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn put(seq: u64, key: &[u8], value: &[u8]) -> Record {
    Record {
        seq,
        key: key.to_vec(),
        op: Op::Put {
            value: value.to_vec(),
        },
    }
}

fn tombstone(seq: u64, key: &[u8]) -> Record {
    Record {
        seq,
        key: key.to_vec(),
        op: Op::Delete,
    }
}

// a source made of records the test writes out by hand, already in key order
fn listed(records: Vec<Record>) -> Source<'static> {
    Box::new(records.into_iter().map(Ok))
}

fn collect(sources: Vec<Source<'_>>) -> Vec<Record> {
    merge(sources).map(|entry| entry.unwrap()).collect()
}

fn small_block_config() -> SstableConfig {
    SstableConfig {
        block_size_bytes: 512,
        restart_interval: 4,
        bloom_bits_per_key: 6,
        ..SstableConfig::default()
    }
}

fn write_sstable(path: &Path, memtable: &Memtable) -> SstReader {
    let mut writer: SstWriter = SstWriter::create(path, &small_block_config()).unwrap();
    for (key, seq, op) in memtable.iter() {
        writer.add(key, seq, op).unwrap();
    }
    writer.finish().unwrap();
    SstReader::open(path).unwrap()
}

// xorshift64: deterministic, no rand dependency, good enough to shuffle test data
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[test]
fn newest_seq_wins_whatever_source_it_is_in() {
    // the newest version sits in the last source: source order doesn't matter, only seqs.
    // same as a retried memtable flushed after a newer one
    let merged: Vec<Record> = collect(vec![
        listed(vec![put(1, b"a", b"old"), put(5, b"c", b"new")]),
        listed(vec![put(2, b"b", b"only"), put(3, b"c", b"old")]),
        listed(vec![put(9, b"a", b"new")]),
    ]);

    assert_eq!(
        merged,
        vec![
            put(9, b"a", b"new"),
            put(2, b"b", b"only"),
            put(5, b"c", b"new")
        ]
    );
}

#[test]
fn tombstone_hides_older_values_and_is_yielded() {
    let merged: Vec<Record> = collect(vec![
        listed(vec![tombstone(7, b"a"), put(8, b"b", b"new")]),
        listed(vec![put(3, b"a", b"old"), tombstone(2, b"b")]),
    ]);

    // the tombstone for a wins over the older put, and stays in the output for the caller
    assert_eq!(merged, vec![tombstone(7, b"a"), put(8, b"b", b"new")]);
}

#[test]
fn empty_and_missing_sources() {
    assert!(collect(Vec::new()).is_empty());
    assert!(collect(vec![listed(Vec::new()), listed(Vec::new())]).is_empty());

    let merged: Vec<Record> = collect(vec![
        listed(Vec::new()),
        listed(vec![put(1, b"a", b"1")]),
        listed(Vec::new()),
    ]);
    assert_eq!(merged, vec![put(1, b"a", b"1")]);
}

#[test]
fn sources_that_dont_overlap_come_out_in_key_order() {
    let merged: Vec<Record> = collect(vec![
        listed(vec![put(1, b"x", b"1"), put(2, b"y", b"2")]),
        listed(vec![put(3, b"a", b"3"), put(4, b"b", b"4")]),
        listed(vec![put(5, b"m", b"5")]),
    ]);
    let keys: Vec<&[u8]> = merged.iter().map(|record| record.key()).collect();
    assert_eq!(keys, vec![&b"a"[..], b"b", b"m", b"x", b"y"]);
}

#[test]
fn memtables_and_sstables_mixed() {
    let dir: TempDir = TempDir::new().unwrap();

    let mut oldest: Memtable = Memtable::new();
    oldest.put(1, b"a".to_vec(), b"sst".to_vec());
    oldest.put(2, b"b".to_vec(), b"sst".to_vec());
    oldest.put(3, b"c".to_vec(), b"sst".to_vec());
    let sstable: SstReader = write_sstable(&dir.path().join("1.sst"), &oldest);

    let mut active: Memtable = Memtable::new();
    active.delete(4, b"b".to_vec());
    active.put(5, b"d".to_vec(), b"mem".to_vec());

    let merged: Vec<Record> = collect(vec![memtable_source(&active), Box::new(sstable.iter())]);
    assert_eq!(
        merged,
        vec![
            put(1, b"a", b"sst"),
            tombstone(4, b"b"),
            put(3, b"c", b"sst"),
            put(5, b"d", b"mem"),
        ]
    );
}

#[test]
fn ranges_of_memtables_and_sstables() {
    let dir: TempDir = TempDir::new().unwrap();
    let mut older: Memtable = Memtable::new();
    let mut newer: Memtable = Memtable::new();
    for i in 0..200u64 {
        older.put(i + 1, vec![b'k', i as u8], vec![1; 50]);
    }
    for i in (0..200u64).step_by(3) {
        newer.put(1000 + i, vec![b'k', i as u8], vec![2; 50]);
    }
    let sstable: SstReader = write_sstable(&dir.path().join("1.sst"), &older);

    let start: &[u8] = &[b'k', 40];
    let end: &[u8] = &[b'k', 90];
    let merged: Vec<Record> = collect(vec![
        memtable_range_source(&newer, start, end),
        Box::new(sstable.range(start, end)),
    ]);

    assert_eq!(merged.len(), 50);
    for (i, record) in (40u64..90).zip(&merged) {
        assert_eq!(record.key(), &[b'k', i as u8][..]);
        let expected_seq: u64 = if i % 3 == 0 { 1000 + i } else { i + 1 };
        assert_eq!(record.seq(), expected_seq, "key {i}");
    }
}

// random writes spread over memtables and SSTables, checked against a BTreeMap that
// applies every write in seq order: the last write to each key is what the merge must give
#[test]
fn matches_a_model_on_random_writes() {
    let dir: TempDir = TempDir::new().unwrap();
    let mut rng: Rng = Rng(0x2C_FD_DC_05);

    for round in 0..20 {
        let source_count: usize = rng.below(6) as usize + 1;
        let mut memtables: Vec<Memtable> = (0..source_count).map(|_| Memtable::new()).collect();
        let mut model: BTreeMap<Vec<u8>, Record> = BTreeMap::new();

        let writes: u64 = rng.below(400) + 1;
        for seq in 1..=writes {
            // few distinct keys, so the same key lands in several sources
            let key: Vec<u8> = vec![b'k', rng.below(60) as u8];
            let source: usize = rng.below(source_count as u64) as usize;
            let record: Record = if rng.below(5) == 0 {
                tombstone(seq, &key)
            } else {
                put(seq, &key, &seq.to_le_bytes())
            };
            memtables[source].apply(record.clone());
            model.insert(key, record);
        }

        // about half the sources become SSTables
        let mut readers: Vec<(usize, SstReader)> = Vec::new();
        for (i, memtable) in memtables.iter().enumerate() {
            if !memtable.is_empty() && rng.below(2) == 0 {
                let path: PathBuf = dir.path().join(format!("{round}_{i}.sst"));
                readers.push((i, write_sstable(&path, memtable)));
            }
        }
        let mut sources: Vec<Source<'_>> = Vec::new();
        for (i, memtable) in memtables.iter().enumerate() {
            match readers.iter().find(|(source, _)| *source == i) {
                Some((_, reader)) => sources.push(Box::new(reader.iter())),
                None => sources.push(memtable_source(memtable)),
            }
        }

        let expected: Vec<Record> = model.into_values().collect();
        assert_eq!(collect(sources), expected, "round {round}");
    }
}

#[test]
fn source_error_is_yielded_once_then_stops() {
    let failing: Source<'static> = Box::new(
        vec![
            Ok(put(1, b"a", b"1")),
            Err(SstError::Empty),
            Ok(put(3, b"c", b"3")),
        ]
        .into_iter(),
    );
    let mut merged: MergeIter<'_> = merge(vec![failing, listed(vec![put(2, b"b", b"2")])]);

    assert_eq!(merged.next().unwrap().unwrap(), put(1, b"a", b"1"));
    assert!(matches!(
        merged.next(),
        Some(Err(MergeError::Sst(SstError::Empty)))
    ));
    assert!(merged.next().is_none());
    assert!(merged.next().is_none());
}

#[test]
fn unsorted_source_is_an_error() {
    let mut merged: MergeIter<'_> = merge(vec![
        listed(vec![put(1, b"a", b"1")]),
        listed(vec![put(2, b"m", b"2"), put(3, b"c", b"3")]),
    ]);

    // every good entry comes out first, the error comes where the bad entry is
    assert_eq!(merged.next().unwrap().unwrap(), put(1, b"a", b"1"));
    assert_eq!(merged.next().unwrap().unwrap(), put(2, b"m", b"2"));
    assert!(matches!(
        merged.next(),
        Some(Err(MergeError::OutOfOrder { source: 1 }))
    ));
    assert!(merged.next().is_none());
}

#[test]
fn same_key_twice_in_one_source_is_an_error() {
    let mut merged: MergeIter<'_> =
        merge(vec![listed(vec![put(1, b"a", b"1"), put(2, b"a", b"2")])]);

    assert_eq!(merged.next().unwrap().unwrap(), put(1, b"a", b"1"));
    assert!(matches!(
        merged.next(),
        Some(Err(MergeError::OutOfOrder { source: 0 }))
    ));
    assert!(merged.next().is_none());
}

#[test]
fn same_key_and_seq_in_two_sources_is_an_error() {
    let mut merged: MergeIter<'_> = merge(vec![
        listed(vec![put(4, b"a", b"one")]),
        listed(vec![put(4, b"a", b"other")]),
    ]);

    assert!(matches!(
        merged.next(),
        Some(Err(MergeError::DuplicateSeq { seq: 4 }))
    ));
    assert!(merged.next().is_none());
}
