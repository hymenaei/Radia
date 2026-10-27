use std::{error::Error, io, path::Path};

use futures::executor::block_on;
use rusqlite::{Connection, Error as RusqliteError, Transaction, TransactionBehavior, params, types::Type};

use crate::sqlite::{MAX_BATCH_ROWS, MigrationError, OpenStatus, SqliteBatch, SqliteDb, SqliteError};

use super::{
    Key, MAX_QUEUED_WRITES, MAX_VALUE_BYTES, NamespaceId, NamespaceTable, StorageError, UnavailableReason, valid_extension, valid_namespace,
};

const INDEX_VERSION: i64 = 5;
pub(super) const MAX_ENTRIES: usize = 262_144;
const ROW_BYTES: u64 = 96;
pub(super) const INDEX_BASE_BYTES: u64 = 16 * 1024;
const MUTATION_BYTES: usize = 64;

#[derive(Clone)]
pub(super) struct Record {
    pub key: Key,
    pub size: u64,
    pub hash: [u8; 32],
    pub last_use: u64,
    pub use_count: u64,
}

pub(super) enum Mutation {
    Put(Record),
    Remove(Key),
}

pub(super) struct Index {
    db: Option<SqliteDb>,
    namespace_count: usize,
    namespace_bytes: u64,
}

