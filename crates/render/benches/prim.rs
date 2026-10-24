use std::hint::black_box;

use criterion::{BatchSize, BenchmarkId, Criterion};
use render::scene::Detail;
use world::PrimShape;

use crate::support;

pub(crate) fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("scene/prim_mesh");
    for (shape_name, shape) in [
        ("box", PrimShape::default()),
        ("cylinder", PrimShape { profile_curve: 0, ..PrimShape::default() }),
        ("sphere", PrimShape { path_curve: 0x20, profile_curve: 5, ..PrimShape::default() }),
        ("torus", PrimShape { path_curve: 0x20, profile_curve: 0, path_scale_x: 150, path_scale_y: 150, ..PrimShape::default() }),
    ] {
        for detail in [Detail::Lowest, Detail::Low, Detail::Medium, Detail::High] {
            group.bench_function(BenchmarkId::new(shape_name, format!("{detail:?}")), |b| {
                b.iter_batched(
                    || support::prim_job(shape, detail),
                    |(reconciler, job)| {
                        black_box(job.run());
                        black_box(reconciler);
                    },
                    BatchSize::SmallInput,
                );
            });
        }
    }
    group.finish();
}
