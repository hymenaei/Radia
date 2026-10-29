use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
};

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

use crate::sqlite::MAX_BATCH_ROWS;
use proptest::{prelude::*, test_runner::RngSeed};
use rusqlite::Connection;
use tempfile::tempdir;

use super::*;

const TEST_NAMESPACES: [Namespace; 2] = [Namespace::new("tests/storage", "bin"), Namespace::new("assets/textures", "j2c")];
const TEST_NAMESPACE: NamespaceId = NamespaceId::new(0);

fn key(value: u16) -> Key {
    let mut name = [0; 16];
    name[..2].copy_from_slice(&value.to_le_bytes());
    Key::new(TEST_NAMESPACE, name)
}

fn data_file(directory: &Path, key: Key) -> PathBuf {
    NamespaceTable::new(&TEST_NAMESPACES).expect("valid test namespaces").path(directory, key).expect("valid test key")
}

fn crash_points() -> [WritePoint; 6] {
    [
        WritePoint::DataTempCreated,
        WritePoint::DataWritePartial,
        WritePoint::DataWritten,
        WritePoint::DataRenamed,
        WritePoint::IndexBatchStarted,
        WritePoint::IndexBatchCommitted,
    ]
}

#[test]
fn lock_is_exclusive_across_processes() {
    let directory = tempdir().expect("cache directory");
    let first = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open first cache");

    let status = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .arg("--exact")
        .arg("cache::tests::lock_helper")
        .arg("--nocapture")
        .env("RADIA_STORAGE_LOCK_TEST_DIR", directory.path())
        .status()
        .expect("start child test process");
    assert!(status.success());
    let marker = directory.path().with_extension("lock-checked");
    assert!(marker.exists());
    fs::remove_file(marker).expect("remove lock check marker");

    first.shutdown().expect("close first cache");
    let second = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("reopen cache");
    second.shutdown().expect("close reopened cache");
}

#[test]
fn lock_helper() {
    let Some(directory) = std::env::var_os("RADIA_STORAGE_LOCK_TEST_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    std::assert_matches!(Cache::open(&directory, DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES), Err(StorageError::InUse));
    fs::write(directory.with_extension("lock-checked"), []).expect("record child lock check");
}

#[test]
fn key_paths_are_sharded_and_validated() {
    let name = [0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89];
    let cache_key = Key::new(NamespaceId::new(1), name);
    assert_eq!(
        data_file(Path::new("cache"), cache_key),
        PathBuf::from("cache/assets/textures/ab/abcdef01-2345-6789-abcd-ef0123456789.j2c")
    );
    let directory = tempdir().expect("invalid namespace cache root");
    for namespace in ["assets/../outside", "assets\\textures"] {
        std::assert_matches!(
            Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &[Namespace::new(namespace, "bin")]),
            Err(StorageError::InvalidNamespaces)
        );
    }
    std::assert_matches!(
        Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &[Namespace::new("assets/textures", "LLSD")]),
        Err(StorageError::InvalidNamespaces)
    );
}

#[test]
fn small_namespace_accepts_lower_limit() {
    let directory = tempdir().expect("cache directory");
    let namespaces = [Namespace::new("items", "bin")];
    let cache = Cache::open(directory.path(), MIN_SIZE_LIMIT - 1, &namespaces).expect("open with namespace-sized limit");
    cache.shutdown().expect("close cache");
}

#[test]
fn small_size_limit_accounts_for_bounded_wal_and_shm() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempdir()?;
    let size_limit = MIN_SIZE_LIMIT + 8192;
    let cache = Cache::open(directory.path(), size_limit, &TEST_NAMESPACES)?;

    for value in 0..40 {
        cache.put(key(value), vec![value as u8; 128])?;
        assert!(cache.accounted_size() <= size_limit);
    }
    cache.flush()?;

    let wal_bytes = match fs::metadata(directory.path().join("index.sqlite-wal")) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error.into()),
    };
    let shm_bytes = match fs::metadata(directory.path().join("index.sqlite-shm")) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error.into()),
    };
    assert!(wal_bytes <= index::INDEX_WAL_LIMIT_BYTES);
    assert!(wal_bytes + shm_bytes <= index::INDEX_AUXILIARY_RESERVATION_BYTES);
    assert!(cache.accounted_size() <= size_limit);

    cache.shutdown()?;
    Ok(())
}

