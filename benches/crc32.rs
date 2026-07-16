use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use rand::{Rng, rng};

fn crc32_1mb(c: &mut Criterion) {
    let mut data = vec![0u8; 1024 * 1024];
    rng().fill_bytes(&mut data);

    c.bench_function("crc32_1mb", |b| {
        b.iter(|| crc32fast::hash(black_box(&data)));
    });
}

criterion_group!(benches, crc32_1mb);
criterion_main!(benches);
