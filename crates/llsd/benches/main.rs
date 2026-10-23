mod codec;

use criterion::{Criterion, criterion_group, criterion_main};

fn bench_llsd(c: &mut Criterion) {
    codec::bench(c);
}

criterion_group!(benches, bench_llsd);
criterion_main!(benches);
