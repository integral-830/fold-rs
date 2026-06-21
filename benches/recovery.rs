use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use fold_rs::{
    storage_engine::StorageEngine,
    wal::{
        format::{RecordType, WalRecord},
        writer::WalWriter,
    },
};
use tempfile::TempDir;

fn generate_wal(records: usize) -> TempDir {
    let dir = tempfile::tempdir().unwrap();

    let wal_path = dir.path().join("wal.log");

    let mut writer = WalWriter::open(&wal_path).unwrap();

    let value = vec![b'x'; 128];

    for i in 0..records {
        let key = format!("key-{i}");

        let record = WalRecord {
            record_type: RecordType::Put,
            key: key.as_bytes(),
            value: &value,
        };

        writer.append_without_sync(&record).unwrap();
    }

    writer.sync().unwrap();

    dir
}

fn recovery_bench(c: &mut Criterion) {
    let wal_10k = generate_wal(10_000);

    let wal_100k = generate_wal(100_000);

    let wal_1m = generate_wal(1_000_000);

    let mut group = c.benchmark_group("recovery");

    group.sample_size(10);

    group.bench_function(BenchmarkId::new("open", "10k"), |b| {
        b.iter(|| {
            let engine = StorageEngine::open(wal_10k.path()).unwrap();

            std::hint::black_box(engine);
        });
    });

    group.bench_function(BenchmarkId::new("open", "100k"), |b| {
        b.iter(|| {
            let engine = StorageEngine::open(wal_100k.path()).unwrap();

            std::hint::black_box(engine);
        });
    });

    group.bench_function(BenchmarkId::new("open", "1m"), |b| {
        b.iter(|| {
            let engine = StorageEngine::open(wal_1m.path()).unwrap();

            std::hint::black_box(engine);
        });
    });

    group.finish();
}

criterion_group!(benches, recovery_bench);

criterion_main!(benches);