#[test]
fn index_schema_stores_verified_rows() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempdir()?;
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES)?;
    cache.put(key(12), b"indexed payload".to_vec())?;
    cache.shutdown()?;

    let database = Connection::open(directory.path().join("index.sqlite"))?;
    let version: i64 = database.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let row: (i64, Vec<u8>, i64, i64, i64, i64) =
        database.query_row("SELECT namespace, name, size, length(hash), last_use, use_count FROM entries", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
        })?;
    assert_eq!(version, 5);
    assert_eq!(row.0, 0);
    assert_eq!(row.1, key(12).name());
    assert_eq!(row.2, 15);
    assert_eq!(row.3, 32);
    assert!(row.4 > 0);
    assert_eq!(row.5, 0);
    let secondary_indexes: i64 =
        database.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND tbl_name='entries'", [], |row| row.get(0))?;
    assert_eq!(secondary_indexes, 0);
    Ok(())
}

#[test]
fn namespace_change_remaps_entries() {
    const OLD: [Namespace; 2] = [Namespace::new("removed/items", "bin"), Namespace::new("retained/items", "bin")];
    const NEW: [Namespace; 1] = [Namespace::new("retained/items", "bin")];
    let directory = tempdir().expect("cache directory");
    let old_namespaces = NamespaceTable::new(&OLD).expect("old namespace table");
    let removed_key = Key::new(NamespaceId::new(0), [1; 16]);
    let retained_key = Key::new(NamespaceId::new(1), [2; 16]);
    let removed_path = old_namespaces.path(directory.path(), removed_key).expect("removed payload path");
    let retained_path = old_namespaces.path(directory.path(), retained_key).expect("retained payload path");

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &OLD).expect("open old namespace table");
    cache.put(removed_key, b"removed".to_vec()).expect("write removed namespace");
    cache.put(retained_key, b"retained".to_vec()).expect("write retained namespace");
    cache.shutdown().expect("close old namespace cache");

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &NEW).expect("open new namespace table");
    let remapped = Key::new(NamespaceId::new(0), retained_key.name());
    std::assert_matches!(cache.lookup(remapped), Lookup::Hit(CacheValue { bytes, .. }) if bytes == b"retained");
    cache.flush().expect("sweep removed namespace payload");
    assert!(!removed_path.exists());
    assert!(retained_path.exists());
    std::assert_matches!(cache.put(Key::new(NamespaceId::new(1), [3; 16]), vec![0]), Err(StorageError::UnknownNamespace));
    cache.shutdown().expect("close remapped cache");
}

#[test]
fn old_index_files_are_discarded() {
    let directory = tempdir().expect("cache directory");
    for name in ["snapshot", "journal", ".snapshot-tmp-old", ".journal-tmp-old", ".lock-tmp-old"] {
        fs::write(directory.path().join(name), b"obsolete").expect("write obsolete index file");
    }
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
    for name in ["snapshot", "journal"] {
        assert!(!directory.path().join(name).exists(), "{name} remained after open");
    }
    cache.flush().expect("finish bounded orphan sweep");
    for name in ["snapshot", "journal", ".snapshot-tmp-old", ".journal-tmp-old", ".lock-tmp-old"] {
        assert!(!directory.path().join(name).exists(), "{name} was retained");
    }
    assert!(directory.path().join("index.sqlite").exists());
    assert!(directory.path().join("lock").exists());
    cache.shutdown().expect("close cache");
}