impl Index {
    pub(super) fn open(path: impl AsRef<Path>, namespaces: &NamespaceTable) -> Result<(Self, Vec<Record>), StorageError> {
        let mut definitions = Vec::new();
        definitions.try_reserve_exact(namespaces.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        for definition in namespaces.definitions() {
            definitions.push((definition.directory.to_owned(), definition.extension.to_owned()));
        }
        let namespace_bytes = namespace_bytes(namespaces)?;
        let migration_definitions = definitions.clone();
        let (db, _status): (SqliteDb, OpenStatus) =
            SqliteDb::open(path, move |connection| migrate(connection, &migration_definitions)).map_err(map_sqlite_error)?;
        let index = Self { db: Some(db), namespace_count: definitions.len(), namespace_bytes };
        let records = index.load_records().map_err(map_sqlite_error)?;
        Ok((index, records))
    }

    pub(super) fn apply(&self, mutations: Vec<Mutation>) -> Result<(), SqliteError> {
        if mutations.is_empty() {
            return Ok(());
        }
        let rows = mutations.len();
        let bytes = rows.checked_mul(MUTATION_BYTES).ok_or(SqliteError::QueueFull)?;
        let batch = SqliteBatch::new(rows, bytes, move |transaction| apply_mutations(transaction, mutations))?;
        let receipt = self.database()?.submit_batches(vec![batch])?;
        block_on(receipt.wait())
    }

    pub(super) fn touch(&self, records: Vec<Record>) -> Result<(), SqliteError> {
        if records.is_empty() {
            return Ok(());
        }
        let rows = records.len();
        let bytes = rows.checked_mul(MUTATION_BYTES).ok_or(SqliteError::QueueFull)?;
        let batch = SqliteBatch::new(rows, bytes, move |transaction| {
            let mut statement = transaction.prepare("UPDATE entries SET last_use=?1, use_count=?2 WHERE namespace=?3 AND name=?4")?;
            for record in records {
                statement.execute(params![
                    sql_integer(record.last_use),
                    sql_integer(record.use_count),
                    i64::from(record.key.namespace().0),
                    record.key.name().as_slice(),
                ])?;
            }
            Ok(())
        })?;
        let receipt = self.database()?.submit_batches(vec![batch])?;
        block_on(receipt.wait())
    }

    pub(super) fn clear(&self) -> Result<(), SqliteError> {
        let batch = SqliteBatch::new(0, 0, |transaction| transaction.execute("DELETE FROM entries", []).map(|_| ()))?;
        let receipt = self.database()?.submit_batches(vec![batch])?;
        block_on(receipt.wait())
    }

    pub(super) fn estimated_bytes(&self, entry_count: usize) -> Result<u64, StorageError> {
        INDEX_BASE_BYTES
            .checked_add(self.namespace_bytes)
            .and_then(|bytes| bytes.checked_add(u64::try_from(entry_count).ok()?.checked_mul(ROW_BYTES)?))
            .ok_or(StorageError::TooLarge)
    }

    pub(super) fn shutdown(mut self) -> Result<(), SqliteError> {
        match self.db.take() {
            Some(db) => db.shutdown(),
            None => Ok(()),
        }
    }

    fn load_records(&self) -> Result<Vec<Record>, SqliteError> {
        self.load_records_with_limits(MAX_ENTRIES, MAX_QUEUED_WRITES)
    }

    fn load_records_with_limits(&self, max_entries: usize, max_excess: usize) -> Result<Vec<Record>, SqliteError> {
        for attempt in 0..2 {
            let result = (|| {
                self.evict_excess(max_entries, max_excess)?;
                let namespace_count = self.namespace_count;
                let receipt = self.database()?.read(0, move |transaction| {
                    let mut statement = transaction
                        .prepare("SELECT namespace, name, size, hash, last_use, use_count FROM entries ORDER BY namespace, name")?;
                    let mut rows = statement.query([])?;
                    let mut records = Vec::new();
                    while let Some(row) = rows.next()? {
                        if records.len() == max_entries {
                            return Err(conversion_error(0, Type::Integer));
                        }
                        let namespace: i64 = row.get(0)?;
                        let name = blob::<16>(row.get(1)?, 1)?;
                        let size = unsigned(row.get(2)?, 2)?;
                        if size > MAX_VALUE_BYTES as u64 {
                            return Err(conversion_error(2, Type::Integer));
                        }
                        let hash = blob::<32>(row.get(3)?, 3)?;
                        let last_use = unsigned(row.get(4)?, 4)?;
                        let use_count = unsigned(row.get(5)?, 5)?;
                        let namespace = u8::try_from(namespace).map_err(|_| conversion_error(0, Type::Integer))?;
                        if usize::from(namespace) >= namespace_count {
                            return Err(conversion_error(0, Type::Integer));
                        }
                        records.try_reserve(1).map_err(|_| allocation_error())?;
                        records.push(Record { key: Key::new(NamespaceId::new(namespace), name), size, hash, last_use, use_count });
                    }
                    Ok(records)
                })?;
                block_on(receipt.wait())
            })();
            match result {
                Ok(records) => return Ok(records),
                Err(SqliteError::Corrupt | SqliteError::SchemaMismatch) if attempt == 0 => {}
                Err(error) => return Err(error),
            }
        }
        Err(SqliteError::Corrupt)
    }

    fn evict_excess(&self, max_entries: usize, max_excess: usize) -> Result<(), SqliteError> {
        let namespace_count = self.namespace_count;
        let receipt = self.database()?.read(0, move |transaction| {
            let row_count: i64 = transaction.query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))?;
            let row_count = usize::try_from(row_count).map_err(|_| conversion_error(0, Type::Integer))?;
            let excess = row_count.saturating_sub(max_entries);
            if excess > max_excess {
                return Err(conversion_error(0, Type::Integer));
            }
            if excess == 0 {
                return Ok(Vec::new());
            }
            let limit = i64::try_from(excess).map_err(|_| conversion_error(0, Type::Integer))?;
            let mut statement =
                transaction.prepare("SELECT namespace, name FROM entries ORDER BY last_use ASC, namespace ASC, name ASC LIMIT ?1")?;
            let mut rows = statement.query([limit])?;
            let mut keys = Vec::new();
            keys.try_reserve_exact(excess).map_err(|_| allocation_error())?;
            while let Some(row) = rows.next()? {
                let namespace = u8::try_from(row.get::<_, i64>(0)?).map_err(|_| conversion_error(0, Type::Integer))?;
                if usize::from(namespace) >= namespace_count {
                    return Err(conversion_error(0, Type::Integer));
                }
                keys.push(Key::new(NamespaceId::new(namespace), blob::<16>(row.get(1)?, 1)?));
            }
            if keys.len() != excess {
                return Err(conversion_error(0, Type::Integer));
            }
            Ok(keys)
        })?;
        let keys = block_on(receipt.wait())?;
        for chunk in keys.chunks(MAX_BATCH_ROWS) {
            let mut mutations = Vec::new();
            mutations.try_reserve_exact(chunk.len()).map_err(|_| SqliteError::Sqlite(allocation_error()))?;
            mutations.extend(chunk.iter().copied().map(Mutation::Remove));
            self.apply(mutations)?;
        }
        Ok(())
    }

    fn database(&self) -> Result<&SqliteDb, SqliteError> {
        self.db.as_ref().ok_or(SqliteError::WorkerStopped)
    }
}

