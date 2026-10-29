use std::{assert_matches, sync::Arc, thread, time::Duration};

use tempfile::tempdir;

use super::*;

const TEST_NAMESPACES: [Namespace; 2] = [Namespace::new("tests/storage", "bin"), Namespace::new("assets/textures", "j2c")];
const TEST_NAMESPACE: NamespaceId = NamespaceId::new(0);

fn key(value: u16) -> Key {
    let mut name = [0; 16];
    name[..2].copy_from_slice(&value.to_le_bytes());
    Key::new(TEST_NAMESPACE, name)
}

fn same_shard_key(value: u16) -> Key {
    let mut name = [0; 16];
    name[1..3].copy_from_slice(&value.to_be_bytes());
    Key::new(TEST_NAMESPACE, name)
}

fn test_low_watermark(limit: u64) -> u64 {
    let headroom = (MAX_VALUE_BYTES as u64 + 64 * 1024).min(limit / 8);
    limit.saturating_sub(headroom)
}
#[test]
fn size_limit_evicts_least_recently_used() {
    let single = tempdir().expect("single cache directory");
    let single_cache = Cache::open(single.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open single cache");
    single_cache.put(key(10), vec![1; 64 * 1024]).expect("store sample entry");
    let one_entry_limit = single_cache.accounted_size();
    single_cache.shutdown().expect("close single cache");

    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
    for id in [11, 12, 13] {
        cache.put(key(id), vec![1; 64 * 1024]).expect("store entry");
    }
    std::assert_matches!(cache.lookup(key(12)), Lookup::Hit(_));
    cache.set_size_limit(one_entry_limit).expect("shrink cache");
    assert_eq!(cache.entry_count(), 1);
    let Lookup::Hit(value) = cache.lookup(key(12)) else {
        panic!("most recently used entry remains");
    };
    assert_eq!(value.bytes, vec![1; 64 * 1024]);
    assert_eq!(value.use_count, 2);
    assert_eq!(cache.lookup(key(11)), Lookup::Miss);
    assert_eq!(cache.lookup(key(13)), Lookup::Miss);
    assert!(cache.accounted_size() <= cache.size_limit());
    cache.clear().expect("clear cache");
    assert_eq!(cache.entry_count(), 0);
    cache.shutdown().expect("close cache");
}

#[test]
fn set_limit_enforces_once_per_writer_batch() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
    cache.put(key(100), vec![1; 1024]).expect("store first entry");
    let one_entry_limit = cache.accounted_size();
    for id in 101..140 {
        cache.put(key(id), vec![1; 1024]).expect("store entry");
    }
    let current_limit = cache.accounted_size();
    let (data_started, resume_data) = cache.pause_at(WritePoint::DataTempCreated);
    let (limit_started, resume_limit) = cache.pause_at(WritePoint::SetLimitReplied);
    cache.arm_fault(WritePoint::DataWriteFailure);
    thread::scope(|scope| {
        let blocker = scope.spawn(|| cache.put(key(200), vec![0; 1024]));
        data_started.recv_timeout(Duration::from_secs(3)).expect("blocker reached its data write");
        let (first_limit_queued, resume_first_limit) = cache.pause_at(WritePoint::SetLimitQueued);
        let (final_limit_queued, resume_final_limit) = cache.pause_at(WritePoint::SetLimitQueued);
        let first_limit = scope.spawn(|| cache.set_size_limit(current_limit));
        first_limit_queued.recv_timeout(Duration::from_secs(3)).expect("first limit entered the queue");
        resume_first_limit.send(()).expect("release first limit caller");
        let final_limit = scope.spawn(|| cache.set_size_limit(one_entry_limit));
        final_limit_queued.recv_timeout(Duration::from_secs(3)).expect("final limit entered the queue");
        resume_final_limit.send(()).expect("release final limit caller");
        let (put_queued, resume_put) = cache.pause_at(WritePoint::PutQueued);
        let put = scope.spawn(|| cache.put(key(201), vec![2; 1024]));
        put_queued.recv_timeout(Duration::from_secs(3)).expect("put entered the queue after both limits");
        resume_put.send(()).expect("release put caller");
        resume_data.send(()).expect("resume blocker write");
        assert_matches!(
            blocker.join().expect("blocker thread"),
            Err(StorageError::Io(error)) if error.kind() == std::io::ErrorKind::StorageFull
        );
        first_limit.join().expect("first limit thread").expect("set first cache limit");
        final_limit.join().expect("final limit thread").expect("set final cache limit");
        limit_started.recv_timeout(Duration::from_secs(3)).expect("limit command finished");
        assert_eq!(cache.entry_count(), 8, "the final limit is enforced before the following put");

        resume_limit.send(()).expect("resume writer after limit batch");
        put.join().expect("put thread").expect("later put uses the final limit");
    });
    assert_eq!(cache.entry_count(), 1);
    assert!(cache.accounted_size() <= cache.size_limit());
    cache.shutdown().expect("close cache");
}

