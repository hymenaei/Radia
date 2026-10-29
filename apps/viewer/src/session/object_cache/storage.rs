use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};

use ::storage::sqlite::SqliteReceipt;
use ::storage::{
    default_cache_dir,
    sqlite::{MAX_BATCH_BYTES, MAX_BATCH_ROWS, MigrationError, OpenStatus, SqliteBatch, SqliteDb, SqliteError},
};
#[cfg(test)]
use network::Grid;
use rusqlite::{Connection, Error as RusqliteError, OptionalExtension, Transaction, params};
#[cfg(test)]
use tokio::runtime::Handle;

use super::{
    CacheScope, EncodedEntry, MAX_DATABASE_PAGES, MAX_ENTRY_BATCH_ROWS, MAX_GLOBAL_PRUNE_ROWS, MAX_OBJECT_CACHE_ENTRY_BYTES,
    MAX_REGION_BYTES, MAX_REGION_DELETE_BATCHES, MAX_REGION_ENTRIES, MAX_TOTAL_BYTES, MAX_TOTAL_ENTRIES, SCHEMA_VERSION, StoredEntry,
};
#[cfg(test)]
use super::{CacheWriteWorker, ObjectCacheEvent, ObjectCacheSession};

pub(super) struct PendingSubmission {
    pub(super) batches: Vec<SqliteBatch>,
    pub(super) rows: usize,
    pub(super) bytes: usize,
}
pub(super) fn open_default() -> Result<(SqliteDb, OpenStatus), String> {
    let root = default_cache_dir().map_err(|error| error.to_string())?;
    open_at(root.join("objects.sqlite")).map_err(|error| error.to_string())
}

pub(super) fn open_at(path: impl AsRef<Path>) -> Result<(SqliteDb, OpenStatus), SqliteError> {
    SqliteDb::open(path, migrate)
}

pub(super) fn migrate(connection: &mut Connection) -> Result<(), MigrationError> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match version {
        0 => {
            let transaction = connection.transaction()?;
            transaction.execute_batch(
                "CREATE TABLE region_scopes (
                    grid_id TEXT NOT NULL,
                    region_id BLOB NOT NULL CHECK(length(region_id) = 16),
                    cache_id BLOB NOT NULL CHECK(length(cache_id) = 16),
                    last_used INTEGER NOT NULL,
                    PRIMARY KEY (grid_id, region_id, cache_id)
                ) WITHOUT ROWID;
                CREATE TABLE object_entries (
                    grid_id TEXT NOT NULL,
                    region_id BLOB NOT NULL CHECK(length(region_id) = 16),
                    cache_id BLOB NOT NULL CHECK(length(cache_id) = 16),
                    local_id INTEGER NOT NULL CHECK(local_id > 0 AND local_id <= 4294967295),
                    crc INTEGER NOT NULL CHECK(crc >= 0 AND crc <= 4294967295),
                    update_kind INTEGER NOT NULL CHECK(update_kind IN (0, 1)),
                    update_flags INTEGER NOT NULL CHECK(update_flags >= 0 AND update_flags <= 4294967295),
                    payload BLOB NOT NULL CHECK(length(payload) >= 64 AND length(payload) <= 10000),
                    PRIMARY KEY (grid_id, region_id, cache_id, local_id)
                ) WITHOUT ROWID;
                ",
            )?;
            install_global_stats(&transaction, false)?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            transaction.commit()?;
        }
        1 => {
            let transaction = connection.transaction()?;
            install_global_stats(&transaction, true)?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            transaction.commit()?;
        }
        SCHEMA_VERSION => {}
        _ => return Err(MigrationError::SchemaMismatch),
    }
    connection.pragma_update(None, "max_page_count", MAX_DATABASE_PAGES)?;
    Ok(())
}

