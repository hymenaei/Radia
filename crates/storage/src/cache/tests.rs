use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
};

use proptest::{prelude::*, test_runner::RngSeed};
use sha2::{Digest, Sha256};
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
        .filter_map(Result::ok)
        .map(|item| item.path())
        .filter(|path| {
            path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("data-") && name.to_string_lossy().ends_with(".bin"))
        })
        .collect()
}

fn crash_points() -> [WritePoint; 7] {
    [
        WritePoint::DataTempCreated,
        WritePoint::DataWritePartial,
        WritePoint::DataWritten,
        WritePoint::DataRenamed,
        WritePoint::JournalAppendStarted,
        WritePoint::JournalAppendPartial,
        WritePoint::JournalAppended,
    ]
}

fn compact_points() -> [WritePoint; 8] {
    [
        WritePoint::SnapshotTempCreated,
        WritePoint::SnapshotWritten,
        WritePoint::SnapshotSynced,
        WritePoint::SnapshotRenamed,
        WritePoint::JournalResetTempCreated,
        WritePoint::JournalResetWritten,
        WritePoint::JournalResetSynced,
        WritePoint::JournalResetRenamed,
    ]
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
    assert_eq!(usage.load(Ordering::Relaxed), u64::MAX);
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
    cache.flush().expect("remove corrupted entry");
    assert_eq!(cache.lookup(key(1)), Lookup::Miss);
    cache.shutdown().expect("close cache after removal");
}

#[test]
fn lookup_detects_truncated_and_missing_data() {
    for missing in [false, true] {
        let directory = tempdir().expect("cache directory");
        let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache");
        cache.put(key(2), b"payload bytes".to_vec()).expect("store payload");
        cache.shutdown().expect("close cache");
        let file = data_files(directory.path()).into_iter().next().expect("payload file");
        if missing {
            fs::remove_file(&file).expect("remove payload");
        } else {
            let bytes = fs::read(&file).expect("read payload");
            let truncated = bytes.get(..bytes.len().saturating_sub(1)).expect("truncated payload range");
            fs::write(&file, truncated).expect("truncate payload");
        }

        let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open without data rescan");
        assert_eq!(cache.entry_count(), 1, "startup leaves payload validation to lookup");
        assert_eq!(cache.lookup(key(2)), Lookup::Corrupt);
        cache.flush().expect("remove damaged entry");
        assert_eq!(cache.entry_count(), 0);
        cache.shutdown().expect("close repaired cache");
    }
}

#[test]
fn recovered_entry_accepts_replacement() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache");
    cache.arm_fault(WritePoint::JournalAppended);
    assert!(cache.put(key(3), b"recovered payload".to_vec()).is_err());
    cache.simulate_crash().expect("simulate process crash");

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("recover cache");
    cache.put(key(3), b"replacement payload".to_vec()).expect("replace recovered entry");
    std::assert_matches!(cache.lookup(key(3)), Lookup::Hit(CacheValue { bytes, .. }) if bytes == b"replacement payload");
    cache.shutdown().expect("close cache");
}

#[test]
fn recovery_discards_extra_files() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache");
    cache.shutdown().expect("close cache");
    let extra = directory.path().join(DATA_DIRECTORY).join("stray.bin");
    fs::write(&extra, b"unindexed").expect("create extra file");

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache with extra file");
    cache.flush().expect("sweep unreferenced file");
    assert!(!extra.exists());
    assert_eq!(cache.entry_count(), 0);
    cache.shutdown().expect("close cache");
}

#[test]
fn recovery_discards_unreadable_and_incompatible_snapshots() {
    for incompatible in [false, true] {
        let directory = tempdir().expect("cache directory");
        let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache");
        cache.put(key(4), b"payload".to_vec()).expect("store payload");
        cache.shutdown().expect("close cache");
        let snapshot_path = directory.path().join(SNAPSHOT_FILE);
        if incompatible {
            let mut snapshot = fs::read(&snapshot_path).expect("read snapshot");
            let checksum_start = snapshot.len().saturating_sub(journal::SNAPSHOT_CHECKSUM_BYTES);
            let version = snapshot.get_mut(8..12).expect("snapshot version bytes");
            version.copy_from_slice(&3u32.to_le_bytes());
            let checksum: [u8; 32] = Sha256::digest(snapshot.get(..checksum_start).expect("snapshot content")).into();
            snapshot.get_mut(checksum_start..).expect("snapshot checksum").copy_from_slice(&checksum);
            fs::write(&snapshot_path, snapshot).expect("write incompatible version");
        } else {
            fs::write(&snapshot_path, b"short").expect("truncate snapshot");
        }

        let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("discard unreadable cache");
        assert_eq!(cache.entry_count(), 0);
        cache.flush().expect("sweep discarded data");
        assert!(data_files(directory.path()).is_empty());
        cache.shutdown().expect("close repaired cache");
    }
}