#[test]
fn impossible_put_preserves_entries() {
    let empty_directory = tempdir().expect("empty cache directory");
    let limit = MIN_SIZE_LIMIT + 2 * 1024 * 1024;
    let empty_cache = Cache::open(empty_directory.path(), limit, &TEST_NAMESPACES).expect("open empty cache");
    let empty_size = empty_cache.accounted_size();
    empty_cache.shutdown().expect("close empty cache");

    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), limit, &TEST_NAMESPACES).expect("open cache");
    for id in 0..8u16 {
        cache.put(key(id), vec![1; 1024]).expect("store entry");
    }
    let entry_count = cache.entry_count();
    let evictions = cache.recent_evictions();
    let value_size = usize::try_from(limit - empty_size - 1).expect("test value fits usize");

    assert_matches!(cache.put(key(8), vec![2; value_size]), Err(StorageError::TooLarge));
    assert_eq!(cache.entry_count(), entry_count);
    assert_eq!(cache.recent_evictions(), evictions);
    for id in 0..8u16 {
        assert_matches!(cache.lookup(key(id)), Lookup::Hit(_));
    }
    cache.shutdown().expect("close cache");
}

#[test]
fn idle_enforcement_reaches_watermark() {
    const ENTRIES: u16 = 512;
    const ENTRY_BYTES: usize = 512;
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
    for id in 0..ENTRIES {
        cache.put(same_shard_key(id), vec![1; ENTRY_BYTES]).expect("fill cache");
    }
    let limit = cache.accounted_size();
    let watermark = test_low_watermark(limit);
    assert!(watermark < limit);
    let (limit_replied, resume_limit) = cache.pause_at(WritePoint::SetLimitReplied);
    cache.set_size_limit(limit).expect("set current cache limit");
    limit_replied.recv_timeout(Duration::from_secs(3)).expect("writer finished the limit command");

    let mut previous_count = cache.entry_count();
    let mut resume_batch = resume_limit;
    while cache.accounted_size() > watermark {
        let (batch_committed, resume_next) = cache.pause_at(WritePoint::IndexBatchCommitted);
        resume_batch.send(()).expect("resume limit command");
        batch_committed.recv_timeout(Duration::from_secs(3)).expect("idle enforcement committed a batch");
        let next_count = cache.entry_count();
        let removed = previous_count - next_count;
        assert!((1..=32).contains(&removed), "idle batch removed {removed} entries");
        previous_count = next_count;
        if cache.accounted_size() > watermark {
            resume_batch = resume_next;
        } else {
            resume_next.send(()).expect("resume final enforcement batch");
        }
    }

    assert!(cache.accounted_size() <= watermark);
    let evicted = usize::from(ENTRIES) - cache.entry_count();
    for id in 0..ENTRIES {
        if usize::from(id) < evicted {
            assert_eq!(cache.lookup(same_shard_key(id)), Lookup::Miss);
        } else {
            assert_matches!(cache.lookup(same_shard_key(id)), Lookup::Hit(_));
        }
    }
    cache.shutdown().expect("close cache");
}

