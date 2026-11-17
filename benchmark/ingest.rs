//! bulk load vs puts: loading a finished session.
//!
//! answers:
//!   - how much faster bulk load is than a put per chunk (one fsync each)
//!   - what loading a whole race costs with bulk load
//!
//! run with: cargo bench --bench ingest
//!
//! fake data shaped like the 2023 Belgian GP: 44 laps x 3 sectors x 2 streams (telemetry +
//! position) per car, 2,000-byte chunks, 12-byte keys. real OpenF1 data needs the value
//! encoding first. every iteration loads into a fresh store: loading the same session twice
//! is refused as an overlap
//!
//! fsync matters most here: run on Linux hardware for real numbers, the dev container's
//! fsync is not usable (md/BENCHMARKS.md)

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use lsm_kv::config::{Config, MemtableConfig};
use lsm_kv::store::{Ingest, Store};
use tempfile::TempDir;

const VALUE_LEN: usize = 2_000;
const LAPS: u16 = 44;
const SESSION: u32 = 9141;

// 12-byte key: namespace | session | car | lap | sector | seq
fn key(namespace: u8, car: u16, lap: u16, sector: u8) -> Vec<u8> {
    let mut key: Vec<u8> = vec![namespace];
    key.extend_from_slice(&SESSION.to_be_bytes());
    key.extend_from_slice(&car.to_be_bytes());
    key.extend_from_slice(&lap.to_be_bytes());
    key.push(sector);
    key.extend_from_slice(&1u16.to_be_bytes());
    key
}

// every chunk of the given cars, in key order: telemetry first, then position
fn race(cars: u16) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut entries: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for namespace in [0x01, 0x02] {
        for car in 1..=cars {
            for lap in 1..=LAPS {
                for sector in 1..=3 {
                    let value: Vec<u8> = vec![(lap as u8) ^ sector; VALUE_LEN];
                    entries.push((key(namespace, car, lap, sector), value));
                }
            }
        }
    }
    entries
}

// no swap or flush inside the measurement, puts only pay the WAL
fn config() -> Config {
    Config {
        memtable: MemtableConfig {
            flush_threshold_bytes: 1024 * 1024 * 1024,
            max_frozen: 1,
        },
        ..Config::default()
    }
}

fn load_with_puts(entries: &[(Vec<u8>, Vec<u8>)]) -> Duration {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), config()).unwrap();
    let start: Instant = Instant::now();
    for (key, value) in entries {
        store.put(key.clone(), value.clone()).unwrap();
    }
    start.elapsed()
}

fn load_with_ingest(entries: &[(Vec<u8>, Vec<u8>)]) -> Duration {
    let dir: TempDir = TempDir::new().unwrap();
    let store: Store = Store::open(dir.path(), config()).unwrap();
    let start: Instant = Instant::now();
    let mut ingest: Ingest<'_> = store.begin_ingest().unwrap();
    for (key, value) in entries {
        ingest.put(key.clone(), value.clone()).unwrap();
    }
    black_box(ingest.commit().unwrap());
    start.elapsed()
}

fn bench_ingest(c: &mut Criterion) {
    let one_car: Vec<(Vec<u8>, Vec<u8>)> = race(1);
    let full_race: Vec<(Vec<u8>, Vec<u8>)> = race(20);
    let bytes = |entries: &[(Vec<u8>, Vec<u8>)]| -> u64 {
        entries
            .iter()
            .map(|(k, v)| (k.len() + v.len()) as u64)
            .sum()
    };
    eprintln!(
        "ingest: one car {} entries ({} bytes), full race {} entries ({} bytes)",
        one_car.len(),
        bytes(&one_car),
        full_race.len(),
        bytes(&full_race)
    );

    let mut group = c.benchmark_group("ingest");
    // a put per chunk is one fsync each: hundreds of ms per iteration
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(20));

    group.throughput(Throughput::Elements(one_car.len() as u64));
    group.bench_function(BenchmarkId::new("puts", "one_car"), |b| {
        b.iter_custom(|iters| (0..iters).map(|_| load_with_puts(&one_car)).sum())
    });
    group.bench_function(BenchmarkId::new("bulk_load", "one_car"), |b| {
        b.iter_custom(|iters| (0..iters).map(|_| load_with_ingest(&one_car)).sum())
    });

    group.throughput(Throughput::Elements(full_race.len() as u64));
    group.bench_function(BenchmarkId::new("bulk_load", "full_race"), |b| {
        b.iter_custom(|iters| (0..iters).map(|_| load_with_ingest(&full_race)).sum())
    });

    group.finish();
}

criterion_group!(benches, bench_ingest);
criterion_main!(benches);
