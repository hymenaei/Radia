use std::{hint::black_box, sync::Arc, time::Instant};

use assets::{
    Budget, Kind, LoadJob, Priority, Request, Resource, Route, Store,
    cache::{AssetCache, ValidatedExtent},
};
use criterion::Criterion;
use tempfile::tempdir;
use test_support::texture::RGBA_J2C;
use utils::AssetId;
use uuid::Uuid;

fn resource() -> Resource {
    Resource { id: AssetId::new(Uuid::from_u128(1)), kind: Kind::Texture }
}

fn cold_fetch_decode() {
    let now = Instant::now();
    let mut store = Store::new(Budget::default());
    let route = store.register_route().expect("register benchmark route");
    let _demand = store.request(route, Request::new(resource()), Priority::Visible, now).expect("request benchmark texture");
    let Some(LoadJob::Fetch(fetch)) = store.next_job(now) else { panic!("cold texture starts with fetch") };
    let input = fetch.complete(Ok(RGBA_J2C.to_vec())).expect("attach benchmark response");
    let decode = store.start_decode(input).expect("admit benchmark decode");
    black_box(store.complete(decode.run(), now));
}

fn warm_disk_decode(cache: &Arc<AssetCache>) {
    let now = Instant::now();
    let mut store = Store::with_cache(Budget::default(), Arc::clone(cache));
    let route: Route = store.register_route().expect("register benchmark route");
    let _demand = store.request(route, Request::new(resource()), Priority::Visible, now).expect("request benchmark texture");
    let Some(LoadJob::ReadCache(read)) = store.next_job(now) else { panic!("warm texture starts with disk read") };
    let Some(LoadJob::Decode(input)) = store.complete_cache_read(read.run(), now) else { panic!("disk hit supplies decoder input") };
    let decode = store.start_decode(input).expect("admit benchmark decode");
    black_box(store.complete(decode.run(), now));
}

pub fn bench(c: &mut Criterion) {
    let directory = tempdir().expect("benchmark cache directory");
    let cache = Arc::new(AssetCache::open(directory.path(), 1024 * 1024 * 1024).expect("open benchmark cache"));
    cache
        .put_extent(
            resource(),
            ValidatedExtent::from_validated(u64::try_from(RGBA_J2C.len()).expect("fixture length"), 0, RGBA_J2C.to_vec()),
        )
        .expect("seed benchmark cache");
    let mut group = c.benchmark_group("asset_cache_texture");
    group.bench_function("cold_fetch_decode", |b| b.iter(cold_fetch_decode));
    group.bench_function("warm_disk_read_decode", |b| b.iter(|| warm_disk_decode(&cache)));
    group.finish();
    Arc::try_unwrap(cache).expect("benchmark cache handle released").shutdown().expect("close benchmark cache");
}