#[test]
fn headroom_accepts_large_put() {
    const ENTRIES: u16 = 256;
    const ENTRY_BYTES: usize = 1024;
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache"));
    for id in 0..ENTRIES {
        cache.put(same_shard_key(id), vec![1; ENTRY_BYTES]).expect("fill cache");
    }
    let watermark = cache.accounted_size();
    let limit = watermark + watermark / 7;
    assert_eq!(test_low_watermark(limit), watermark);
    let (limit_replied, resume_limit) = cache.pause_at(WritePoint::SetLimitReplied);
    cache.set_size_limit(limit).expect("set limit at the watermark");
    limit_replied.recv_timeout(Duration::from_secs(3)).expect("writer finished the limit command");
    assert_eq!(cache.accounted_size(), watermark);

    let mut protections = Vec::new();
    for id in 0..ENTRIES {
        protections.push(cache.protect(same_shard_key(id)).expect("protect existing entry"));
    }
    let key = same_shard_key(u16::MAX);
    let key_protection = cache.protect(key).expect("protect new entry");
    let evictions = cache.recent_evictions();
    let value_size = usize::try_from(limit - watermark - 1024).expect("headroom fits usize");
    let (put_queued, resume_put) = cache.pause_at(WritePoint::PutQueued);
    let writing_cache = Arc::clone(&cache);
    let writer = thread::spawn(move || writing_cache.put(key, vec![2; value_size]));
    put_queued.recv_timeout(Duration::from_secs(3)).expect("put entered the queue");
    let (batch_committed, resume_commit) = cache.pause_at(WritePoint::IndexBatchCommitted);
    resume_put.send(()).expect("release put caller");
    resume_limit.send(()).expect("resume limit command");
    batch_committed.recv_timeout(Duration::from_secs(3)).expect("large put committed");

    assert_eq!(cache.entry_count(), usize::from(ENTRIES) + 1);
    assert!(cache.accounted_size() <= limit);
    assert_eq!(cache.recent_evictions(), evictions);
    assert_matches!(cache.lookup(key), Lookup::Hit(_));
    resume_commit.send(()).expect("resume writer");
    writer.join().expect("put thread completes").expect("headroom accepts the put");
    drop(key_protection);
    drop(protections);
    Arc::try_unwrap(cache).unwrap_or_else(|_| panic!("cache handles released")).shutdown().expect("close cache");
}

#[test]
fn over_budget_put_preserves_entries() {
    const ENTRIES: u16 = 256;
    const ENTRY_BYTES: usize = 1024;
    const LARGE_BYTES: usize = 96 * 1024;
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache"));
    for id in 0..ENTRIES {
        cache.put(same_shard_key(id), vec![1; ENTRY_BYTES]).expect("fill cache");
    }
    let limit = cache.accounted_size();
    let watermark = test_low_watermark(limit);
    assert!(u64::try_from(LARGE_BYTES + 1024).expect("test value fits u64") < limit - watermark);
    let (limit_replied, resumes) = cache.pause_many_at(WritePoint::SetLimitReplied, 2);
    cache.set_size_limit(limit).expect("set current cache limit");
    limit_replied.recv_timeout(Duration::from_secs(3)).expect("writer finished the first limit command");
    let evictions = cache.recent_evictions();

    let key = same_shard_key(u16::MAX);
    let (put_queued, resume_put) = cache.pause_at(WritePoint::PutQueued);
    let writing_cache = Arc::clone(&cache);
    let put = thread::spawn(move || writing_cache.put(key, vec![2; LARGE_BYTES]));
    put_queued.recv_timeout(Duration::from_secs(3)).expect("put entered the queue");
    let (limit_queued, resume_second_limit) = cache.pause_at(WritePoint::SetLimitQueued);
    let limit_cache = Arc::clone(&cache);
    let final_limit = thread::spawn(move || limit_cache.set_size_limit(limit));
    limit_queued.recv_timeout(Duration::from_secs(3)).expect("second limit entered the queue");
    resume_put.send(()).expect("release put caller");
    resume_second_limit.send(()).expect("release second limit caller");
    let mut limit_resumes = resumes.into_iter();
    limit_resumes.next().expect("first limit resume").send(()).expect("resume first limit command");

    assert_matches!(put.join().expect("put thread completes"), Err(StorageError::TooLarge));
    final_limit.join().expect("second limit thread completes").expect("set current cache limit");
    limit_replied.recv_timeout(Duration::from_secs(3)).expect("writer finished the second limit command");
    assert_eq!(cache.entry_count(), usize::from(ENTRIES));
    assert_eq!(cache.accounted_size(), limit);
    assert_eq!(cache.recent_evictions(), evictions);
    for id in 0..ENTRIES {
        assert_matches!(cache.lookup(same_shard_key(id)), Lookup::Hit(_));
    }
    limit_resumes.next().expect("second limit resume").send(()).expect("resume final limit command");
    Arc::try_unwrap(cache).unwrap_or_else(|_| panic!("cache handles released")).shutdown().expect("close cache");
}