impl Drop for Index {
    fn drop(&mut self) {
        if let Some(db) = self.db.take() {
            match db.shutdown() {
                Ok(()) => {}
                Err(error) => tracing::warn!(error = %error, "asset cache SQLite worker stopped with an error"),
            }
        }
    }
}

pub(super) fn namespace_bytes(namespaces: &NamespaceTable) -> Result<u64, StorageError> {
    namespaces.definitions().try_fold(0u64, |bytes, definition| {
        let namespace_bytes =
            u64::try_from(definition.directory.len() + definition.extension.len() + 32).map_err(|_| StorageError::TooLarge)?;
        bytes.checked_add(namespace_bytes).ok_or(StorageError::TooLarge)
    })
}

fn migrate(connection: &mut Connection, definitions: &[(String, String)]) -> Result<(), MigrationError> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version == 0 {
        let existing: i64 =
            connection
                .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'", [], |row| row.get(0))?;
        if existing != 0 {
            return Err(MigrationError::SchemaMismatch);
        }
        connection.pragma_update(None, "auto_vacuum", "FULL")?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(
            "CREATE TABLE namespaces (
                namespace INTEGER PRIMARY KEY CHECK(namespace >= 0 AND namespace <= 255),
                directory TEXT NOT NULL,
                extension TEXT NOT NULL,
                UNIQUE(directory, extension)
            );
            CREATE TABLE entries (
                namespace INTEGER NOT NULL CHECK(namespace >= 0 AND namespace <= 511),
                name BLOB NOT NULL CHECK(length(name) = 16),
                size INTEGER NOT NULL CHECK(size >= 0),
                hash BLOB NOT NULL CHECK(length(hash) = 32),
                last_use INTEGER NOT NULL CHECK(last_use >= 0),
                use_count INTEGER NOT NULL CHECK(use_count >= 0),
                PRIMARY KEY(namespace, name)
            ) WITHOUT ROWID;",
        )?;
        insert_namespaces(&transaction, definitions)?;
        transaction.pragma_update(None, "user_version", INDEX_VERSION)?;
        transaction.commit()?;
        return Ok(());
    }
    if version != INDEX_VERSION {
        return Err(MigrationError::SchemaMismatch);
    }

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !valid_schema(&transaction)? {
        return Err(MigrationError::SchemaMismatch);
    }
    let mut statement = transaction.prepare("SELECT namespace, directory, extension FROM namespaces ORDER BY namespace")?;
    let mut rows = statement.query([])?;
    let mut previous = Vec::new();
    while let Some(row) = rows.next()? {
        let namespace: i64 = row.get(0)?;
        let directory: String = row.get(1)?;
        let extension: String = row.get(2)?;
        if namespace != i64::try_from(previous.len()).map_err(|_| RusqliteError::InvalidQuery)?
            || previous.len() >= 256
            || !valid_namespace(&directory)
            || !valid_extension(&extension)
            || previous.iter().any(|(old_directory, old_extension)| old_directory == &directory && old_extension == &extension)
        {
            return Err(MigrationError::SchemaMismatch);
        }
        previous.push((directory, extension));
    }
    drop(rows);
    drop(statement);
    let namespace_count = i64::try_from(previous.len()).map_err(|_| RusqliteError::InvalidQuery)?;
    let invalid_rows: i64 =
        transaction
            .query_row("SELECT COUNT(*) FROM entries WHERE namespace < 0 OR namespace >= ?1", [namespace_count], |row| row.get(0))?;
    if invalid_rows != 0 {
        return Err(MigrationError::SchemaMismatch);
    }
    if previous != definitions {
        transaction.execute("UPDATE entries SET namespace = namespace + 256", [])?;
        for (old_index, (directory, extension)) in previous.iter().enumerate() {
            let old_namespace = i64::try_from(old_index).map_err(|_| RusqliteError::InvalidQuery)? + 256;
            if let Some(new_index) = definitions
                .iter()
                .position(|candidate| candidate.0.as_str() == directory.as_str() && candidate.1.as_str() == extension.as_str())
            {
                let new_namespace = i64::try_from(new_index).map_err(|_| RusqliteError::InvalidQuery)?;
                transaction.execute("UPDATE entries SET namespace=?1 WHERE namespace=?2", params![new_namespace, old_namespace])?;
            } else {
                transaction.execute("DELETE FROM entries WHERE namespace=?1", [old_namespace])?;
            }
        }
        transaction.execute("DELETE FROM namespaces", [])?;
        insert_namespaces(&transaction, definitions)?;
    }
    transaction.commit()?;
    Ok(())
}

