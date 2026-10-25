use std::{
    fs,
    io::{Seek, SeekFrom, Write},
    sync::Arc,
    thread,
    time::Duration,
};

use proptest::prelude::*;
use tempfile::tempdir;

use super::*;

fn key(value: u16) -> Key {
    let mut bytes = [0; 32];
    bytes[..2].copy_from_slice(&value.to_le_bytes());
    Key::new(bytes)
}

fn data_files(directory: &Path) -> Vec<PathBuf> {
    fs::read_dir(directory.join(DATA_DIRECTORY))
        .expect("read cache data directory")
        .map(|item| item.expect("read cache data entry").path())
        .collect()
}

fn point(value: u8) -> WritePoint {
    match value {
        0 => WritePoint::DataTempCreated,
        1 => WritePoint::DataWritten,
        2 => WritePoint::DataSynced,
        3 => WritePoint::DataRenamed,
        4 => WritePoint::IndexTempCreated,
        5 => WritePoint::IndexWritten,
        6 => WritePoint::IndexSynced,
        _ => WritePoint::IndexRenamed,
    }
}

#[test]
fn lock_is_exclusive_across_processes() {
    let directory = tempdir().expect("cache directory");
    let first = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open first cache");

    let status = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .arg("--exact")
        .arg("cache::tests::lock_helper")
        .arg("--nocapture")
        .env("RADIA_STORAGE_LOCK_TEST_DIR", directory.path())
        .status()
        .expect("start child test process");
    assert!(status.success());
    assert!(directory.path().join("child-lock-checked").exists());

    first.shutdown().expect("close first cache");
    let second = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("reopen cache");
    second.shutdown().expect("close reopened cache");
}

#[test]
fn lock_helper() {
    let Some(directory) = std::env::var_os("RADIA_STORAGE_LOCK_TEST_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    std::assert_matches!(Cache::open(&directory, DEFAULT_SIZE_LIMIT), Err(StorageError::InUse));
    fs::write(directory.join("child-lock-checked"), []).expect("record child lock check");
}

#[test]
fn usage_metadata_does_not_wrap() {
    let usage = std::sync::atomic::AtomicU64::new(u64::MAX);
    assert_eq!(saturating_increment(&usage), u64::MAX);
    assert_eq!(usage.load(std::sync::atomic::Ordering::Relaxed), u64::MAX);
}

#[test]
fn lookup_checks_hash_and_removes_corruption() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache");
    cache.put(key(1), b"payload bytes".to_vec()).expect("store payload");
    cache.shutdown().expect("close cache");
    let file = data_files(directory.path()).into_iter().next().expect("payload file");
    let mut bytes = fs::read(&file).expect("read payload file");
    let byte = bytes.get_mut(0).expect("first payload byte");
    *byte ^= 0xff;
    fs::write(file, bytes).expect("flip payload byte");

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("reopen cache");
    assert_eq!(cache.lookup(key(1)), Lookup::Corrupt);
    cache.shutdown().expect("close cache");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("reopen repaired cache");
    assert_eq!(cache.lookup(key(1)), Lookup::Miss);
    assert_eq!(cache.entry_count(), 0);
    cache.shutdown().expect("close repaired cache");
}

#[test]
fn recovery_discards_truncated_and_missing_data() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache");
    cache.put(key(2), b"payload bytes".to_vec()).expect("store payload");
    cache.shutdown().expect("close cache");
    let file = data_files(directory.path()).into_iter().next().expect("payload file");
    let bytes = fs::read(&file).expect("read payload file");
    let truncated = bytes.get(..bytes.len().saturating_sub(1)).expect("truncated payload range");
    fs::write(&file, truncated).expect("truncate payload");

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open after truncation");
    assert_eq!(cache.entry_count(), 0);
    assert_eq!(cache.lookup(key(2)), Lookup::Miss);
    cache.shutdown().expect("close after truncation");

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("reopen cache");
    cache.put(key(3), b"another payload".to_vec()).expect("store another payload");
    cache.shutdown().expect("close cache");
    let file = data_files(directory.path()).into_iter().next().expect("payload file");
    fs::remove_file(file).expect("remove payload file");

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open after data removal");
    assert_eq!(cache.entry_count(), 0);
    assert_eq!(cache.lookup(key(3)), Lookup::Miss);
    cache.shutdown().expect("close after missing data");
}

#[test]
fn recovery_discards_extra_files() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache");
    cache.shutdown().expect("close cache");
    let extra = directory.path().join(DATA_DIRECTORY).join("stray.bin");
    fs::write(&extra, b"unindexed").expect("create extra file");

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache with extra file");
    assert!(!extra.exists());
    assert_eq!(cache.entry_count(), 0);
    cache.shutdown().expect("close cache");
}