#[test]
fn failed_limit_enforcement_keeps_the_previous_limit() {
    let single = tempdir().expect("single cache directory");
    let single_cache = Cache::open(single.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open single cache");
    single_cache.put(key(10), vec![1; 1024]).expect("store sample entry");
    let one_entry_limit = single_cache.accounted_size();
    single_cache.shutdown().expect("close single cache");

    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
    for id in [11, 12, 13] {
        cache.put(key(id), vec![1; 1024]).expect("store entry");
    }
    let old_limit = cache.size_limit();
    cache.arm_fault(WritePoint::IndexBatchFailure);

    assert_matches!(
        cache.set_size_limit(one_entry_limit),
        Err(StorageError::Io(error)) if error.kind() == std::io::ErrorKind::StorageFull
    );
    assert_eq!(cache.size_limit(), old_limit);
    assert_eq!(cache.entry_count(), 3);
    for id in [11, 12, 13] {
        assert_matches!(cache.lookup(key(id)), Lookup::Hit(_));
    }
    cache.shutdown().expect("close cache");
}

#[test]
fn protection_tracks_live_keys() {
    let directory = tempdir().expect("single cache directory");
    let single = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open single cache");
    single.put(key(10), vec![1; 64 * 1024]).expect("store sample entry");
    let one_entry_limit = single.accounted_size();
    single.shutdown().expect("close single cache");

    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
    for id in [50, 51] {
        cache.put(key(id), vec![u8::try_from(id).expect("small test key"); 64 * 1024]).expect("store entry");
    }
    let first = cache.protect(key(50)).expect("protect first key");
    let second = cache.protect(key(51)).expect("protect second key");
    assert_eq!(cache.protected_bytes(), 128 * 1024);

    cache.set_size_limit(one_entry_limit).expect("limit cache below live demand");
    assert_eq!(cache.entry_count(), 2);
    assert!(cache.accounted_size() > cache.size_limit());
    for _ in 0..3 {
        assert_matches!(cache.put(key(52), vec![3; 64 * 1024]), Err(StorageError::TooLarge));
    }
    assert_eq!(cache.entry_count(), 2);
    assert_eq!(cache.protected_bytes(), 128 * 1024);
    assert_matches!(cache.lookup(key(50)), Lookup::Hit(_));
    assert_matches!(cache.lookup(key(51)), Lookup::Hit(_));

    drop(first);
    assert_eq!(cache.protected_bytes(), 64 * 1024);
    cache.set_size_limit(one_entry_limit).expect("evict released key");
    assert_eq!(cache.entry_count(), 1);
    assert_eq!(cache.lookup(key(50)), Lookup::Miss);
    assert_matches!(cache.lookup(key(51)), Lookup::Hit(_));
    drop(second);
    assert_eq!(cache.protected_bytes(), 0);
    cache.shutdown().expect("close cache");
}

#[test]
fn protected_scan_reaches_unprotected_beyond_four_turns() {
    const ENTRY_BYTES: usize = 16 * 1024;
    const ENTRY_BYTES_U64: u64 = 16 * 1024;
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache"));
    let mut protections = Vec::new();
    for id in 0..513u16 {
        let value = u8::try_from(id % 256).expect("wrapped test value fits a byte");
        cache.put(key(id), vec![value; ENTRY_BYTES]).expect("store entry");
        if id < 512 {
            protections.push(cache.protect(key(id)).expect("protect live entry"));
        }
    }
    let current_size = cache.accounted_size();
    let (limit_replied, resumes) = cache.pause_many_at(WritePoint::SetLimitReplied, 2);
    cache.set_size_limit(current_size).expect("set limit to current size");
    limit_replied.recv_timeout(Duration::from_secs(3)).expect("writer finished the first limit command");

    let (put_queued, resume_put) = cache.pause_at(WritePoint::PutQueued);
    let writing_cache = Arc::clone(&cache);
    let put = thread::spawn(move || writing_cache.put(key(513), vec![1; ENTRY_BYTES]));
    put_queued.recv_timeout(Duration::from_secs(3)).expect("put entered the queue");
    let (limit_queued, resume_final_limit) = cache.pause_at(WritePoint::SetLimitQueued);
    let limit_cache = Arc::clone(&cache);
    let final_limit = thread::spawn(move || limit_cache.set_size_limit(current_size));
    limit_queued.recv_timeout(Duration::from_secs(3)).expect("second limit entered the queue");
    resume_put.send(()).expect("release put caller");
    resume_final_limit.send(()).expect("release second limit caller");
    let mut resumes = resumes.into_iter();
    resumes.next().expect("first limit resume").send(()).expect("resume first limit command");
    put.join().expect("put thread completes").expect("evict beyond four bounded scan turns");
    final_limit.join().expect("second limit thread completes").expect("set current cache limit");
    limit_replied.recv_timeout(Duration::from_secs(3)).expect("writer finished the second limit command");

    assert_eq!(cache.entry_count(), 513);
    assert_eq!(cache.protected_bytes(), 512 * ENTRY_BYTES_U64);
    for id in 0..512u16 {
        assert_matches!(cache.lookup(key(id)), Lookup::Hit(_));
    }
    assert_eq!(cache.lookup(key(512)), Lookup::Miss);
    assert_matches!(cache.lookup(key(513)), Lookup::Hit(_));
    resumes.next().expect("second limit resume").send(()).expect("resume final limit command");
    drop(protections);
    Arc::try_unwrap(cache).unwrap_or_else(|_| panic!("cache handles released")).shutdown().expect("close cache");
}

#[test]
fn protected_scan_stops_after_one_full_cycle() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache"));
    let mut protections = Vec::new();
    for id in 0..256u16 {
        let key = key(id);
        cache.put(key, vec![1; 16]).expect("store protected entry");
        protections.push(cache.protect(key).expect("protect entry"));
    }
    cache.set_size_limit(cache.accounted_size()).expect("set current size limit");

    let writing_cache = Arc::clone(&cache);
    let (reply, receiver) = std::sync::mpsc::channel();
    let writer = thread::spawn(move || {
        let result = writing_cache.put(key(256), vec![2; 16]);
        reply.send(result).expect("report put result");
    });
    let result = receiver.recv_timeout(Duration::from_secs(3)).expect("put finishes after one candidate cycle");
    assert_matches!(result, Err(StorageError::TooLarge));
    writer.join().expect("put thread completes");

    assert_eq!(cache.entry_count(), 256);
    drop(protections);
    Arc::try_unwrap(cache).unwrap_or_else(|_| panic!("cache handles released")).shutdown().expect("close cache");
}