#[cfg(windows)]
#[test]
fn locked_old_index_temp_does_not_block_open() {
    let directory = tempdir().expect("cache directory");
    let path = directory.path().join(".snapshot-tmp-locked");
    fs::write(&path, b"obsolete").expect("write obsolete index temp file");
    let locked_file = fs::OpenOptions::new().read(true).share_mode(0).open(path).expect("lock obsolete index temp file");

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache despite locked temp file");
    drop(locked_file);
    cache.shutdown().expect("close cache");
}

#[cfg(windows)]
fn locked_old_index_file_is_retried(name: &str) {
    let directory = tempdir().expect("cache directory");
    let path = directory.path().join(name);
    fs::write(&path, b"obsolete").expect("write obsolete index file");
    let locked_file = fs::OpenOptions::new().read(true).share_mode(0).open(&path).expect("lock obsolete index file");

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open despite locked legacy index file");
    cache.shutdown().expect("close cache");
    assert!(path.exists(), "locked {name} was removed");

    drop(locked_file);
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("retry obsolete file removal on open");
    assert!(!path.exists(), "{name} remained after the next open");
    cache.shutdown().expect("close reopened cache");
}

#[cfg(windows)]
#[test]
fn locked_old_snapshot_does_not_block_open() {
    locked_old_index_file_is_retried("snapshot");
}

#[cfg(windows)]
#[test]
fn locked_old_journal_does_not_block_open() {
    locked_old_index_file_is_retried("journal");
}

#[test]
fn corrupt_index_sweeps_payloads() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
    cache.put(key(1), b"payload bytes".to_vec()).expect("store payload");
    cache.shutdown().expect("close cache");
    let payload = data_file(directory.path(), key(1));
    for suffix in ["-wal", "-shm"] {
        let mut path = directory.path().join("index.sqlite").into_os_string();
        path.push(suffix);
        let path = PathBuf::from(path);
        if path.exists() {
            fs::remove_file(path).expect("remove stale SQLite sidecar");
        }
    }
    fs::write(directory.path().join("index.sqlite"), b"garbage SQLite data").expect("corrupt SQLite index");

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("recreate damaged index");
    assert_eq!(cache.entry_count(), 0);
    cache.flush().expect("sweep payloads from the recreated index");
    assert!(!payload.exists());
    cache.put(key(2), b"new payload".to_vec()).expect("write to recreated index");
    std::assert_matches!(cache.lookup(key(2)), Lookup::Hit(CacheValue { bytes, .. }) if bytes == b"new payload");
    cache.shutdown().expect("close recreated cache");
}

#[test]
fn unexpected_index_trigger_is_discarded() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
    cache.put(key(2), b"payload bytes".to_vec()).expect("store payload");
    cache.shutdown().expect("close cache");

    let database = Connection::open(directory.path().join("index.sqlite")).expect("open index database");
    database
        .execute_batch("CREATE TRIGGER discard_entries AFTER INSERT ON entries BEGIN DELETE FROM entries; END;")
        .expect("add unexpected trigger");
    drop(database);

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("recreate unexpected schema");
    let payload = data_file(directory.path(), key(2));
    assert_eq!(cache.entry_count(), 0);
    cache.flush().expect("sweep payload for discarded schema");
    assert!(!payload.exists());
    cache.shutdown().expect("close recreated cache");
}

#[test]
fn lookup_checks_hash_and_removes_corruption() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
    cache.put(key(1), b"payload bytes".to_vec()).expect("store payload");
    cache.shutdown().expect("close cache");
    let file = data_file(directory.path(), key(1));
    let mut bytes = fs::read(&file).expect("read payload file");
    let byte = bytes.first_mut().expect("first payload byte");
    *byte ^= 0xff;
    fs::write(file, bytes).expect("flip payload byte");

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("reopen cache");
    assert_eq!(cache.lookup(key(1)), Lookup::Corrupt);
    cache.flush().expect("remove corrupted entry");
    assert_eq!(cache.lookup(key(1)), Lookup::Miss);
    cache.shutdown().expect("close cache after removal");
}

