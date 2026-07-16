use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use fold_rs::memtable::Memtable;
use rand::{Rng, RngExt, distr::Alphanumeric};

const OPS: usize = 1000;

fn random_bytes(rng: &mut impl Rng, len: usize) -> Bytes {
    let s: String = rng
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect();

    Bytes::from(s)
}

fn populate_memtable(entries: usize) -> Memtable {
    let mut rng = rand::rng();

    let mut memtable = Memtable::new();

    for _ in 0..entries {
        memtable.put(random_bytes(&mut rng, 16), random_bytes(&mut rng, 128));
    }

    memtable
}

fn bench_group(c: &mut Criterion, entries: usize) {
    println!("Building {entries} entries...");

    let mut memtable = populate_memtable(entries);

    let existing_keys: Vec<_> = memtable.iter().take(OPS).map(|(k, _)| k.clone()).collect();

    let mut rng = rand::rng();

    let new_pairs: Vec<_> = (0..OPS)
        .map(|_| (random_bytes(&mut rng, 16), random_bytes(&mut rng, 128)))
        .collect();

    let mut group = c.benchmark_group(format!("memtable_{entries}"));

    group.sample_size(10);

    group.bench_function(BenchmarkId::new("get_1000_ops", entries), |b| {
        b.iter(|| {
            for key in &existing_keys {
                let _ = memtable.get(key.as_ref());
            }
        });
    });

    group.bench_function(BenchmarkId::new("put_1000_ops", entries), |b| {
        b.iter(|| {
            for (k, v) in &new_pairs {
                memtable.put(k.clone(), v.clone());
            }
        });
    });

    group.bench_function(BenchmarkId::new("delete_1000_ops", entries), |b| {
        b.iter(|| {
            for key in &existing_keys {
                memtable.delete(key.clone());
            }
        });
    });

    group.finish();
}

fn memtable_1m(c: &mut Criterion) {
    bench_group(c, 1_000_000);
}

fn memtable_10m(c: &mut Criterion) {
    bench_group(c, 10_000_000);
}

criterion_group!(benches, memtable_1m, memtable_10m);

criterion_main!(benches);