#[test]
fn lru_eviction_stays_oldest_first_across_scan_windows() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache"));
    for id in 0..256u16 {
        cache.put(same_shard_key(id), vec![u8::try_from(id % 251).expect("payload byte")]).expect("store entry");
    }

    let limit = cache.accounted_size().checked_sub(1).expect("cache size is nonzero");
    let (limit_replied, resumes) = cache.pause_many_at(WritePoint::SetLimitReplied, 2);
    cache.set_size_limit(limit).expect("evict oldest entry");
    limit_replied.recv_timeout(Duration::from_secs(3)).expect("writer finished the first limit command");

    let (put_queued, resume_put) = cache.pause_at(WritePoint::PutQueued);
    let writing_cache = Arc::clone(&cache);
    let put = thread::spawn(move || writing_cache.put(same_shard_key(256), vec![0]));
    put_queued.recv_timeout(Duration::from_secs(3)).expect("put entered the queue");
    let (limit_queued, resume_final_limit) = cache.pause_at(WritePoint::SetLimitQueued);
    let limit_cache = Arc::clone(&cache);
    let final_limit = thread::spawn(move || limit_cache.set_size_limit(limit));
    limit_queued.recv_timeout(Duration::from_secs(3)).expect("second limit entered the queue");
    resume_put.send(()).expect("release put caller");
    resume_final_limit.send(()).expect("release second limit caller");
    let mut resumes = resumes.into_iter();
    resumes.next().expect("first limit resume").send(()).expect("resume first limit command");
    put.join().expect("put thread completes").expect("evict the next oldest entry");
    final_limit.join().expect("second limit thread completes").expect("set current cache limit");
    limit_replied.recv_timeout(Duration::from_secs(3)).expect("writer finished the second limit command");

    assert_eq!(cache.lookup(same_shard_key(0)), Lookup::Miss);
    assert_eq!(cache.lookup(same_shard_key(1)), Lookup::Miss);
    for id in 2..256u16 {
        assert_matches!(cache.lookup(same_shard_key(id)), Lookup::Hit(_));
    }
    assert_matches!(cache.lookup(same_shard_key(256)), Lookup::Hit(_));
    assert_eq!(cache.entry_count(), 255);
    resumes.next().expect("second limit resume").send(()).expect("resume final limit command");
    Arc::try_unwrap(cache).unwrap_or_else(|_| panic!("cache handles released")).shutdown().expect("close cache");
}