#[test]
fn busy_usage_update_is_requeued_and_merged() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempdir()?;
    let hooks = Arc::new(writer::FaultHooks::default());
    let cache = Cache::open_internal(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES, hooks)?;
    let cache_key = key(25);
    cache.put(cache_key, b"usage metadata".to_vec())?;
    let first = match cache.lookup(cache_key) {
        Lookup::Hit(value) => value,
        lookup => return Err(format!("expected first cache hit, got {lookup:?}").into()),
    };

    cache.arm_fault(WritePoint::IndexTouchBusy);
    std::assert_matches!(cache.flush(), Err(StorageError::Unavailable(UnavailableReason::Busy)));
    let newest = match cache.lookup(cache_key) {
        Lookup::Hit(value) => value,
        lookup => return Err(format!("expected second cache hit, got {lookup:?}").into()),
    };
    assert!(newest.last_use > first.last_use);
    assert_eq!(newest.use_count, 2);
    cache.flush()?;
    cache.shutdown()?;

    let database = Connection::open(directory.path().join("index.sqlite"))?;
    let row: (i64, i64) = database.query_row(
        "SELECT last_use, use_count FROM entries WHERE namespace=0 AND name=?1",
        [cache_key.name().as_slice()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(row.0, i64::try_from(newest.last_use)?);
    assert_eq!(row.1, 2);
    Ok(())
}

#[test]
fn bounded_lookup_rejects_large_entries() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
    cache.put(key(3), b"payload bytes".to_vec()).expect("store payload");

    assert_eq!(cache.lookup_bounded(key(3), 4), Lookup::Corrupt);
    assert_eq!(cache.entry_count(), 0);
    cache.flush().expect("discard oversized entry");
    assert_eq!(cache.lookup(key(3)), Lookup::Miss);
    cache.shutdown().expect("close cache");
}

#[test]
fn missing_and_truncated_payloads_are_corrupt() {
    for missing in [false, true] {
        let directory = tempdir().expect("cache directory");
        let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
        cache.put(key(2), b"payload bytes".to_vec()).expect("store payload");
        cache.shutdown().expect("close cache");
        let file = data_file(directory.path(), key(2));
        if missing {
            fs::remove_file(&file).expect("remove payload");
        } else {
            let bytes = fs::read(&file).expect("read payload");
            fs::write(&file, bytes.get(..bytes.len().saturating_sub(1)).expect("truncated payload range")).expect("truncate payload");
        }

        let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open without data rescan");
        assert_eq!(cache.entry_count(), 1, "startup leaves payload validation to lookup");
        assert_eq!(cache.lookup(key(2)), Lookup::Corrupt);
        cache.flush().expect("remove damaged entry");
        assert_eq!(cache.entry_count(), 0);
        cache.shutdown().expect("close repaired cache");
    }
}

#[test]
fn write_failure_keeps_existing_entries() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
    cache.put(key(40), b"existing payload".to_vec()).expect("store existing payload");
    cache.arm_fault(WritePoint::DataWriteFailure);
    std::assert_matches!(
        cache.put(key(41), b"disk full payload".to_vec()),
        Err(StorageError::Io(error)) if error.kind() == std::io::ErrorKind::StorageFull
    );
    std::assert_matches!(cache.lookup(key(40)), Lookup::Hit(CacheValue { bytes, .. }) if bytes == b"existing payload");
    assert_eq!(cache.lookup(key(41)), Lookup::Miss);
    cache.shutdown().expect("close cache after failed write");
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
fn lookup_ignores_slow_writes() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache"));
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
fn lookup_retries_after_replacement() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache"));
    cache.put(key(21), b"old payload".to_vec()).expect("store old payload");
    let (selected, resume_reader) = cache.pause_at(WritePoint::LookupEntrySelected);

    thread::scope(|scope| {
        let reading_cache = Arc::clone(&cache);
        let reader = scope.spawn(move || reading_cache.lookup(key(21)));
        selected.recv_timeout(Duration::from_secs(3)).expect("reader selected old entry");
        cache.put(key(21), b"replacement payload".to_vec()).expect("replace entry");
        resume_reader.send(()).expect("resume reader");
        std::assert_matches!(
            reader.join().expect("reader completes"),
            Lookup::Hit(CacheValue { bytes, .. }) if bytes == b"replacement payload"
        );
    });

    Arc::try_unwrap(cache).unwrap_or_else(|_| panic!("cache handles released")).shutdown().expect("close cache");
}

