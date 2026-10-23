use std::{hint::black_box, thread, time::Instant};

use assets::{Budget, DecodeJob, Demand, LoadJob, Priority, Request, Store, TextureSize};
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};
use test_support::texture::GRAY_4096_J2C;
use utils::AssetId;
use uuid::Uuid;

fn decode_jobs(edge: u32, workers: usize) -> (Store, Vec<DecodeJob>, Vec<Demand>, Instant) {
    let now = Instant::now();
    let mut store = Store::new(Budget::default());
    let route = store.register_route().expect("benchmark route");
    let mut demands = Vec::with_capacity(workers);
    for index in 0..workers {
        let id = AssetId::new(Uuid::from_u128(index as u128 + 1));
        demands.push(
            store.request(route, Request::Texture { id, size: TextureSize::new(edge) }, Priority::Visible, now).expect("benchmark demand"),
        );
    }
    let mut jobs = Vec::with_capacity(workers);
    for _ in 0..workers {
        let LoadJob::Fetch(fetch) = store.next_job(now).expect("benchmark fetch") else {
            panic!("new texture request unexpectedly used a cached source");
        };
        let input = fetch.complete(Ok(GRAY_4096_J2C.to_vec())).expect("fixture response");
        let job = store.start_decode(input).unwrap_or_else(|_| panic!("decode admission available"));
        jobs.push(job);
    }
    (store, jobs, demands, now)
}

pub(crate) fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("texture/assets_decode");
    group.sample_size(10);
    for edge in [512u32, 1024, 2048] {
        for workers in [1usize, 2] {
            group.throughput(Throughput::Elements(u64::from(edge).pow(2) * workers as u64));
            group.bench_function(BenchmarkId::new(format!("gray_4096/{workers}_workers"), edge), |b| {
                b.iter_batched(
                    || decode_jobs(edge, workers),
                    |(mut store, jobs, demands, now)| {
                        let completions = thread::scope(|scope| {
                            jobs.into_iter()
                                .map(|job| scope.spawn(move || job.run()))
                                .collect::<Vec<_>>()
                                .into_iter()
                                .map(|job| job.join().expect("decode worker completes"))
                                .collect::<Vec<_>>()
                        });
                        for completion in completions {
                            store.complete(completion, now);
                        }
                        for demand in demands {
                            black_box(demand.status());
                        }
                    },
                    BatchSize::SmallInput,
                );
            });
        }
    }
    group.finish();
}