fn install_global_stats(connection: &Connection, backfill: bool) -> Result<(), MigrationError> {
    connection.execute_batch(
        "CREATE TABLE object_cache_totals (
            singleton INTEGER NOT NULL PRIMARY KEY CHECK(singleton = 1),
            entry_count INTEGER NOT NULL CHECK(entry_count >= 0),
            payload_bytes INTEGER NOT NULL CHECK(payload_bytes >= 0)
        ) WITHOUT ROWID;",
    )?;
    if backfill {
        connection.execute(
            "INSERT INTO object_cache_totals (singleton, entry_count, payload_bytes)
             SELECT 1, COUNT(*), COALESCE(SUM(length(payload)), 0) FROM object_entries",
            [],
        )?;
    } else {
        connection.execute("INSERT INTO object_cache_totals (singleton, entry_count, payload_bytes) VALUES (1, 0, 0)", [])?;
    }
    connection.execute_batch(
        "CREATE TRIGGER object_entries_totals_insert AFTER INSERT ON object_entries BEGIN
            UPDATE object_cache_totals SET entry_count = entry_count + 1, payload_bytes = payload_bytes + length(NEW.payload)
            WHERE singleton = 1;
         END;
         CREATE TRIGGER object_entries_totals_delete AFTER DELETE ON object_entries BEGIN
            UPDATE object_cache_totals SET entry_count = entry_count - 1, payload_bytes = payload_bytes - length(OLD.payload)
            WHERE singleton = 1;
         END;
         CREATE TRIGGER object_entries_totals_update AFTER UPDATE OF payload ON object_entries
         WHEN length(OLD.payload) != length(NEW.payload) BEGIN
            UPDATE object_cache_totals SET payload_bytes = payload_bytes + length(NEW.payload) - length(OLD.payload)
            WHERE singleton = 1;
         END;",
    )?;
    Ok(())
}

#[cfg(test)]
pub(super) fn open_for_test(path: impl AsRef<Path>, grid: Grid) -> ObjectCacheSession {
    let (db, status) = open_at(path).expect("open isolated object cache database");
    let mut cache = ObjectCacheSession::new(grid);
    let db = Arc::new(db);
    let writer = CacheWriteWorker::start(Arc::clone(&db), Handle::current()).expect("start isolated cache writer");
    assert!(cache.handle_event(ObjectCacheEvent::Ready { db, status, writer }).is_none());
    cache
}

pub(super) fn queue_scope_load(db: &SqliteDb, scope: CacheScope) -> Result<SqliteReceipt<HashMap<u32, StoredEntry>>, SqliteError> {
    db.read(0, move |transaction| {
        let (row_count, bytes) = scope_stats(transaction, &scope)?;
        if row_count > MAX_REGION_ENTRIES || bytes > MAX_REGION_BYTES {
            return Ok(HashMap::new());
        }
        let mut statement = transaction.prepare(
            "SELECT local_id, crc, update_kind, update_flags, length(payload),
                    CASE WHEN length(payload) <= ?1 THEN payload ELSE NULL END
             FROM object_entries WHERE grid_id = ?2 AND region_id = ?3 AND cache_id = ?4 ORDER BY local_id",
        )?;
        let row_iter = statement.query_map(
            params![
                MAX_OBJECT_CACHE_ENTRY_BYTES as i64,
                scope.grid_id,
                scope.region_id.as_bytes().as_slice(),
                scope.cache_id.as_bytes().as_slice()
            ],
            |row| {
                Ok(StoredEntry {
                    local_id: row.get(0)?,
                    crc: row.get(1)?,
                    update_kind: row.get(2)?,
                    update_flags: row.get(3)?,
                    payload_bytes: row.get(4)?,
                    payload: row.get::<_, Option<Vec<u8>>>(5)?.map(Arc::from),
                })
            },
        )?;
        let mut entries = HashMap::with_capacity(row_count);
        for row in row_iter {
            let entry = row?;
            if let Ok(local_id) = u32::try_from(entry.local_id) {
                entries.insert(local_id, entry);
            }
        }
        Ok(entries)
    })
}

