use criterion::{Criterion, criterion_group, criterion_main};

mod inventory_browser;

fn bench_inventory_browser(c: &mut Criterion) {
    inventory_browser::bench(c);
}

criterion_group!(benches, bench_inventory_browser);
criterion_main!(benches);