#[test]
fn recovery_discards_unreadable_and_incompatible_indexes() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache");
    cache.put(key(4), b"payload".to_vec()).expect("store payload");
    cache.shutdown().expect("close cache");
    let index_path = directory.path().join(INDEX_FILE);
    fs::write(&index_path, b"short").expect("truncate index");

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("recover short index");
    assert_eq!(cache.entry_count(), 0);
    assert!(data_files(directory.path()).is_empty());
    cache.shutdown().expect("close repaired cache");

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("reopen cache");
    cache.put(key(5), b"payload".to_vec()).expect("store payload");
    cache.shutdown().expect("close cache");
    let mut index = fs::OpenOptions::new().write(true).open(&index_path).expect("open index");
    index.seek(SeekFrom::Start(8)).expect("seek to version");
    index.write_all(&2u32.to_le_bytes()).expect("write incompatible version");
    index.sync_all().expect("sync incompatible version");
    drop(index);

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("recover incompatible index");
    assert_eq!(cache.entry_count(), 0);
    assert!(data_files(directory.path()).is_empty());
    cache.shutdown().expect("close repaired cache");
}

#[test]
fn size_limit_evicts_least_recently_used() {
    let single = tempdir().expect("single cache directory");
    let single_cache = Cache::open(single.path(), DEFAULT_SIZE_LIMIT).expect("open single cache");
    single_cache.put(key(10), vec![1; 64]).expect("store sample entry");
    let one_entry_limit = single_cache.accounted_size();
    single_cache.shutdown().expect("close single cache");

    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache");
    for id in [11, 12, 13] {
        cache.put(key(id), vec![1; 64]).expect("store entry");
    }
    std::assert_matches!(cache.lookup(key(12)), Lookup::Hit(_));
    cache.set_size_limit(one_entry_limit).expect("shrink cache");
    assert_eq!(cache.entry_count(), 1);
    let Lookup::Hit(value) = cache.lookup(key(12)) else {
        panic!("most recently used entry remains");
    };
    assert_eq!(value.bytes, vec![1; 64]);
    assert_eq!(value.use_count, 2);
    assert_eq!(cache.lookup(key(11)), Lookup::Miss);
    assert_eq!(cache.lookup(key(13)), Lookup::Miss);
    assert!(cache.accounted_size() <= cache.size_limit());
    cache.clear().expect("clear cache");
    assert_eq!(cache.entry_count(), 0);
    cache.shutdown().expect("close cache");
}

#[test]
fn lookup_does_not_wait_for_a_slow_write() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache"));
    let (started, resume) = cache.pause_at(WritePoint::DataWritten);

    thread::scope(|scope| {
        let writing_cache = Arc::clone(&cache);
        let writer = scope.spawn(move || writing_cache.put(key(20), b"slow payload".to_vec()));
        started.recv_timeout(Duration::from_secs(3)).expect("writer reached pause");
        let reading_cache = Arc::clone(&cache);
        let reader = scope.spawn(move || reading_cache.lookup(key(21)));
        assert_eq!(reader.join().expect("reader completes"), Lookup::Miss);
        resume.send(()).expect("release writer");
        writer.join().expect("writer completes").expect("put payload");
    });

    Arc::try_unwrap(cache).unwrap_or_else(|_| panic!("cache handles released")).shutdown().expect("close cache");
}

#[test]
fn crashes_recover_each_write_step() {
    let points = [
        WritePoint::DataTempCreated,
        WritePoint::DataWritten,
        WritePoint::DataSynced,
        WritePoint::DataRenamed,
        WritePoint::IndexTempCreated,
        WritePoint::IndexWritten,
        WritePoint::IndexSynced,
        WritePoint::IndexRenamed,
    ];
    for point in points {
        let directory = tempdir().expect("cache directory");
        let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache");
        cache.arm_fault(point);
        assert!(cache.put(key(30), b"recovery payload".to_vec()).is_err(), "{point:?}");
        cache.simulate_crash().expect("stop writer at simulated crash");

        let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("recover cache");
        let expected = if point == WritePoint::IndexRenamed {
            Lookup::Hit(CacheValue { bytes: b"recovery payload".to_vec(), last_use: 2, use_count: 1 })
        } else {
            Lookup::Miss
        };
        assert_eq!(cache.lookup(key(30)), expected, "{point:?}");
        assert!(cache.accounted_size() <= cache.size_limit());
        cache.shutdown().expect("close recovered cache");
    }
}

