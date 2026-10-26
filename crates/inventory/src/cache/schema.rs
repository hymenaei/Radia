use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, params};
use storage::sqlite::MigrationError;
use uuid::Uuid;

const SCHEMA_VERSION: i64 = 1;

const CREATE_SCHEMA: &str = "
CREATE TABLE meta (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    schema_version INTEGER NOT NULL,
    grid_identity TEXT NOT NULL,
    agent_id BLOB NOT NULL CHECK (length(agent_id) = 16),
    library_owner_id BLOB CHECK (library_owner_id IS NULL OR length(library_owner_id) = 16),
    cache_generation INTEGER NOT NULL,
    clean_shutdown INTEGER NOT NULL CHECK (clean_shutdown IN (0, 1))
);
CREATE TABLE categories (
    owner_id BLOB NOT NULL CHECK (length(owner_id) = 16),
    category_id BLOB NOT NULL CHECK (length(category_id) = 16),
    parent_id BLOB CHECK (parent_id IS NULL OR length(parent_id) = 16),
    preferred_type INTEGER NOT NULL,
    name TEXT NOT NULL,
    server_version INTEGER NOT NULL,
    server_descendent_count INTEGER CHECK (server_descendent_count IS NULL OR server_descendent_count BETWEEN 0 AND 4294967295),
    complete INTEGER NOT NULL CHECK (complete IN (0, 1)),
    complete_generation INTEGER NOT NULL,
    PRIMARY KEY (owner_id, category_id)
);
CREATE INDEX categories_by_parent ON categories(owner_id, parent_id);
CREATE TABLE items (
    owner_id BLOB NOT NULL CHECK (length(owner_id) = 16),
    item_id BLOB NOT NULL CHECK (length(item_id) = 16),
    parent_id BLOB NOT NULL CHECK (length(parent_id) = 16),
    asset_id BLOB NOT NULL CHECK (length(asset_id) = 16),
    asset_type INTEGER NOT NULL,
    inventory_type INTEGER NOT NULL,
    flags INTEGER NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    creator_id BLOB NOT NULL CHECK (length(creator_id) = 16),
    item_owner_id BLOB NOT NULL CHECK (length(item_owner_id) = 16),
    group_id BLOB NOT NULL CHECK (length(group_id) = 16),
    last_owner_id BLOB NOT NULL CHECK (length(last_owner_id) = 16),
    base_mask INTEGER NOT NULL,
    owner_mask INTEGER NOT NULL,
    group_mask INTEGER NOT NULL,
    everyone_mask INTEGER NOT NULL,
    next_owner_mask INTEGER NOT NULL,
    sale_type INTEGER NOT NULL,
    sale_price INTEGER NOT NULL,
    PRIMARY KEY (owner_id, item_id)
);
CREATE INDEX items_by_parent ON items(owner_id, parent_id);
";

const META_COLUMNS: [&str; 7] =
    ["id", "schema_version", "grid_identity", "agent_id", "library_owner_id", "cache_generation", "clean_shutdown"];
const CATEGORY_COLUMNS: [&str; 9] = [
    "owner_id",
    "category_id",
    "parent_id",
    "preferred_type",
    "name",
    "server_version",
    "server_descendent_count",
    "complete",
    "complete_generation",
];
const ITEM_COLUMNS: [&str; 21] = [
    "owner_id",
    "item_id",
    "parent_id",
    "asset_id",
    "asset_type",
    "inventory_type",
    "flags",
    "name",
    "description",
    "created_at",
    "creator_id",
    "item_owner_id",
    "group_id",
    "last_owner_id",
    "base_mask",
    "owner_mask",
    "group_mask",
    "everyone_mask",
    "next_owner_mask",
    "sale_type",
    "sale_price",
];

pub(super) fn migrate(
    connection: &mut Connection,
    grid_identity: &str,
    agent_id: Uuid,
    library_owner: Option<Uuid>,
) -> Result<(), MigrationError> {
    if !table_exists(connection, "meta")? {
        if table_exists(connection, "categories")? || table_exists(connection, "items")? {
            return Err(MigrationError::SchemaMismatch);
        }
        let transaction = connection.transaction()?;
        transaction.execute_batch(CREATE_SCHEMA)?;
        transaction.execute(
            "INSERT INTO meta (id, schema_version, grid_identity, agent_id, library_owner_id, cache_generation, clean_shutdown)
             VALUES (1, ?1, ?2, ?3, ?4, 1, 0)",
            params![SCHEMA_VERSION, grid_identity, agent_id.as_bytes().as_slice(), library_owner.map(|owner| owner.as_bytes().to_vec())],
        )?;
        transaction.commit()?;
        return Ok(());
    }

    require_columns(connection, "meta", &META_COLUMNS)?;
    require_columns(connection, "categories", &CATEGORY_COLUMNS)?;
    require_columns(connection, "items", &ITEM_COLUMNS)?;
    let metadata = connection
        .query_row(
            "SELECT schema_version, grid_identity, agent_id, library_owner_id, cache_generation, clean_shutdown FROM meta WHERE id=1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Option<Vec<u8>>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()
        .map_err(schema_or_sqlite)?
        .ok_or(MigrationError::SchemaMismatch)?;
    let (schema_version, stored_grid, stored_agent, stored_library, mut generation, clean_shutdown) = metadata;
    let stored_library = stored_library.map(|value| Uuid::from_slice(&value).map_err(|_| MigrationError::SchemaMismatch)).transpose()?;
    if schema_version != SCHEMA_VERSION
        || stored_grid != grid_identity
        || stored_agent.as_slice() != agent_id.as_bytes()
        || stored_library != library_owner
        || generation < 1
        || !matches!(clean_shutdown, 0 | 1)
    {
        return Err(MigrationError::SchemaMismatch);
    }

    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "CREATE INDEX IF NOT EXISTS categories_by_parent ON categories(owner_id, parent_id);
         CREATE INDEX IF NOT EXISTS items_by_parent ON items(owner_id, parent_id);",
    )?;
    if clean_shutdown == 0 {
        generation = generation.checked_add(1).ok_or(MigrationError::SchemaMismatch)?;
    }
    transaction.execute("UPDATE meta SET cache_generation=?1, clean_shutdown=0 WHERE id=1", [generation])?;
    transaction.commit()?;
    Ok(())
}

fn table_exists(connection: &Connection, name: &str) -> Result<bool, MigrationError> {
    Ok(connection.query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1", [name], |_| Ok(())).optional()?.is_some())
}

fn require_columns(connection: &Connection, table: &str, expected: &[&str]) -> Result<(), MigrationError> {
    if !table_exists(connection, table)? {
        return Err(MigrationError::SchemaMismatch);
    }
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?.collect::<Result<HashSet<_>, _>>()?;
    if expected.iter().all(|name| columns.contains(*name)) { Ok(()) } else { Err(MigrationError::SchemaMismatch) }
}

fn schema_or_sqlite(error: rusqlite::Error) -> MigrationError {
    let is_busy = matches!(
        &error,
        rusqlite::Error::SqliteFailure(details, _) if matches!(
            details.code,
            rusqlite::ffi::ErrorCode::DatabaseBusy | rusqlite::ffi::ErrorCode::DatabaseLocked
        )
    );
    if is_busy { MigrationError::Sqlite(error) } else { MigrationError::SchemaMismatch }
}