#[test]
fn replacement_crash_never_trusts_bytes() {
    for point in crash_points() {
        let directory = tempdir().expect("cache directory");
        let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
        cache.put(key(23), b"old payload".to_vec()).expect("store old payload");
        cache.arm_fault(point);
        let result = cache.put(key(23), b"replacement payload".to_vec());
        if point == WritePoint::IndexBatchCommitted {
            std::assert_matches!(result, Ok(()), "the replacement row committed before the crash hook");
        } else {
            std::assert_matches!(result, Err(StorageError::Io(error)) if error.kind() == std::io::ErrorKind::Interrupted, "{point:?}");
        }
        cache.simulate_crash().expect("simulate replacement crash");

        let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("recover fixed path");
        match point {
            WritePoint::DataTempCreated | WritePoint::DataWritePartial | WritePoint::DataWritten => {
                std::assert_matches!(cache.lookup(key(23)), Lookup::Hit(CacheValue { bytes, .. }) if bytes == b"old payload", "{point:?}");
            }
            WritePoint::DataRenamed | WritePoint::IndexBatchStarted => {
                assert_eq!(cache.lookup(key(23)), Lookup::Corrupt, "{point:?}");
                cache.flush().expect("remove unrecoverable replaced row");
                assert_eq!(cache.lookup(key(23)), Lookup::Miss, "{point:?}");
            }
            WritePoint::IndexBatchCommitted => {
                std::assert_matches!(cache.lookup(key(23)), Lookup::Hit(CacheValue { bytes, .. }) if bytes == b"replacement payload", "{point:?}");
            }
            WritePoint::DataWriteFailure
            | WritePoint::IndexBatchFailure
            | WritePoint::LookupEntrySelected
            | WritePoint::LookupHashMismatch
            | WritePoint::IndexTouchBusy
            | WritePoint::PutQueued => unreachable!(),
        }
        cache.shutdown().expect("close recovered cache");
    }
}

#[test]
fn renamed_unindexed_payload_is_swept() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
    let path = data_file(directory.path(), key(77));
    cache.arm_fault(WritePoint::DataRenamed);
    assert!(cache.put(key(77), b"unindexed payload".to_vec()).is_err());
    cache.simulate_crash().expect("simulate crash after rename");
    assert!(path.exists());

    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("reopen cache");
    assert_eq!(cache.lookup(key(77)), Lookup::Miss);
    cache.flush().expect("sweep unindexed payload");
    assert!(!path.exists());
    cache.shutdown().expect("close cache");
}

