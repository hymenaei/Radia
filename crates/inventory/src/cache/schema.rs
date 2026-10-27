use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, params};
use storage::sqlite::MigrationError;

use super::CacheIdentity;

const SCHEMA_VERSION: i64 = 1;

const CREATE_SCHEMA: &str = "
CREATE TABLE scopes (
    scope_id INTEGER PRIMARY KEY,
    grid_id TEXT NOT NULL CHECK (length(grid_id) BETWEEN 1 AND 64),
    owner_id BLOB NOT NULL CHECK (length(owner_id) = 16),
    cache_generation INTEGER NOT NULL CHECK (cache_generation >= 1),
    UNIQUE (grid_id, owner_id)
);
CREATE TABLE categories (
    scope_id INTEGER NOT NULL,
    id BLOB NOT NULL CHECK (length(id) = 16),
    parent_id BLOB CHECK (parent_id IS NULL OR length(parent_id) = 16),
    preferred_type INTEGER NOT NULL,
    name TEXT NOT NULL,
    server_version INTEGER NOT NULL,
    server_descendent_count INTEGER CHECK (server_descendent_count IS NULL OR server_descendent_count BETWEEN 0 AND 4294967295),
    complete INTEGER NOT NULL CHECK (complete IN (0, 1)),
    complete_generation INTEGER NOT NULL,
    PRIMARY KEY (scope_id, id)
);
CREATE TABLE items (
    scope_id INTEGER NOT NULL,
    id BLOB NOT NULL CHECK (length(id) = 16),
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
    PRIMARY KEY (scope_id, id)
);
";

const SCOPE_COLUMNS: [&str; 4] = ["scope_id", "grid_id", "owner_id", "cache_generation"];
const CATEGORY_COLUMNS: [&str; 9] = [
    "scope_id",
    "id",
    "parent_id",
    "preferred_type",
    "name",
    "server_version",
    "server_descendent_count",
    "complete",
    "complete_generation",
];
const ITEM_COLUMNS: [&str; 21] = [
    "scope_id",
    "id",
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

pub(super) fn migrate(connection: &mut Connection, identity: &CacheIdentity) -> Result<(), MigrationError> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match version {
        0 => {
            if table_exists(connection, "meta")?
                || table_exists(connection, "scopes")?
                || table_exists(connection, "categories")?
                || table_exists(connection, "items")?
            {
                return Err(MigrationError::SchemaMismatch);
            }
            let transaction = connection.transaction()?;
            transaction.execute_batch(CREATE_SCHEMA)?;
            for owner in identity.owners() {
                transaction.execute(
                    "INSERT INTO scopes (grid_id, owner_id, cache_generation) VALUES (?1, ?2, 1)",
                    params![identity.grid_id, owner.as_bytes().as_slice()],
                )?;
            }
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            transaction.commit()?;
        }
        SCHEMA_VERSION => {
            require_columns(connection, "scopes", &SCOPE_COLUMNS)?;
            require_columns(connection, "categories", &CATEGORY_COLUMNS)?;
            require_columns(connection, "items", &ITEM_COLUMNS)?;
            let transaction = connection.transaction()?;
            for owner in identity.owners() {
                transaction.execute(
                    "INSERT OR IGNORE INTO scopes (grid_id, owner_id, cache_generation) VALUES (?1, ?2, 1)",
                    params![identity.grid_id, owner.as_bytes().as_slice()],
                )?;
            }
            transaction.commit()?;
        }
        _ => return Err(MigrationError::SchemaMismatch),
    }
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
    if columns.len() == expected.len() && expected.iter().all(|name| columns.contains(*name)) {
        Ok(())
    } else {
        Err(MigrationError::SchemaMismatch)
    }
}
