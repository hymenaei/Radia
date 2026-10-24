mod store;
mod texture;

use criterion::{Criterion, criterion_group, criterion_main};

fn bench_assets(c: &mut Criterion) {
    texture::bench(c);
    store::bench(c);
}

criterion_group!(benches, bench_assets);
criterion_main!(benches);