#[test]
fn partial_batch_reports_committed_puts() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache"));
    let (started, resume) = cache.pause_at(WritePoint::IndexBatchStarted);

    thread::scope(|scope| {
        let writing_cache = Arc::clone(&cache);
        let first = thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn_scoped(scope, move || writing_cache.put(key(600), b"barrier".to_vec()))
            .expect("spawn first cache write");
        started.recv_timeout(Duration::from_secs(3)).expect("first write reached index");

        let write_count = MAX_BATCH_ROWS + 1;
        let (queued, resumes) = cache.pause_many_at(WritePoint::PutQueued, write_count);
        let mut pending = Vec::new();
        for id in 0..=MAX_BATCH_ROWS {
            let writing_cache = Arc::clone(&cache);
            let write_key = key(u16::try_from(id).expect("batch key fits"));
            let bytes = vec![u8::try_from(id % 251).expect("byte value")];
            let write = thread::Builder::new()
                .stack_size(256 * 1024)
                .spawn_scoped(scope, move || writing_cache.put(write_key, bytes))
                .expect("spawn queued cache write");
            pending.push((id, write));
        }
        let mut admitted = 0;
        for _ in 0..write_count {
            if queued.recv_timeout(Duration::from_secs(3)).is_err() {
                break;
            }
            admitted += 1;
        }
        for resume in resumes {
            resume.send(()).expect("release queued write");
        }
        cache.read_state.hooks.arm_after(WritePoint::IndexBatchStarted, 3);
        resume.send(()).expect("release first write");
        first.join().expect("first writer completes").expect("first write commits");
        assert_eq!(admitted, write_count, "every public write entered the bounded queue");
        let mut committed = 0;
        for (id, write) in pending {
            let result = write.join().expect("queued write completes");
            if id == 255 {
                std::assert_matches!(result, Err(StorageError::Io(error)) if error.kind() == std::io::ErrorKind::Interrupted);
            } else {
                std::assert_matches!(result, Ok(()), "key {id} was committed by the first chunk");
                committed += 1;
            }
        }
        assert_eq!(committed, MAX_BATCH_ROWS);
    });

    Arc::try_unwrap(cache).unwrap_or_else(|_| panic!("cache handles released")).simulate_crash().expect("stop failed writer");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("recover partially committed batch");
    for id in 0..MAX_BATCH_ROWS - 1 {
        let id = u16::try_from(id).expect("batch key fits");
        std::assert_matches!(cache.lookup(key(id)), Lookup::Hit(_), "row {id} committed before the simulated crash");
    }
    std::assert_matches!(cache.lookup(key(256)), Lookup::Hit(_));
    assert_eq!(cache.lookup(key(255)), Lookup::Miss);
    cache.shutdown().expect("close recovered cache");
}

#[test]
fn failed_replacement_removes_uncommitted_bytes() {
    let directory = tempdir().expect("cache directory");
    let cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
    let payload = data_file(directory.path(), key(23));
    cache.put(key(23), b"old payload".to_vec()).expect("store old payload");
    cache.arm_fault(WritePoint::IndexBatchFailure);

    std::assert_matches!(
        cache.put(key(23), b"replacement payload".to_vec()),
        Err(StorageError::Io(error)) if error.kind() == std::io::ErrorKind::StorageFull
    );
    cache.flush().expect("drain invalid replacement");

    assert_eq!(cache.lookup(key(23)), Lookup::Miss);
    assert_eq!(cache.entry_count(), 0);
    assert!(!payload.exists());
    cache.shutdown().expect("close cache");
}

