use criterion::{Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: test_support::allocation::CountingAllocator = test_support::allocation::CountingAllocator;

mod inventory;

fn bench_inventory(c: &mut Criterion) {
    inventory::bench(c);
}

criterion_group!(benches, bench_inventory);
criterion_main!(benches);
