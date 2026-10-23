mod capabilities;

use criterion::{Criterion, criterion_group, criterion_main};

fn bench_network(c: &mut Criterion) {
    capabilities::bench(c);
}

criterion_group!(benches, bench_network);
criterion_main!(benches);
