mod mesh_loading;

use criterion::{criterion_group, criterion_main};

criterion_group!(benches, mesh_loading::register);
criterion_main!(benches);
