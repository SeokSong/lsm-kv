//! memtable get: borrow vs copy vs allocate + copy.
//!
//! answers: for a ~2 KB value, is the allocation in `to_vec()` more expensive than the copy itself?
//!   - borrow:      `Lookup::Found(&[u8])`, no copy (what zero-copy reads would give)
//!   - copy only:   copy into a buffer that is reused, so no allocation
//!   - to_vec:      allocate + copy, what `Store::get` does today
//!
//! run with: cargo bench --bench memtable_get

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use lsm_kv::memtable::{Lookup, Memtable};
use std::hint::black_box;

// one full 32 MiB memtable of typical sector chunks
const VALUE_LEN: usize = 2000;
const ENTRIES: usize = 16_384;

fn telemetry_key(lap: u16, sector: u8, seq: u16) -> Vec<u8> {
    let mut key: Vec<u8> = vec![0x01, 0x00, 0x2C, 0xFD, 0xDC, 0x00, 0x05];
    key.extend_from_slice(&lap.to_be_bytes());
    key.push(sector);
    key.extend_from_slice(&seq.to_be_bytes());
    key
}

fn bench_memtable_get(c: &mut Criterion) {
    let mut memtable: Memtable = Memtable::new();
    let mut keys: Vec<Vec<u8>> = Vec::with_capacity(ENTRIES);
    for i in 0..ENTRIES {
        let key: Vec<u8> = telemetry_key((i / 3) as u16, (i % 3) as u8 + 1, 1);
        memtable.put(i as u64 + 1, key.clone(), vec![(i % 251) as u8; VALUE_LEN]);
        keys.push(key);
    }

    let mut group = c.benchmark_group("memtable_get");
    group.throughput(Throughput::Bytes(VALUE_LEN as u64));

    // step through keys by a large prime so lookups jump around the map instead of walking it in order
    let mut index: usize = 0;
    let mut next_key = move || {
        index = (index + 7_919) % ENTRIES;
        index
    };

    group.bench_function(BenchmarkId::new("borrow", VALUE_LEN), |b| {
        b.iter(|| {
            let key: &[u8] = &keys[next_key()];
            match memtable.get(black_box(key)) {
                Lookup::Found(value) => black_box(value.len()),
                _ => unreachable!(),
            }
        })
    });

    let mut buffer: Vec<u8> = Vec::with_capacity(VALUE_LEN);
    group.bench_function(
        BenchmarkId::new("copy_into_reused_buffer", VALUE_LEN),
        |b| {
            b.iter(|| {
                let key: &[u8] = &keys[next_key()];
                match memtable.get(black_box(key)) {
                    Lookup::Found(value) => {
                        buffer.clear();
                        buffer.extend_from_slice(value);
                        black_box(buffer.len())
                    }
                    _ => unreachable!(),
                }
            })
        },
    );

    group.bench_function(BenchmarkId::new("to_vec", VALUE_LEN), |b| {
        b.iter(|| {
            let key: &[u8] = &keys[next_key()];
            match memtable.get(black_box(key)) {
                Lookup::Found(value) => black_box(value.to_vec()),
                _ => unreachable!(),
            }
        })
    });

    group.finish();
}

criterion_group!(benches, bench_memtable_get);
criterion_main!(benches);