pub(super) fn make_write_batches(scope: CacheScope, entries: Arc<[EncodedEntry]>, last_used: i64) -> Result<Vec<SqliteBatch>, SqliteError> {
    let chunk_count = entries.len().div_ceil(MAX_ENTRY_BATCH_ROWS);
    let mut batches = Vec::with_capacity(chunk_count + 3);
    for chunk_index in 0..chunk_count {
        let start = chunk_index * MAX_ENTRY_BATCH_ROWS;
        let chunk_rows = entries.len().saturating_sub(start).min(MAX_ENTRY_BATCH_ROWS);
        let include_scope = chunk_index == 0;
        let bytes = entries
            .iter()
            .skip(start)
            .take(chunk_rows)
            .map(|entry| entry.payload.len().saturating_add(64))
            .sum::<usize>()
            .saturating_add(usize::from(include_scope) * 64);
        let rows = chunk_rows.saturating_add(usize::from(include_scope));
        let entries_for_batch = Arc::clone(&entries);
        let scope_for_batch = scope.clone();
        let batch = SqliteBatch::new(rows, bytes, move |transaction| {
            if include_scope {
                upsert_scope(transaction, &scope_for_batch, last_used)?;
            }
            write_entry_chunk(transaction, &scope_for_batch, &entries_for_batch, start, chunk_rows)
        })?;
        batches.push(batch);
    }
    let region_prune =
        SqliteBatch::new(256, 256, move |transaction| prune_scope(transaction, &scope, MAX_REGION_ENTRIES, MAX_REGION_BYTES))?;
    let global_prune_first = SqliteBatch::new(256, 256, prune_global_chunk)?;
    let global_prune_second = SqliteBatch::new(256, 256, prune_global_chunk)?;
    batches.extend([region_prune, global_prune_first, global_prune_second]);
    Ok(batches)
}

fn write_entry_chunk(
    transaction: &Transaction<'_>,
    scope: &CacheScope,
    entries: &[EncodedEntry],
    start: usize,
    count: usize,
) -> Result<(), RusqliteError> {
    for entry in entries.iter().skip(start).take(count) {
        transaction.execute(
            "INSERT INTO object_entries (grid_id, region_id, cache_id, local_id, crc, update_kind, update_flags, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (grid_id, region_id, cache_id, local_id) DO UPDATE SET
                crc = excluded.crc,
                update_kind = excluded.update_kind,
                update_flags = excluded.update_flags,
                payload = excluded.payload
             WHERE object_entries.crc != excluded.crc
                OR object_entries.update_kind != excluded.update_kind
                OR object_entries.update_flags != excluded.update_flags
                OR object_entries.payload != excluded.payload",
            params![
                scope.grid_id.as_str(),
                scope.region_id.as_bytes().as_slice(),
                scope.cache_id.as_bytes().as_slice(),
                i64::from(entry.local_id),
                i64::from(entry.crc),
                entry.update_kind,
                i64::from(entry.update_flags),
                entry.payload.as_ref(),
            ],
        )?;
    }
    Ok(())
}

pub(super) fn make_scope_delete_submission(scope: CacheScope) -> PendingSubmission {
    let mut batches = Vec::with_capacity(MAX_REGION_DELETE_BATCHES);
    for _ in 0..MAX_REGION_DELETE_BATCHES {
        let scope = scope.clone();
        let batch = SqliteBatch::new(256, 256, move |transaction| delete_scope_chunk(transaction, &scope))
            .expect("scope delete batch fits SQLite limits");
        batches.push(batch);
    }
    PendingSubmission { batches, rows: MAX_REGION_DELETE_BATCHES * 256, bytes: MAX_REGION_DELETE_BATCHES * 256 }
}

pub(super) fn make_clear_all_batches() -> Vec<SqliteBatch> {
    vec![
        SqliteBatch::new(MAX_BATCH_ROWS, MAX_BATCH_BYTES, |transaction| {
            transaction.execute_batch("DELETE FROM object_entries; DELETE FROM region_scopes;")?;
            Ok(())
        })
        .expect("clear cache batch fits SQLite limits"),
    ]
}

