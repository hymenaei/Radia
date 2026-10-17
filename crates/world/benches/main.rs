use criterion::{Criterion, criterion_group, criterion_main};

mod lifecycle;
mod load;
mod pose;
mod queries;
mod relations;
mod support;
mod updates;

fn bench_world(c: &mut Criterion) {
    lifecycle::bench(c);
    load::bench(c);
    updates::bench(c);
    queries::bench(c);
    relations::bench(c);
    pose::bench(c);
}

criterion_group!(benches, bench_world);
criterion_main!(benches);
