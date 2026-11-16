//! what a durable write costs: raw file appends with and without fsync, and a whole Store::put.
//!
//! answers:
//!   - how much of a put is the fsync (write only vs write + sync_data vs write + sync_all)
//!   - how much would group commit help (n records, one sync_data, cost per record)
//!   - what Store::put adds on top of the raw append (encode, WAL lock, memtable insert)
//!
//! run with: cargo bench --bench fsync
//!
//! macOS: std's sync_data and sync_all are both F_FULLFSYNC, so they cost the same there.
//! linux: sync_data is fdatasync, sync_all is fsync. only linux numbers go in BENCHMARKS.md

use std::fs::{File, OpenOptions};
use std::hint::black_box;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use lsm_kv::config::{Config, MemtableConfig};
use lsm_kv::store::Store;
use tempfile::TempDir;

// a typical sector chunk as the WAL writes it: 18-byte header + 12-byte key + 2,000-byte value
const RECORD_LEN: usize = 2_030;
const VALUE_LEN: usize = 2_000;
// records per sync_data for the group commit preview
const BATCH_SIZES: &[usize] = &[1, 4, 16, 64];

fn open_append(path: &Path) -> File {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap()
}

// deterministic bytes, the content doesn't matter to the disk
fn make_record(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i.wrapping_mul(31) ^ (i >> 3)) as u8)
        .collect()
}

// 12-byte telemetry key, unique per i, so every put is a new memtable entry
fn telemetry_key(i: u64) -> Vec<u8> {
    let mut key: Vec<u8> = vec![0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05];
    key.extend_from_slice(&((i / 3) as u16).to_be_bytes());
    key.push((i % 3) as u8 + 1);
    key.extend_from_slice(&((i / (3 * 65_536)) as u16).to_be_bytes());
    key
}

fn bench_append(c: &mut Criterion) {
    let dir: TempDir = TempDir::new().unwrap();
    let record: Vec<u8> = make_record(RECORD_LEN);

    let mut group = c.benchmark_group("append");
    group.throughput(Throughput::Elements(1));

    // page cache only, nothing reaches the disk
    let mut file: File = open_append(&dir.path().join("write_only.wal"));
    group.bench_function(BenchmarkId::new("write_only", RECORD_LEN), |b| {
        b.iter(|| file.write_all(black_box(&record)).unwrap())
    });

    // fsync calls take milliseconds, so fewer samples keep the run short
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(10));

    // what WalWriter::append does today
    let mut file: File = open_append(&dir.path().join("sync_data.wal"));
    group.bench_function(BenchmarkId::new("write_sync_data", RECORD_LEN), |b| {
        b.iter(|| {
            file.write_all(black_box(&record)).unwrap();
            file.sync_data().unwrap();
        })
    });

    // also flushes metadata like the modified time. on linux this shows what sync_data saves
    let mut file: File = open_append(&dir.path().join("sync_all.wal"));
    group.bench_function(BenchmarkId::new("write_sync_all", RECORD_LEN), |b| {
        b.iter(|| {
            file.write_all(black_box(&record)).unwrap();
            file.sync_all().unwrap();
        })
    });

    group.finish();
}

// group commit preview: n records written, then one sync_data for all of them.
// throughput is per record, so criterion reports the cost of one record in the batch
fn bench_group_commit(c: &mut Criterion) {
    let dir: TempDir = TempDir::new().unwrap();
    let record: Vec<u8> = make_record(RECORD_LEN);

    let mut group = c.benchmark_group("group_commit");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(10));

    for &batch in BATCH_SIZES {
        let mut file: File = open_append(&dir.path().join(format!("batch_{batch}.wal")));
        group.throughput(Throughput::Elements(batch as u64));
        group.bench_with_input(
            BenchmarkId::new("records_per_sync", batch),
            &batch,
            |b, &batch| {
                b.iter(|| {
                    for _ in 0..batch {
                        file.write_all(black_box(&record)).unwrap();
                    }
                    file.sync_data().unwrap();
                })
            },
        );
    }

    group.finish();
}

// the whole put path: validate, encode, WAL lock, write + sync_data, memtable insert
fn bench_store_put(c: &mut Criterion) {
    let dir: TempDir = TempDir::new().unwrap();
    // threshold far above what the run writes, so no swap or flush lands inside the measurement
    let config: Config = Config {
        memtable: MemtableConfig {
            flush_threshold_bytes: 1024 * 1024 * 1024,
            max_frozen: 1,
        },
        ..Config::default()
    };
    let store: Store = Store::open(dir.path(), config).unwrap();
    let value: Vec<u8> = make_record(VALUE_LEN);

    let mut group = c.benchmark_group("store_put");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(10));
    group.throughput(Throughput::Elements(1));

    let mut i: u64 = 0;
    group.bench_function(BenchmarkId::new("put", VALUE_LEN), |b| {
        b.iter(|| {
            i += 1;
            store.put(telemetry_key(i), value.clone()).unwrap();
        })
    });

    group.finish();
    assert_eq!(store.stats().flushes, 0, "a flush ran during the benchmark");
}

criterion_group!(benches, bench_append, bench_group_commit, bench_store_put);
criterion_main!(benches);