#[test]
fn staged_replacement_can_be_evicted_and_restored_in_one_batch() {
    const ENTRY_BYTES: usize = 256 * 1024;
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
    cache.put(key(70), vec![7; ENTRY_BYTES]).expect("store original A");
    let watermark = cache.accounted_size();
    let limit = watermark + watermark / 7;
    assert_eq!(test_low_watermark(limit), watermark);
    cache.set_size_limit(limit).expect("limit cache to one entry");
    let (started, resume) = cache.pause_at(WritePoint::DataTempCreated);
    cache.arm_fault(WritePoint::DataWriteFailure);

    thread::scope(|scope| {
        let blocker = scope.spawn(|| cache.put(key(900), vec![0; ENTRY_BYTES]));
        started.recv_timeout(Duration::from_secs(3)).expect("blocker reached its data write");
        let mut replies = Vec::new();
        for (key, value) in [(key(70), 1), (key(71), 2), (key(70), 3)] {
            let (reply, receiver) = std::sync::mpsc::sync_channel(0);
            let bytes = vec![value; ENTRY_BYTES];
            cache.queue.try_send(writer::Command::Put { key, bytes, reply }, ENTRY_BYTES).expect("queue staged put");
            replies.push(receiver);
        }
        resume.send(()).expect("resume blocker write");
        assert_matches!(
            blocker.join().expect("blocker thread"),
            Err(StorageError::Io(error)) if error.kind() == std::io::ErrorKind::StorageFull
        );
        for receiver in replies {
            receiver.recv_timeout(Duration::from_secs(3)).expect("staged put reply").expect("commit staged put");
        }
    });

    assert_matches!(cache.lookup(key(70)), Lookup::Hit(CacheValue { bytes, .. }) if bytes == vec![3; ENTRY_BYTES]);
    assert_eq!(cache.lookup(key(71)), Lookup::Miss);
    assert_eq!(cache.entry_count(), 1);
    cache.shutdown().expect("close cache");
}

#[test]
fn protection_applies_to_later_writes() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
    let protection = cache.protect(key(60)).expect("protect missing key");
    assert_eq!(cache.protected_bytes(), 0);

    cache.put(key(60), vec![7; 1024]).expect("write protected key");
    assert_eq!(cache.protected_bytes(), 1024);

    drop(protection);
    assert_eq!(cache.protected_bytes(), 0);
    cache.shutdown().expect("close cache");
}
