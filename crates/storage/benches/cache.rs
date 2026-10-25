use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};
use storage::{Cache, DEFAULT_SIZE_LIMIT, Key};
use tempfile::tempdir;

fn key(value: u64) -> Key {
    let mut bytes = [0; 32];
    bytes[..8].copy_from_slice(&value.to_le_bytes());
    Key::new(bytes)
}

pub(crate) fn bench(c: &mut Criterion) {
    let directory = tempdir().expect("benchmark cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("benchmark cache");
    let mut keys = Vec::new();
    let small = vec![0x5a; 64];
    for value in 0..4096u64 {
        let item_key = key(value + 1);
        cache.put(item_key, small.clone()).expect("write small entry");
        keys.push(item_key);
    }
    let large_key = key(u64::MAX);
    cache.put(large_key, vec![0xa5; 256 * 1024]).expect("write large entry");
    cache.flush().expect("flush benchmark cache");

    let mut group = c.benchmark_group("storage/cache_lookup");
    group.throughput(Throughput::Bytes(64));
    group.bench_function(BenchmarkId::new("large_index", keys.len()), |b| {
        let mut cursor = 0usize;
        b.iter(|| {
            let selected = keys.get(cursor % keys.len()).copied().expect("benchmark key");
            cursor = cursor.wrapping_add(1);
            black_box(cache.lookup(selected))
        });
    });
    group.bench_function("many_small_entries/read_64_bytes", |b| {
        let first_key = keys.first().copied().expect("first benchmark key");
        b.iter(|| black_box(cache.lookup(first_key)));
    });
    group.throughput(Throughput::Bytes(256 * 1024));
    group.bench_function("read_256_kib", |b| {
        b.iter(|| black_box(cache.lookup(large_key)));
    });
    group.finish();
    cache.shutdown().expect("close benchmark cache");
}
