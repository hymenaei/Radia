use std::{
    hint::black_box,
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

use criterion::{BenchmarkId, Criterion, Throughput};
use storage::{Cache, DEFAULT_SIZE_LIMIT, Key, Namespace, NamespaceId, StorageError, UnavailableReason};
use tempfile::tempdir;

const EXISTING_ENTRIES: u64 = 50_000;
const BURST_WRITES: usize = 16;
const SETUP_WRITERS: usize = 32;
const PAYLOAD_BYTES: usize = 64;
const NAMESPACES: [Namespace; 1] = [Namespace::new("bench", "bin")];

fn key(value: u64) -> Key {
    let mut name = [0; 16];
    name[..8].copy_from_slice(&value.to_le_bytes());
    Key::new(NamespaceId::new(0), name)
}

fn put_until_accepted(cache: &Cache, key: Key) {
    loop {
        match cache.put(key, vec![0x5a; PAYLOAD_BYTES]) {
            Ok(()) => return,
            Err(StorageError::Unavailable(UnavailableReason::WriteQueueFull)) => thread::yield_now(),
            Err(error) => panic!("benchmark put failed: {error}"),
        }
    }
}

fn populate(cache: &Cache) {
    let barrier = Arc::new(Barrier::new(SETUP_WRITERS));
    thread::scope(|scope| {
        for worker in 0..SETUP_WRITERS {
            let barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                barrier.wait();
                for value in (u64::try_from(worker).expect("worker index fits")..EXISTING_ENTRIES).step_by(SETUP_WRITERS) {
                    put_until_accepted(cache, key(value));
                }
            });
        }
    });
    cache.flush().expect("compact benchmark fixture");
}

pub(crate) fn bench(c: &mut Criterion) {
    let directory = tempdir().expect("create benchmark directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &NAMESPACES).expect("open benchmark cache");
    populate(&cache);
    assert_eq!(cache.entry_count(), usize::try_from(EXISTING_ENTRIES).expect("entry count fits"));

    let hit_key = key(EXISTING_ENTRIES / 2);
    std::assert_matches!(cache.lookup(hit_key), storage::Lookup::Hit(_));
    let cache_ref = &cache;
    let mut group = c.benchmark_group("storage/scale");

    group.throughput(Throughput::Elements(1));
    group.bench_function(BenchmarkId::new("put_single", EXISTING_ENTRIES), |b| {
        let mut sequence = EXISTING_ENTRIES + 1;
        b.iter(|| {
            cache_ref.put(key(sequence), vec![0x5a; PAYLOAD_BYTES]).expect("single put");
            sequence += 1;
        });
    });

    group.throughput(Throughput::Elements(BURST_WRITES as u64));
    group.bench_function(BenchmarkId::new("put_burst_16", EXISTING_ENTRIES), |b| {
        let mut sequence = EXISTING_ENTRIES + 100_000;
        b.iter(|| {
            let barrier = Arc::new(Barrier::new(BURST_WRITES));
            thread::scope(|scope| {
                for _ in 0..BURST_WRITES {
                    let barrier = Arc::clone(&barrier);
                    let item_key = key(sequence);
                    sequence += 1;
                    scope.spawn(move || {
                        barrier.wait();
                        put_until_accepted(cache_ref, item_key);
                    });
                }
            });
        });
    });

    group.throughput(Throughput::Elements(1));
    group.bench_function(BenchmarkId::new("lookup_hit", EXISTING_ENTRIES), |b| {
        b.iter(|| black_box(cache_ref.lookup(hit_key)));
    });
    group.finish();
    cache.shutdown().expect("close benchmark cache");

    let mut group = c.benchmark_group("storage/scale/open_replay");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(2));
    group.bench_function(BenchmarkId::new("open_replay", EXISTING_ENTRIES), |b| {
        b.iter_custom(|iterations| {
            let mut measured = Duration::ZERO;
            for _ in 0..iterations {
                let start = Instant::now();
                let opened = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &NAMESPACES).expect("open replay fixture");
                measured += start.elapsed();
                opened.shutdown().expect("close replay fixture");
            }
            measured
        });
    });
    group.finish();
}