#[test]
fn full_write_queue_rejects_more_work() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache"));
    let (started, resume) = cache.pause_at(WritePoint::DataWritten);

    thread::scope(|scope| {
        let writing_cache = Arc::clone(&cache);
        let first = thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn_scoped(scope, move || writing_cache.put(key(u16::MAX), vec![1]))
            .expect("spawn first cache write");
        started.recv_timeout(Duration::from_secs(3)).expect("first write reached payload stage");

        let write_count = MAX_QUEUED_WRITES - 1;
        let (queued, resumes) = cache.pause_many_at(WritePoint::PutQueued, write_count);
        let mut pending = Vec::new();
        for id in 0..MAX_QUEUED_WRITES - 1 {
            let writing_cache = Arc::clone(&cache);
            let write_key = key(u16::try_from(id).expect("queue key fits"));
            pending.push(
                thread::Builder::new()
                    .stack_size(256 * 1024)
                    .spawn_scoped(scope, move || writing_cache.put(write_key, Vec::new()))
                    .expect("spawn queued cache write"),
            );
        }
        let mut admitted = 0;
        for _ in 0..write_count {
            if queued.recv_timeout(Duration::from_secs(3)).is_err() {
                break;
            }
            admitted += 1;
        }
        std::assert_matches!(cache.put(key(1024), Vec::new()), Err(StorageError::Unavailable(UnavailableReason::WriteQueueFull)));
        for resume in resumes {
            resume.send(()).expect("release queued write");
        }
        resume.send(()).expect("release first write");
        first.join().expect("first writer completes").expect("first write commits");
        assert_eq!(admitted, write_count, "all public writes entered the bounded queue");
        assert!(pending.into_iter().all(|write| write.join().is_ok_and(|result| result.is_ok())));
    });

    Arc::try_unwrap(cache).unwrap_or_else(|_| panic!("cache handles released")).shutdown().expect("close cache");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 12, rng_seed: RngSeed::Fixed(0x5eed), ..ProptestConfig::default() })]

    #[test]
    fn stateful_operations_preserve_index(
        operations in proptest::collection::vec((0u8..8, 0u16..12, proptest::collection::vec(any::<u8>(), 0..33), 0u8..6), 1..24)
    ) {
        let directory = tempdir().expect("cache directory");
        let mut cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("open cache");
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
                    let existed = model.contains_key(&id);
                    cache.arm_fault(selected);
                    let result = cache.put(key(id), bytes.clone());
                    match result {
                        Ok(()) if selected == WritePoint::IndexBatchCommitted => {}
                        Err(StorageError::Io(error))
                            if selected != WritePoint::IndexBatchCommitted && error.kind() == std::io::ErrorKind::Interrupted => {}
                        other => prop_assert!(false, "unexpected write result: {other:?}"),
                    }
                    cache.simulate_crash().expect("simulate process crash");
                    if selected == WritePoint::IndexBatchCommitted {
                        model.insert(id, bytes);
                    } else if matches!(selected, WritePoint::DataRenamed | WritePoint::IndexBatchStarted) {
                        model.remove(&id);
                    }
                    cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("recover after index crash");
                    if matches!(selected, WritePoint::DataRenamed | WritePoint::IndexBatchStarted) && existed {
                        prop_assert_eq!(cache.lookup(key(id)), Lookup::Corrupt);
                        cache.flush().expect("remove stale row");
                    }
                }
                6 => {
                    let removed = cache.remove(key(id)).expect("remove entry");
                    prop_assert_eq!(removed, model.remove(&id).is_some());
                }
                _ => check_model(&cache, &model)?,
            }

            prop_assert!(cache.accounted_size() <= cache.size_limit());
            prop_assert_eq!(cache.entry_count(), model.len());
            check_model(&cache, &model)?;
            cache.shutdown().expect("close cache after operation");
            cache = Cache::open(directory.path(), DEFAULT_SIZE_LIMIT, &TEST_NAMESPACES).expect("reopen cache after operation");
            prop_assert!(cache.accounted_size() <= cache.size_limit());
            prop_assert_eq!(cache.entry_count(), model.len());
            check_model(&cache, &model)?;
        }

        let small_limit = MIN_SIZE_LIMIT + 8192;
        cache.set_size_limit(small_limit).expect("lower cache limit");
        reconcile_after_eviction(&cache, &mut model)?;
        for _ in 0..20 {
            cache.clear().expect("clear small cache");
            model.clear();
            prop_assert!(cache.accounted_size() <= small_limit);
            prop_assert_eq!(cache.entry_count(), model.len());
        }
        for revision in 0..20u8 {
            let bytes = vec![revision; 32];
            cache.arm_fault(WritePoint::IndexBatchCommitted);
            let result = cache.put(key(0), bytes.clone());
            prop_assert!(result.is_ok(), "the row committed before the crash hook");
            cache.simulate_crash().expect("simulate replacement crash");
            model.insert(0, bytes);
            cache = Cache::open(directory.path(), small_limit, &TEST_NAMESPACES).expect("reopen after committed replacement");
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
            actual => prop_assert!(false, "unexpected cache result after eviction: {actual:?}"),
        }
    }
    Ok(())
}
