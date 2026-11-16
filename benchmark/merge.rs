//! merge iterator: how fast compaction can read several overlapping SSTables as one sorted stream.
//!
//! answers:
//!   - merge throughput for 4 overlapping SSTables (what an L0 -> L1 compaction reads)
//!   - what the merge adds on top of reading the same files one after another
//!
//! run with: cargo bench --bench merge
//!
//! fake data with the real shape, like benchmark/sstable.rs: 12-byte telemetry keys, 2,000-byte
//! values. 4 SSTables of ~8 MiB, every key lands in one of them, every 10th key in all 4
//! (overwrites, the newest one wins)

use std::hint::black_box;
use std::path::PathBuf;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use lsm_kv::config::SstableConfig;
use lsm_kv::memtable::Memtable;
use lsm_kv::merge::{Source, merge};
use lsm_kv::sstable::{SstReader, SstWriter};
use tempfile::TempDir;

const VALUE_LEN: usize = 2_000;
const KEYS: usize = 16_000;
const TABLES: usize = 4;

// 12-byte telemetry key: namespace | session | car | lap | sector | seq
fn telemetry_key(i: usize) -> Vec<u8> {
    let car: u16 = (i % 20) as u16 + 1;
    let sector_index: usize = i / 20;
    let mut key: Vec<u8> = vec![0x01, 0x00, 0x2C, 0xFD, 0xDC];
    key.extend_from_slice(&car.to_be_bytes());
    key.extend_from_slice(&((sector_index / 3) as u16 + 1).to_be_bytes());
    key.push((sector_index % 3) as u8 + 1);
    key.extend_from_slice(&1u16.to_be_bytes());
    key
}

// table t holds the keys with i % TABLES == t, plus every 10th key. later tables are newer
fn table(t: usize) -> Memtable {
    let mut memtable: Memtable = Memtable::new();
    for i in 0..KEYS {
        if i % TABLES == t || i % 10 == 0 {
            let seq: u64 = (t * KEYS + i + 1) as u64;
            memtable.put(seq, telemetry_key(i), vec![(i % 251) as u8; VALUE_LEN]);
        }
    }
    memtable
}

fn bench_merge(c: &mut Criterion) {
    let dir: TempDir = TempDir::new().unwrap();
    let config: SstableConfig = SstableConfig::default();

    let mut readers: Vec<SstReader> = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut total_entries: usize = 0;
    for t in 0..TABLES {
        let path: PathBuf = dir.path().join(format!("{t}.sst"));
        let memtable: Memtable = table(t);
        let mut writer: SstWriter = SstWriter::create(&path, &config).unwrap();
        for (key, seq, op) in memtable.iter() {
            writer.add(key, seq, op).unwrap();
        }
        let info = writer.finish().unwrap();
        total_bytes += info.file_size;
        total_entries += info.entry_count;
        readers.push(SstReader::open(&path).unwrap());
    }
    eprintln!(
        "merge: {TABLES} sstables, {total_entries} entries, {total_bytes} bytes, {KEYS} distinct keys"
    );

    let mut group = c.benchmark_group("merge");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(10));
    group.throughput(Throughput::Bytes(total_bytes));

    // baseline: the same files read one after another, no merging
    group.bench_function(BenchmarkId::new("read_one_by_one", TABLES), |b| {
        b.iter(|| {
            let mut count: usize = 0;
            for reader in &readers {
                for entry in reader.iter() {
                    black_box(entry.unwrap());
                    count += 1;
                }
            }
            assert_eq!(count, total_entries);
        })
    });

    // what compaction will do: one sorted stream, the newest version of each key
    group.bench_function(BenchmarkId::new("merge", TABLES), |b| {
        b.iter(|| {
            let sources: Vec<Source<'_>> = readers
                .iter()
                .map(|reader| Box::new(reader.iter()) as Source<'_>)
                .collect();
            let mut count: usize = 0;
            for entry in merge(sources) {
                black_box(entry.unwrap());
                count += 1;
            }
            assert_eq!(count, KEYS);
        })
    });

    group.finish();
}

criterion_group!(benches, bench_merge);
criterion_main!(benches);