pub(super) fn make_access_submission(scope: CacheScope, stale: HashSet<u32>, last_used: i64) -> PendingSubmission {
    let rows = stale.len().saturating_add(1).min(256);
    let bytes = stale.len().saturating_mul(4).saturating_add(64);
    let batch = SqliteBatch::new(rows, bytes, move |transaction| {
        for local_id in stale {
            transaction.execute(
                "DELETE FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3 AND local_id = ?4",
                params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice(), i64::from(local_id)],
            )?;
        }
        let has_entries = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3)",
            params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice()],
            |row| row.get::<_, bool>(0),
        )?;
        if has_entries {
            transaction.execute(
                "UPDATE region_scopes SET last_used = MAX(last_used, ?4) WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3",
                params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice(), last_used],
            )?;
        } else {
            transaction.execute(
                "DELETE FROM region_scopes WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3",
                params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice()],
            )?;
        }
        Ok(())
    })
    .expect("cache access batch fits SQLite limits");
    PendingSubmission { batches: vec![batch], rows, bytes }
}

fn upsert_scope(transaction: &Transaction<'_>, scope: &CacheScope, last_used: i64) -> Result<(), RusqliteError> {
    transaction.execute(
        "INSERT INTO region_scopes (grid_id, region_id, cache_id, last_used) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (grid_id, region_id, cache_id) DO UPDATE SET last_used = MAX(last_used, excluded.last_used)",
        params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice(), last_used],
    )?;
    Ok(())
}

fn prune_scope(transaction: &Transaction<'_>, scope: &CacheScope, max_rows: usize, max_bytes: usize) -> Result<(), RusqliteError> {
    let (rows, bytes) = scope_stats(transaction, scope)?;
    if rows <= max_rows && bytes <= max_bytes {
        return Ok(());
    }
    let mut statement = transaction.prepare(
        "SELECT local_id FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3
         ORDER BY length(payload) DESC, local_id LIMIT 255",
    )?;
    let local_ids = statement
        .query_map(params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice()], |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for local_id in local_ids {
        transaction.execute(
            "DELETE FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3 AND local_id = ?4",
            params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice(), local_id],
        )?;
    }
    remove_empty_scope(transaction, scope)?;
    Ok(())
}

fn prune_global_chunk(transaction: &Transaction<'_>) -> Result<(), RusqliteError> {
    let (rows, bytes) = global_stats(transaction)?;
    let rows = i64::try_from(rows).unwrap_or(i64::MAX);
    let bytes = i64::try_from(bytes).unwrap_or(i64::MAX);
    if rows <= MAX_TOTAL_ENTRIES as i64 && bytes <= MAX_TOTAL_BYTES as i64 {
        return Ok(());
    }
    let mut remaining_rows = usize::try_from(rows).unwrap_or(usize::MAX);
    let mut remaining_bytes = usize::try_from(bytes).unwrap_or(usize::MAX);
    let mut deleted = 0;
    while (remaining_rows > MAX_TOTAL_ENTRIES || remaining_bytes > MAX_TOTAL_BYTES) && deleted < MAX_GLOBAL_PRUNE_ROWS {
        let oldest = transaction
            .query_row(
                "SELECT grid_id, region_id, cache_id FROM region_scopes
                 WHERE EXISTS (
                    SELECT 1 FROM object_entries e
                    WHERE e.grid_id = region_scopes.grid_id AND e.region_id = region_scopes.region_id AND e.cache_id = region_scopes.cache_id
                 )
                 ORDER BY last_used, grid_id, region_id, cache_id LIMIT 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?, row.get::<_, Vec<u8>>(2)?)),
            )
            .optional()?;
        let Some((grid_id, region_id, cache_id)) = oldest else { break };
        let limit = i64::try_from(MAX_GLOBAL_PRUNE_ROWS - deleted).unwrap_or(i64::MAX);
        let mut statement = transaction.prepare(
            "SELECT local_id, length(payload) FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3
             ORDER BY length(payload) DESC, local_id LIMIT ?4",
        )?;
        let local_ids = statement
            .query_map(params![grid_id, region_id, cache_id, limit], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        if local_ids.is_empty() {
            break;
        }
        for (local_id, payload_bytes) in &local_ids {
            transaction.execute(
                "DELETE FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3 AND local_id = ?4",
                params![grid_id, region_id, cache_id, local_id],
            )?;
            remaining_rows = remaining_rows.saturating_sub(1);
            remaining_bytes = remaining_bytes.saturating_sub(usize::try_from(*payload_bytes).unwrap_or(usize::MAX));
        }
        deleted += local_ids.len();
        if !transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3)",
            params![grid_id, region_id, cache_id],
            |row| row.get::<_, bool>(0),
        )? {
            transaction.execute(
                "DELETE FROM region_scopes WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3",
                params![grid_id, region_id, cache_id],
            )?;
        }
    }
    Ok(())
}

