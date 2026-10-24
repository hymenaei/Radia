mod packets;
mod receive;

use criterion::{Criterion, criterion_group, criterion_main};

fn bench_protocol(c: &mut Criterion) {
    packets::bench(c);
    receive::bench(c);
}

criterion_group!(benches, bench_protocol);
criterion_main!(benches);