#[test]
fn size_limit_evicts_least_recently_used() {
    let single = tempdir().expect("single cache directory");
    let single_cache = Cache::open(single.path(), DEFAULT_SIZE_LIMIT).expect("open single cache");
    single_cache.put(key(10), vec![1; 64]).expect("store sample entry");
    single_cache.flush().expect("compact sample entry");
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
fn lookup_retries_after_replacement_deletes_old_file() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache"));
    cache.put(key(21), b"old payload".to_vec()).expect("store old payload");
    let (started, resume) = cache.pause_at(WritePoint::LookupEntrySelected);

    thread::scope(|scope| {
        let reading_cache = Arc::clone(&cache);
        let reader = scope.spawn(move || reading_cache.lookup(key(21)));
        started.recv_timeout(Duration::from_secs(3)).expect("reader selected old entry");
        cache.put(key(21), b"replacement payload".to_vec()).expect("replace entry");
        resume.send(()).expect("resume reader");
        std::assert_matches!(reader.join().expect("reader completes"), Lookup::Hit(CacheValue { bytes, .. }) if bytes == b"replacement payload");
    });

    Arc::try_unwrap(cache).unwrap_or_else(|_| panic!("cache handles released")).shutdown().expect("close cache");
}

#[test]
fn write_failure_is_nonfatal() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache");
    cache.put(key(40), b"existing payload".to_vec()).expect("store existing payload");
    cache.arm_fault(WritePoint::DataWriteFailure);
    std::assert_matches!(cache.put(key(41), b"disk full payload".to_vec()), Err(StorageError::Io(error)) if error.kind() == io::ErrorKind::StorageFull);
    std::assert_matches!(cache.lookup(key(40)), Lookup::Hit(CacheValue { bytes, .. }) if bytes == b"existing payload");
    assert_eq!(cache.lookup(key(41)), Lookup::Miss);
    cache.shutdown().expect("close cache after failed write");
}

#[test]
fn torn_journal_tail_is_truncated() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache");
    cache.arm_fault(WritePoint::JournalAppendPartial);
    assert!(cache.put(key(42), b"torn record".to_vec()).is_err());
    cache.simulate_crash().expect("simulate process crash");

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("truncate torn journal");
    assert_eq!(cache.lookup(key(42)), Lookup::Miss);
    assert_eq!(fs::metadata(directory.path().join(JOURNAL_FILE)).expect("journal metadata").len(), journal::JOURNAL_HEADER_BYTES);
    cache.shutdown().expect("close recovered cache");
}

