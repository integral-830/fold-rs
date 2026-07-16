use criterion::{Criterion, criterion_group, criterion_main};
use fold_rs::wal::format::{RecordType, WalRecord};
use fold_rs::wal::writer::WalWriter;
use tempfile::tempdir;

fn bench_wal_append(c: &mut Criterion) {
    for value_size in [100, 1024, 10 * 1024] {
        c.bench_function(&format!("wal_append_{value_size}B"), |b| {
            let dir = tempdir().unwrap();
            let path = dir.path().join("wal.log");

            let mut writer = WalWriter::open(path).unwrap();

            let key = b"key";
            let value = vec![b'x'; value_size];

            b.iter(|| {
                let record = WalRecord {
                    record_type: RecordType::Put,
                    key,
                    value: &value,
                };

                writer.append(&record).unwrap();
            });
        });
    }
}

criterion_group!(benches, bench_wal_append);
criterion_main!(benches);