fn valid_schema(transaction: &Transaction<'_>) -> Result<bool, RusqliteError> {
    let mut tables = transaction.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
    let names = tables.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    if names != ["entries", "namespaces"] {
        return Ok(false);
    }
    let executable_objects: i64 =
        transaction.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type IN ('view', 'trigger')", [], |row| row.get(0))?;
    if executable_objects != 0 {
        return Ok(false);
    }
    if !valid_columns(
        transaction,
        "namespaces",
        &[("namespace", "INTEGER", 0, 1), ("directory", "TEXT", 1, 0), ("extension", "TEXT", 1, 0)],
    )? || !valid_columns(
        transaction,
        "entries",
        &[
            ("namespace", "INTEGER", 1, 1),
            ("name", "BLOB", 1, 2),
            ("size", "INTEGER", 1, 0),
            ("hash", "BLOB", 1, 0),
            ("last_use", "INTEGER", 1, 0),
            ("use_count", "INTEGER", 1, 0),
        ],
    )? {
        return Ok(false);
    }
    let indexes: i64 =
        transaction.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND tbl_name='entries'", [], |row| row.get(0))?;
    Ok(indexes == 0)
}

fn valid_columns(transaction: &Transaction<'_>, table: &str, expected: &[(&str, &str, i64, i64)]) -> Result<bool, RusqliteError> {
    let mut statement = transaction.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?, row.get::<_, i64>(5)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns
        .iter()
        .map(|(name, kind, required, primary)| (name.as_str(), kind.as_str(), *required, *primary))
        .eq(expected.iter().copied()))
}

fn insert_namespaces(transaction: &Transaction<'_>, definitions: &[(String, String)]) -> Result<(), RusqliteError> {
    let mut statement = transaction.prepare("INSERT INTO namespaces(namespace, directory, extension) VALUES (?1, ?2, ?3)")?;
    for (index, (directory, extension)) in definitions.iter().enumerate() {
        statement.execute(params![i64::try_from(index).map_err(|_| RusqliteError::InvalidQuery)?, directory, extension])?;
    }
    Ok(())
}