fn global_stats(transaction: &Transaction<'_>) -> Result<(usize, usize), RusqliteError> {
    let (rows, bytes): (i64, i64) =
        transaction.query_row("SELECT entry_count, payload_bytes FROM object_cache_totals WHERE singleton = 1", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    Ok((usize::try_from(rows).unwrap_or(usize::MAX), usize::try_from(bytes).unwrap_or(usize::MAX)))
}

fn delete_scope_chunk(transaction: &Transaction<'_>, scope: &CacheScope) -> Result<(), RusqliteError> {
    let mut statement = transaction.prepare(
        "SELECT local_id FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3 ORDER BY local_id LIMIT 255",
    )?;
    let local_ids = statement
        .query_map(params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice()], |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for local_id in local_ids {
        transaction.execute(
            "DELETE FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3 AND local_id = ?4",
            params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice(), local_id],
        )?;
    }
    remove_empty_scope(transaction, scope)
}

fn remove_empty_scope(transaction: &Transaction<'_>, scope: &CacheScope) -> Result<(), RusqliteError> {
    transaction.execute(
        "DELETE FROM region_scopes WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3
         AND NOT EXISTS (
            SELECT 1 FROM object_entries e
            WHERE e.grid_id = region_scopes.grid_id AND e.region_id = region_scopes.region_id AND e.cache_id = region_scopes.cache_id
         )",
        params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice()],
    )?;
    Ok(())
}

