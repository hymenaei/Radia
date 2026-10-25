mod cache;

use criterion::{Criterion, criterion_group, criterion_main};

fn bench_storage(c: &mut Criterion) {
    cache::bench(c);
}

criterion_group!(benches, bench_storage);
criterion_main!(benches);
