//! one full-size SSTable: what a flush costs, and what reads cost once it's on disk.
//!
//! answers:
//!   - how long one flush of a full 32 MiB memtable takes (write + fsync + rename)
//!   - how long opening an SSTable takes (footer, index, bloom filter)
//!   - get cost: key present, key missing with the bloom filter, key missing without it
//!   - how long iterating every entry takes (what compaction will do with each input)
//!
//! run with: cargo bench --bench sstable
//!
//! fake data with the real shape: 12-byte telemetry keys, 20 cars, 2,000-byte values.
//! there's no value compression, so value bytes don't change the timings, only their size.
//! reads run with a warm page cache. a cold cache is a linux-only rerun (drop_caches)

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use lsm_kv::config::{Config, SstableConfig};
use lsm_kv::memtable::Memtable;
use lsm_kv::sstable::{SstInfo, SstLookup, SstReader, SstWriter};
use tempfile::TempDir;

const VALUE_LEN: usize = 2_000;
const CARS: usize = 20;
// step through keys by a large prime so lookups jump around the file instead of walking it
const STRIDE: usize = 7_919;

// 12-byte telemetry key: namespace | session | car | lap | sector | seq
fn telemetry_key(car: u16, lap: u16, sector: u8, seq: u16) -> Vec<u8> {
    let mut key: Vec<u8> = vec![0x01, 0x00, 0x2C, 0xFD, 0xDC];
    key.extend_from_slice(&car.to_be_bytes());
    key.extend_from_slice(&lap.to_be_bytes());
    key.push(sector);
    key.extend_from_slice(&seq.to_be_bytes());
    key
}

// i-th chunk of a race: every car's sector 1, then every car's sector 2, ...
fn nth_key(i: usize, seq: u16) -> Vec<u8> {
    let car: u16 = (i % CARS) as u16 + 1;
    let sector_index: usize = i / CARS;
    telemetry_key(
        car,
        (sector_index / 3) as u16 + 1,
        (sector_index % 3) as u8 + 1,
        seq,
    )
}

// filled the way the store fills it: until it reaches the flush threshold
fn full_memtable(flush_threshold_bytes: usize) -> (Memtable, Vec<Vec<u8>>) {
    let mut memtable: Memtable = Memtable::new();
    let mut keys: Vec<Vec<u8>> = Vec::new();
    let mut i: usize = 0;
    while memtable.approximate_size() < flush_threshold_bytes {
        let key: Vec<u8> = nth_key(i, 1);
        memtable.put(i as u64 + 1, key.clone(), vec![(i % 251) as u8; VALUE_LEN]);
        keys.push(key);
        i += 1;
    }
    (memtable, keys)
}

// what a flush does: every memtable entry into one SSTable, then finish (fsync, rename, fsync dir)
fn write_sstable(memtable: &Memtable, path: &Path, config: &SstableConfig) -> SstInfo {
    let mut writer: SstWriter = SstWriter::create(path, config).unwrap();
    for (key, seq, op) in memtable.iter() {
        writer.add(key, seq, op).unwrap();
    }
    writer.finish().unwrap()
}

fn bench_sstable(c: &mut Criterion) {
    let config: Config = Config::default();
    let dir: TempDir = TempDir::new().unwrap();
    let (memtable, keys) = full_memtable(config.memtable.flush_threshold_bytes);

    let with_bloom: PathBuf = dir.path().join("with_bloom.sst");
    let info: SstInfo = write_sstable(&memtable, &with_bloom, &config.sstable);
    let no_bloom_config: SstableConfig = SstableConfig {
        bloom_bits_per_key: 0,
        ..config.sstable.clone()
    };
    let no_bloom: PathBuf = dir.path().join("no_bloom.sst");
    let no_bloom_info: SstInfo = write_sstable(&memtable, &no_bloom, &no_bloom_config);

    let reader: SstReader = SstReader::open(&with_bloom).unwrap();
    let no_bloom_reader: SstReader = SstReader::open(&no_bloom).unwrap();

    // the file facts for BENCHMARKS.md, printed once
    let raw_bytes: usize = keys.iter().map(|key| key.len() + VALUE_LEN).sum();
    eprintln!(
        "sstable: {} entries, {} blocks, file {} bytes, raw key + value {} bytes, bloom filter {} bytes",
        info.entry_count,
        reader.block_count(),
        info.file_size,
        raw_bytes,
        info.file_size - no_bloom_info.file_size,
    );

    let mut group = c.benchmark_group("sstable");

    // one flush is ~tens of ms, 10 samples is plenty
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(15));
    group.throughput(Throughput::Bytes(info.file_size));
    // iter_custom so deleting the file afterwards isn't timed
    group.bench_function(BenchmarkId::new("flush_write", info.entry_count), |b| {
        b.iter_custom(|iters| {
            let mut total: Duration = Duration::ZERO;
            for _ in 0..iters {
                let path: PathBuf = dir.path().join("flush.sst");
                let start: Instant = Instant::now();
                black_box(write_sstable(&memtable, &path, &config.sstable));
                total += start.elapsed();
                std::fs::remove_file(&path).unwrap();
            }
            total
        })
    });

    group.bench_function(BenchmarkId::new("iterate_all", info.entry_count), |b| {
        b.iter(|| {
            let mut count: usize = 0;
            for entry in reader.iter() {
                black_box(entry.unwrap());
                count += 1;
            }
            assert_eq!(count, info.entry_count);
        })
    });

    group.sample_size(100);
    group.measurement_time(Duration::from_secs(5));
    group.throughput(Throughput::Elements(1));

    group.bench_function("open", |b| {
        b.iter(|| SstReader::open(black_box(&with_bloom)).unwrap())
    });

    let mut index: usize = 0;
    let mut next_index = move || {
        index = (index + STRIDE) % keys.len();
        index
    };

    group.bench_function("get_present", |b| {
        b.iter(|| {
            let key: Vec<u8> = nth_key(next_index(), 1);
            match reader.get(black_box(&key)).unwrap() {
                SstLookup::Found(value) => black_box(value.len()),
                other => panic!("expected Found, got {other:?}"),
            }
        })
    });

    // seq 2 doesn't exist, but the key sorts between real keys, so without the bloom filter
    // the lookup reads and searches a block
    group.bench_function("get_missing_with_bloom", |b| {
        b.iter(|| {
            let key: Vec<u8> = nth_key(next_index(), 2);
            assert_eq!(reader.get(black_box(&key)).unwrap(), SstLookup::NotFound);
        })
    });

    group.bench_function("get_missing_no_bloom", |b| {
        b.iter(|| {
            let key: Vec<u8> = nth_key(next_index(), 2);
            assert_eq!(
                no_bloom_reader.get(black_box(&key)).unwrap(),
                SstLookup::NotFound
            );
        })
    });

    group.finish();
}

criterion_group!(benches, bench_sstable);
criterion_main!(benches);