#[test]
fn failed_data_write_preserves_existing_entry() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache");
    cache.put(key(40), b"existing payload".to_vec()).expect("store existing payload");
    cache.arm_fault(WritePoint::DataWritten);
    assert!(cache.put(key(41), b"disk full payload".to_vec()).is_err());
    cache.simulate_crash().expect("simulate failed write crash");

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("recover cache");
    std::assert_matches!(cache.lookup(key(40)), Lookup::Hit(CacheValue { bytes, .. }) if bytes == b"existing payload");
    assert_eq!(cache.lookup(key(41)), Lookup::Miss);
    assert_eq!(data_files(directory.path()).len(), 1, "unreferenced failed-write files are removed");
    cache.shutdown().expect("close cache");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]

    #[test]
    fn stateful_operations_preserve_recovery_and_limits(
        operations in proptest::collection::vec((0u8..7, 0u16..12, proptest::collection::vec(any::<u8>(), 0..33), 0u8..8), 1..24)
    ) {
        let directory = tempdir().expect("cache directory");
        let mut cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache");
        let mut model = HashMap::<u16, Vec<u8>>::new();

        for (action, id, bytes, fault) in operations {
            match action {
                0 => {
                    cache.put(key(id), bytes.clone()).expect("put model value");
                    model.insert(id, bytes);
                }
                1 => {
                    let expected = model.get(&id).cloned();
                    match (expected, cache.lookup(key(id))) {
                        (Some(bytes), Lookup::Hit(value)) => prop_assert_eq!(value.bytes, bytes),
                        (None, Lookup::Miss) => {}
                        (Some(expected), actual) => prop_assert!(false, "expected cached bytes {expected:?}, got {actual:?}"),
                        (None, actual) => prop_assert!(false, "expected miss, got {actual:?}"),
                    }
                }
                2 => {
                    cache.set_size_limit(MIN_SIZE_LIMIT).expect("lower cache limit");
                    reconcile_after_eviction(&cache, &mut model)?;
                    cache.set_size_limit(DEFAULT_SIZE_LIMIT).expect("restore cache limit");
                }
                3 => {
                    cache.clear().expect("clear cache");
                    model.clear();
                }
                4 => {
                    if fault == 0 {
                        cache.set_size_limit(MIN_SIZE_LIMIT).expect("lower cache limit");
                        reconcile_after_eviction(&cache, &mut model)?;
                    } else {
                        cache.set_size_limit(DEFAULT_SIZE_LIMIT).expect("raise cache limit");
                    }
                }
                5 => {
                    let selected = point(fault);
                    cache.arm_fault(selected);
                    prop_assert!(cache.put(key(id), bytes.clone()).is_err());
                    cache.simulate_crash().expect("simulate process crash");
                    if selected == WritePoint::IndexRenamed {
                        model.insert(id, bytes);
                    }
                    cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("recover after simulated crash");
                }
                _ => {
                    let removed = cache.remove(key(id)).expect("remove entry");
                    prop_assert_eq!(removed, model.remove(&id).is_some());
                }
            }

            prop_assert!(cache.accounted_size() <= cache.size_limit());
            prop_assert_eq!(cache.entry_count(), model.len());
            cache.shutdown().expect("close cache after operation");
            cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("reopen cache after operation");
            prop_assert!(cache.accounted_size() <= cache.size_limit());
            prop_assert_eq!(cache.entry_count(), model.len());
            for candidate in 0..12 {
                let expected = model.get(&candidate).cloned();
                match (expected, cache.lookup(key(candidate))) {
                    (Some(bytes), Lookup::Hit(value)) => prop_assert_eq!(value.bytes, bytes),
                    (None, Lookup::Miss) => {}
                    (Some(expected), actual) => prop_assert!(false, "expected cached bytes {expected:?}, got {actual:?}"),
                    (None, actual) => prop_assert!(false, "expected miss, got {actual:?}"),
                }
            }
        }
        cache.shutdown().expect("close final cache");
    }
}

fn reconcile_after_eviction(cache: &Cache, model: &mut HashMap<u16, Vec<u8>>) -> Result<(), TestCaseError> {
    let previous = std::mem::take(model);
    for (id, bytes) in previous {
        match cache.lookup(key(id)) {
            Lookup::Hit(value) => {
                prop_assert_eq!(value.bytes.as_slice(), bytes.as_slice());
                model.insert(id, bytes);
            }
            Lookup::Miss => {}
            actual => prop_assert!(false, "expected hit or eviction, got {actual:?}"),
        }
    }
    Ok(())
}