fn scope_stats(transaction: &Transaction<'_>, scope: &CacheScope) -> Result<(usize, usize), RusqliteError> {
    let (rows, bytes): (i64, i64) = transaction.query_row(
        "SELECT COUNT(*), COALESCE(SUM(length(payload)), 0) FROM object_entries
         WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3",
        params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok((usize::try_from(rows).unwrap_or(usize::MAX), usize::try_from(bytes).unwrap_or(usize::MAX)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::object_cache::test_support::*;
    #[tokio::test]
    async fn schema_v1_migration_backfills_incremental_global_totals() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let path = directory.path().join("objects.sqlite");
        let connection = Connection::open(&path).expect("create version one cache");
        connection
            .execute_batch(
                "CREATE TABLE region_scopes (
                grid_id TEXT NOT NULL,
                region_id BLOB NOT NULL CHECK(length(region_id) = 16),
                cache_id BLOB NOT NULL CHECK(length(cache_id) = 16),
                last_used INTEGER NOT NULL,
                PRIMARY KEY (grid_id, region_id, cache_id)
            ) WITHOUT ROWID;
            CREATE TABLE object_entries (
                grid_id TEXT NOT NULL,
                region_id BLOB NOT NULL CHECK(length(region_id) = 16),
                cache_id BLOB NOT NULL CHECK(length(cache_id) = 16),
                local_id INTEGER NOT NULL CHECK(local_id > 0 AND local_id <= 4294967295),
                crc INTEGER NOT NULL CHECK(crc >= 0 AND crc <= 4294967295),
                update_kind INTEGER NOT NULL CHECK(update_kind IN (0, 1)),
                update_flags INTEGER NOT NULL CHECK(update_flags >= 0 AND update_flags <= 4294967295),
                payload BLOB NOT NULL CHECK(length(payload) >= 64 AND length(payload) <= 10000),
                PRIMARY KEY (grid_id, region_id, cache_id, local_id)
            ) WITHOUT ROWID;",
            )
            .expect("create version one schema");
        connection
            .execute(
                "INSERT INTO object_entries (grid_id, region_id, cache_id, local_id, crc, update_kind, update_flags, payload)
             VALUES (?1, ?2, ?3, 7, 8, 0, 9, zeroblob(64))",
                params![Grid::Agni.id(), REGION_ID.as_bytes().as_slice(), CACHE_ID.as_bytes().as_slice()],
            )
            .expect("add a version one object entry");
        connection.pragma_update(None, "user_version", 1).expect("set version one schema marker");
        drop(connection);

        let (db, status) = open_at(&path).expect("migrate version one cache");
        assert_eq!(cache_totals(&db).await, (1, 64, 1, 64));
        let mut cache = ObjectCacheSession::new(Grid::Agni);
        let db = std::sync::Arc::new(db);
        let writer = CacheWriteWorker::start(std::sync::Arc::clone(&db), Handle::current()).expect("start cache writer");
        assert!(cache.handle_event(ObjectCacheEvent::Ready { db, status, writer }).is_none());
        cache.close().await;
    }

    #[test]
    fn schema_v1_migration_rolls_back_after_a_partial_upgrade_failure() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let path = directory.path().join("objects.sqlite");
        let connection = Connection::open(&path).expect("create isolated SQLite database");
        connection
            .execute_batch("CREATE TABLE object_entries (local_id INTEGER); PRAGMA user_version = 1;")
            .expect("create partial v1 schema");
        drop(connection);

        assert!(open_at(&path).is_err());
        let connection = Connection::open(&path).expect("reopen database after failed migration");
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0)).expect("read schema version");
        let totals_table_exists: bool = connection
            .query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'object_cache_totals')", [], |row| {
                row.get(0)
            })
            .expect("check whether migration table remains");

        assert_eq!(version, 1);
        assert!(!totals_table_exists);
    }

    #[tokio::test]
    async fn global_totals_track_payload_updates_and_deletions() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let mut cache = open_cache(directory.path().join("objects.sqlite"));
        let full = fixture_message(vectors::OBJECT_UPDATE);
        cache.record_update(SIMULATOR, full.clone());
        flush(&mut cache).await;

        let db = cache.db.as_ref().expect("cache database is open");
        let before = cache_totals(db).await;
        assert!(before.0 > 1);
        assert_eq!(before.0, before.2);
        assert_eq!(before.1, before.3);

        let (_, references) = cached_references(&full);
        let updated_local_id = references[0].local_id;
        let deleted_local_id = references[1].local_id;
        let update = SqliteBatch::new(1, 65, move |transaction| {
            transaction.execute(
                "UPDATE object_entries SET payload = zeroblob(65)
             WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3 AND local_id = ?4",
                params![Grid::Agni.id(), REGION_ID.as_bytes().as_slice(), CACHE_ID.as_bytes().as_slice(), i64::from(updated_local_id)],
            )?;
            transaction.execute(
                "DELETE FROM object_entries
             WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3 AND local_id = ?4",
                params![Grid::Agni.id(), REGION_ID.as_bytes().as_slice(), CACHE_ID.as_bytes().as_slice(), i64::from(deleted_local_id)],
            )?;
            Ok(())
        })
        .expect("stats trigger batch fits SQLite limits");
        cache.db.as_ref().expect("cache database is open").submit_batches(vec![update]).expect("queue stats trigger batch");
        flush(&mut cache).await;
        let after = cache_totals(cache.db.as_ref().expect("cache database is open")).await;
        assert_eq!(after.0, before.0 - 1);
        assert_ne!(after.1, before.1);
        assert_eq!(after.0, after.2);
        assert_eq!(after.1, after.3);
        cache.close().await;
    }
}
