//! CRC32 (crc32fast) vs CRC32C / Castagnoli (crc32c) microbenchmark.
//!
//! Run with:   cargo bench --bench crc
//! HTML report: target/criterion/report/index.html
//!
//! Buffer sizes match the WAL records this store actually writes:
//!   - 22 B    : tombstone record (10-byte header + 12-byte key)
//!   - 2 KB    : typical sector chunk (p50 ~106 samples)
//!   - 5 KB    : slowest measured sector (242 samples)
//!   - 64 KB   : MAX_VALUE_LEN, the largest value the store accepts

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

const SIZES: &[(&str, usize)] = &[
    ("22B_tombstone", 22),
    ("2KB_typical", 2 * 1024),
    ("5KB_max_sector", 5 * 1024),
    ("64KB_max_value", 64 * 1024),
];

/// Deterministic, non-trivial bytes so neither algorithm gets a lucky input.
fn make_buffer(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i.wrapping_mul(31) ^ (i >> 3)) as u8)
        .collect()
}

fn bench_crc(c: &mut Criterion) {
    let mut group = c.benchmark_group("crc");

    for &(label, len) in SIZES {
        let buf = make_buffer(len);
        group.throughput(Throughput::Bytes(len as u64));

        group.bench_with_input(BenchmarkId::new("crc32fast", label), &buf, |b, buf| {
            b.iter(|| crc32fast::hash(black_box(buf)))
        });

        group.bench_with_input(BenchmarkId::new("crc32c", label), &buf, |b, buf| {
            b.iter(|| crc32c::crc32c(black_box(buf)))
        });
    }

    group.finish();
}

criterion_group!(benches, bench_crc);
criterion_main!(benches);