fn apply_mutations(transaction: &Transaction<'_>, mutations: Vec<Mutation>) -> Result<(), RusqliteError> {
    let mut put = transaction.prepare(
        "INSERT INTO entries(namespace, name, size, hash, last_use, use_count) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(namespace, name) DO UPDATE SET size=excluded.size, hash=excluded.hash, last_use=excluded.last_use, use_count=excluded.use_count",
    )?;
    let mut remove = transaction.prepare("DELETE FROM entries WHERE namespace=?1 AND name=?2")?;
    for mutation in mutations {
        match mutation {
            Mutation::Put(record) => {
                put.execute(params![
                    i64::from(record.key.namespace().0),
                    record.key.name().as_slice(),
                    sql_integer(record.size),
                    record.hash.as_slice(),
                    sql_integer(record.last_use),
                    sql_integer(record.use_count),
                ])?;
            }
            Mutation::Remove(key) => {
                remove.execute(params![i64::from(key.namespace().0), key.name().as_slice()])?;
            }
        }
    }
    Ok(())
}

fn blob<const N: usize>(bytes: Vec<u8>, column: usize) -> Result<[u8; N], RusqliteError> {
    bytes.try_into().map_err(|_| conversion_error(column, Type::Blob))
}

fn unsigned(value: i64, column: usize) -> Result<u64, RusqliteError> {
    u64::try_from(value).map_err(|_| conversion_error(column, Type::Integer))
}

fn conversion_error(column: usize, kind: Type) -> RusqliteError {
    RusqliteError::FromSqlConversionFailure(
        column,
        kind,
        Box::new(io::Error::new(io::ErrorKind::InvalidData, "invalid asset cache index row")) as Box<dyn Error + Send + Sync>,
    )
}

fn sql_integer(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

pub(super) fn map_sqlite_error(error: SqliteError) -> StorageError {
    match error {
        SqliteError::Busy => StorageError::Unavailable(UnavailableReason::Busy),
        SqliteError::QueueFull => StorageError::Unavailable(UnavailableReason::WriteQueueFull),
        SqliteError::WorkerStopped => StorageError::Unavailable(UnavailableReason::WriterStopped),
        SqliteError::BatchTooLarge { .. } => StorageError::TooLarge,
        SqliteError::Corrupt | SqliteError::SchemaMismatch => {
            StorageError::Io(io::Error::new(io::ErrorKind::InvalidData, "asset cache index was recreated"))
        }
        SqliteError::Unavailable(error) => StorageError::Io(error),
        SqliteError::Sqlite(RusqliteError::SqliteFailure(details, _)) if details.code == rusqlite::ffi::ErrorCode::OutOfMemory => {
            StorageError::Unavailable(UnavailableReason::Allocation)
        }
        SqliteError::Sqlite(error) => StorageError::Io(io::Error::other(error)),
    }
}

fn allocation_error() -> RusqliteError {
    RusqliteError::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::ErrorCode::OutOfMemory as i32),
        Some("asset cache index allocation failed".to_owned()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{Namespace, NamespaceTable};
    use tempfile::tempdir;

    fn record(value: u8, last_use: u64) -> Record {
        Record { key: Key::new(NamespaceId::new(0), [value; 16]), size: 0, hash: [0; 32], last_use, use_count: 0 }
    }

    #[test]
    fn load_records_recovers_an_interrupted_insert_batch() {
        let directory = tempdir().expect("cache directory");
        let namespaces = NamespaceTable::new(&[Namespace::new("items", "bin")]).expect("valid namespaces");
        let (index, records) = Index::open(directory.path().join("index.sqlite"), &namespaces).expect("create index");
        assert!(records.is_empty());
        let mutations = (1..=4).map(|value| Mutation::Put(record(value, u64::from(value)))).collect();
        index.apply(mutations).expect("write partial insertion batch");

        let recovered = index.load_records_with_limits(3, 1).expect("recover one excess row");

        assert_eq!(recovered.len(), 3);
        assert!(recovered.iter().all(|record| record.key != Key::new(NamespaceId::new(0), [1; 16])));
        assert!(recovered.iter().any(|record| record.key == Key::new(NamespaceId::new(0), [4; 16])));
        index.shutdown().expect("close index");
    }
}
