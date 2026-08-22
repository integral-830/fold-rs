use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use bytes::Bytes;
use fold_rs::storage_engine::StorageEngine;

const SSTABLE_COUNTS: &[usize] = &[1, 5, 10, 20];
const ENTRIES_PER_SSTABLE: usize = 1000;
const VALUE_SIZE: usize = 256;

fn build_engine(dir: &std::path::Path, sstable_count: usize) -> StorageEngine {
    let mut engine = StorageEngine::open(dir).unwrap();

    for generation in 0..sstable_count {
        for i in 0..ENTRIES_PER_SSTABLE {
            let key = format!("sstable-{generation:04}-key-{i:06}");
            let value = vec![(generation % 256) as u8; VALUE_SIZE];

            engine.put(key.as_bytes(), &value).unwrap();
        }

        engine.flush_for_test().unwrap();
    }

    engine
}

fn benchmark_read_amplification(c: &mut Criterion) {
    let mut group = c.benchmark_group("read_amplification");

    for &sstable_count in SSTABLE_COUNTS {
        let dir = tempfile::tempdir().unwrap();

        let engine = build_engine(dir.path(), sstable_count);

        let keys: Vec<Bytes> = (0..sstable_count)
            .map(|generation| Bytes::from(format!("sstable-{generation:04}-key-000500")))
            .collect();

        group.bench_with_input(
            BenchmarkId::from_parameter(sstable_count),
            &keys,
            |b, keys| {
                let mut index = 0usize;

                b.iter(|| {
                    let key = &keys[index % keys.len()];
                    index += 1;

                    black_box(engine.get(key).unwrap());
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, benchmark_read_amplification);

criterion_main!(benches);