#[test]
fn crashes_recover_each_journal_and_compaction_step() {
    for point in crash_points() {
        let directory = tempdir().expect("cache directory");
        let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache");
        cache.arm_fault(point);
        assert!(cache.put(key(43), b"recovery payload".to_vec()).is_err(), "{point:?}");
        cache.simulate_crash().expect("stop writer at simulated crash");

        let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("recover cache");
        let result = cache.lookup(key(43));
        if point == WritePoint::JournalAppended {
            let entries = match cache.read_state.entries.read() {
                Ok(entries) => entries,
                Err(poisoned) => poisoned.into_inner(),
            };
            let entry = entries.get(&key(43)).expect("journal entry exists");
            let path = entry.path(&cache.read_state.data_dir);
            assert!(path.exists(), "{point:?}: {path:?}");
            let bytes = fs::read(&path).expect("read recovered payload");
            assert_eq!(u64::try_from(bytes.len()).ok(), Some(entry.size));
            assert_eq!(Sha256::digest(&bytes).as_slice(), entry.hash);
            std::assert_matches!(result, Lookup::Hit(CacheValue { bytes, .. }) if bytes == b"recovery payload");
        } else {
            assert_eq!(result, Lookup::Miss, "{point:?}");
        }
        assert!(cache.accounted_size() <= cache.size_limit());
        cache.shutdown().expect("close recovered cache");
    }

    for point in compact_points() {
        let directory = tempdir().expect("cache directory");
        let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache");
        cache.put(key(44), b"compacted payload".to_vec()).expect("write before compaction");
        cache.arm_fault(point);
        assert!(cache.flush().is_err(), "{point:?}");
        cache.simulate_crash().expect("stop writer during compaction");

        let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("recover compacted cache");
        match cache.lookup(key(44)) {
            Lookup::Hit(value) => assert_eq!(value.bytes, b"compacted payload", "{point:?}"),
            actual => panic!("{point:?}: {actual:?}"),
        }
        cache.flush().expect("finish compaction");
        cache.shutdown().expect("close recovered cache");
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 12, rng_seed: RngSeed::Fixed(0x5eed), ..ProptestConfig::default() })]

    #[test]
    fn stateful_operations_preserve_recovery_and_limits(
        operations in proptest::collection::vec((0u8..9, 0u16..12, proptest::collection::vec(any::<u8>(), 0..33), 0u8..14), 1..24)
    ) {
        let directory = tempdir().expect("cache directory");
        let mut cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("open cache");
        let mut model = HashMap::<u16, Vec<u8>>::new();

        for (action, id, bytes, fault) in operations {
            match action {
                0 => match cache.put(key(id), bytes.clone()) {
                    Ok(()) => { model.insert(id, bytes); }
                    Err(StorageError::TooLarge) => {}
                    Err(error) => prop_assert!(false, "unexpected put result: {error}"),
                },
                1 => check_model(&cache, &model)?,
                2 => {
                    cache.set_size_limit(MIN_SIZE_LIMIT).expect("lower cache limit");
                    reconcile_after_eviction(&cache, &mut model)?;
                    cache.set_size_limit(DEFAULT_SIZE_LIMIT).expect("restore cache limit");
                }
                3 => {
                    cache.clear().expect("clear cache");
                    model.clear();
                }
                4 => cache.set_size_limit(DEFAULT_SIZE_LIMIT).expect("raise cache limit"),
                5 => {
                    let points = crash_points();
                    let selected = points.get(usize::from(fault) % points.len()).copied().expect("crash point");
                    cache.arm_fault(selected);
                    let result = cache.put(key(id), bytes.clone());
                    prop_assert!(result.is_err(), "injected write failure is observed");
                    cache.simulate_crash().expect("simulate process crash");
                    if selected == WritePoint::JournalAppended {
                        model.insert(id, bytes);
                    }
                    cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("recover after journal crash");
                }
                6 => {
                    let points = compact_points();
                    let selected = points.get(usize::from(fault) % points.len()).copied().expect("compaction point");
                    cache.arm_fault(selected);
                    prop_assert!(cache.flush().is_err(), "injected compaction failure is observed");
                    cache.simulate_crash().expect("simulate compaction crash");
                    cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("recover after compaction crash");
                }
                7 => {
                    let removed = cache.remove(key(id)).expect("remove entry");
                    prop_assert_eq!(removed, model.remove(&id).is_some());
                }
                _ => check_model(&cache, &model)?,
            }

            prop_assert!(cache.accounted_size() <= cache.size_limit());
            prop_assert_eq!(cache.entry_count(), model.len());
            check_model(&cache, &model)?;
            cache.shutdown().expect("close cache after operation");
            cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT).expect("reopen cache after operation");
            prop_assert!(cache.accounted_size() <= cache.size_limit());
            prop_assert_eq!(cache.entry_count(), model.len());
            check_model(&cache, &model)?;
        }

        let small_limit = MIN_SIZE_LIMIT + 4096;
        cache.set_size_limit(small_limit).expect("lower cache limit");
        reconcile_after_eviction(&cache, &mut model)?;
        for _ in 0..160 {
            cache.clear().expect("clear small cache");
            model.clear();
            prop_assert!(cache.accounted_size() <= small_limit);
            prop_assert_eq!(cache.entry_count(), model.len());
        }
        for revision in 0..80u8 {
            let bytes = vec![revision; 32];
            cache.arm_fault(WritePoint::JournalAppended);
            let error = cache.put(key(0), bytes.clone()).expect_err("simulated crash after journal append");
            std::assert_matches!(error, StorageError::Io(error) if error.kind() == io::ErrorKind::Interrupted);
            cache.simulate_crash().expect("simulate replacement crash");
            model.insert(0, bytes);
            cache = Cache::open(directory.path(), small_limit).expect("reopen small cache after replacement");
            prop_assert!(cache.accounted_size() <= small_limit);
            check_model(&cache, &model)?;
        }
        cache.shutdown().expect("close final cache");
    }
}

fn check_model(cache: &Cache, model: &HashMap<u16, Vec<u8>>) -> Result<(), TestCaseError> {
    for candidate in 0..12 {
        match (model.get(&candidate), cache.lookup(key(candidate))) {
            (Some(expected), Lookup::Hit(value)) => prop_assert_eq!(value.bytes.as_slice(), expected.as_slice()),
            (None, Lookup::Miss) => {}
            (Some(expected), actual) => prop_assert!(false, "expected cached bytes {expected:?}, got {actual:?}"),
            (None, actual) => prop_assert!(false, "expected miss, got {actual:?}"),
        }
    }
    Ok(())
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
