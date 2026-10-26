use std::{
    hint::black_box,
    time::{Duration, Instant},
};

use assets::{
    Budget, Demand, Failure, Kind, LoadJob, MAX_ASSET_DECODES, MAX_ASSET_FETCHES, Priority, Request, Resource, Route, Store, TextureSize,
};
use criterion::{BatchSize, BenchmarkId, Criterion};
use test_support::texture::GRAY_4096_J2C;
use utils::AssetId;
use uuid::Uuid;

type Fixture = (Store, Vec<Demand>, Vec<Route>, Instant, Vec<assets::DecodeJob>);

fn fixture(count: usize) -> Fixture {
    let now = Instant::now();
    let mut store = Store::new(Budget::default());
    let routes = (0..4).map(|_| store.register_route().expect("benchmark route")).collect::<Vec<_>>();
    let mut demands = Vec::with_capacity(count + MAX_ASSET_DECODES * 2);
    let mut decodes = Vec::with_capacity(MAX_ASSET_DECODES);

    for (index, route) in routes.iter().enumerate().take(MAX_ASSET_DECODES) {
        let id = AssetId::new(Uuid::from_u128(u128::MAX - index as u128));
        let low =
            store.request(*route, Request::Texture { id, size: TextureSize::new(8) }, Priority::Visible, now).expect("benchmark demand");
        let LoadJob::Fetch(fetch) = store.next_job(now).expect("benchmark warm fetch") else {
            panic!("warm texture unexpectedly used a cached source");
        };
        let input = fetch.complete(Ok(GRAY_4096_J2C.to_vec())).expect("fixture response");
        let decode = store.start_decode(input).expect("benchmark warm decode");
        store.complete(decode.run(), now);
        demands.push(low);

        demands.push(
            store
                .request(*route, Request::Texture { id, size: TextureSize::new(64) }, Priority::Visible, now)
                .expect("benchmark refinement demand"),
        );
        let LoadJob::Decode(input) = store.next_job(now).expect("benchmark cached decode") else {
            panic!("warm texture refinement unexpectedly fetched");
        };
        decodes.push(store.start_decode(input).expect("benchmark cached decode admission"));
    }

    for index in 0..count {
        let id = AssetId::new(Uuid::from_u128(index as u128 + 1));
        let resource = Resource {
            id,
            kind: match index % 4 {
                0 => Kind::Texture,
                1 => Kind::PbrMaterial,
                2 => Kind::LegacyMaterial,
                _ => Kind::Environment,
            },
        };
        let priority = if index < MAX_ASSET_FETCHES / 2 || index % 7 == 0 { Priority::Visible } else { Priority::Background };
        demands.push(store.request(routes[index % routes.len()], Request::new(resource), priority, now).expect("benchmark demand"));
    }

    for _ in 0..MAX_ASSET_FETCHES / 2 {
        let job = store.next_job(now).expect("benchmark retry seed");
        store.complete(job.fail(Failure::Transient { retry_after: Duration::from_secs(20) }), now);
    }

    (store, demands, routes, now, decodes)
}

fn saturated_fixture(count: usize) -> (Fixture, Vec<LoadJob>) {
    let mut fixture = fixture(count);
    let mut jobs = Vec::with_capacity(MAX_ASSET_FETCHES);
    while jobs.len() < MAX_ASSET_FETCHES {
        let Some(job) = fixture.0.next_job(fixture.3) else { break };
        jobs.push(job);
    }
    (fixture, jobs)
}

pub(crate) fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("store");
    for count in [1_000usize, 10_000, 50_000] {
        group.bench_function(BenchmarkId::new("drain", count), |b| {
            b.iter_batched_ref(
                || fixture(count),
                |(store, demands, _, now, decodes)| {
                    let mut jobs = Vec::with_capacity(MAX_ASSET_FETCHES);
                    while let Some(job) = store.next_job(*now) {
                        jobs.push(job);
                    }
                    for decode in std::mem::take(decodes) {
                        store.complete(decode.fail(Failure::NotFound), *now);
                    }
                    let completed = jobs.len();
                    for job in jobs {
                        match job {
                            LoadJob::Fetch(fetch) => drop(store.complete(LoadJob::Fetch(fetch).fail(Failure::NotFound), *now)),
                            LoadJob::Decode(input) => {
                                let decode = store.start_decode(input).expect("benchmark decode admission");
                                drop(store.complete(decode.fail(Failure::NotFound), *now));
                            }
                            LoadJob::ReadCache(_) => unreachable!("benchmark Store has no persistent cache"),
                        }
                    }
                    black_box((completed, demands.len()));
                },
                BatchSize::PerIteration,
            );
        });

        group.bench_function(BenchmarkId::new("saturated_next_job", count), |b| {
            b.iter_custom(|iterations| {
                let ((mut store, demands, _, now, decodes), jobs) = saturated_fixture(count);
                let start = Instant::now();
                for _ in 0..iterations {
                    black_box(store.next_job(now));
                }
                let elapsed = start.elapsed();
                black_box((demands.len(), decodes.len(), jobs.len()));
                elapsed
            });
        });

        group.bench_function(BenchmarkId::new("maintain", count), |b| {
            b.iter_batched_ref(
                || fixture(count),
                |(store, demands, _, now, decodes)| {
                    store.maintain(*now);
                    black_box((demands.len(), decodes.len()));
                },
                BatchSize::PerIteration,
            );
        });

        group.bench_function(BenchmarkId::new("request", count), |b| {
            let (mut store, _demands, routes, now, _decodes) = fixture(count);
            let resource = Resource { id: AssetId::new(Uuid::from_u128(1)), kind: Kind::Texture };
            b.iter(|| {
                black_box(store.request(routes[1], Request::new(resource), Priority::Visible, now).expect("benchmark route admission"));
            });
        });
    }
    group.finish();
}
