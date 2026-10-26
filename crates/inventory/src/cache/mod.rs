mod read;
mod schema;
mod write;

use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use rusqlite::Transaction;
use sha2::{Digest, Sha256};
use storage::{
    StorageError, default_cache_dir,
    sqlite::{SqliteBatch, SqliteDb, SqliteError, SqliteReceipt},
};
use utils::hex_lower;
use uuid::Uuid;

use crate::{Category, CategoryKey, ChangeSet, Inventory, InventoryChild, InventoryError, InventoryLimits, InventorySnapshot};

pub use storage::sqlite::OpenStatus as InventoryCacheOpenStatus;

const MAX_READ_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug)]
pub enum InventoryCacheError {
    Storage(StorageError),
    Sqlite(SqliteError),
    InvalidIdentity,
    Inconsistent,
    Model(InventoryError),
}

impl fmt::Display for InventoryCacheError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => write!(formatter, "inventory cache storage is unavailable: {error}"),
            Self::Sqlite(error) => write!(formatter, "inventory cache database failed: {error}"),
            Self::InvalidIdentity => formatter.write_str("inventory cache identity is invalid"),
            Self::Inconsistent => formatter.write_str("inventory cache tree is inconsistent"),
            Self::Model(error) => write!(formatter, "cached inventory does not fit the model: {error}"),
        }
    }
}

impl Error for InventoryCacheError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            Self::Model(error) => Some(error),
            Self::InvalidIdentity | Self::Inconsistent => None,
        }
    }
}

impl From<StorageError> for InventoryCacheError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<SqliteError> for InventoryCacheError {
    fn from(error: SqliteError) -> Self {
        Self::Sqlite(error)
    }
}

impl From<InventoryError> for InventoryCacheError {
    fn from(error: InventoryError) -> Self {
        Self::Model(error)
    }
}

pub struct InventoryCache {
    db: SqliteDb,
    agent_id: Uuid,
    library_owner: Option<Uuid>,
    invalidated: AtomicBool,
}

impl fmt::Debug for InventoryCache {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("InventoryCache").field("db", &self.db).finish_non_exhaustive()
    }
}

pub struct InventoryCacheLoad {
    folders: Vec<CachedFolder>,
    discarded: bool,
}

#[derive(Debug)]
pub struct InventoryCacheWrite(SqliteReceipt<()>);

impl InventoryCacheWrite {
    pub async fn wait(self) -> Result<(), InventoryCacheError> {
        self.0.wait().await.map_err(Into::into)
    }
}

impl InventoryCacheLoad {
    pub fn was_discarded(&self) -> bool {
        self.discarded
    }

    pub fn apply_to(
        self,
        inventory: &mut Inventory,
        skeleton: &InventorySnapshot,
        excluded: &HashSet<CategoryKey>,
    ) -> Result<usize, InventoryCacheError> {
        if self.discarded {
            return Ok(0);
        }
        let mut folders = self.folders;
        let mut depths = HashMap::new();
        let mut ordered = Vec::new();
        ordered.try_reserve_exact(folders.len()).map_err(|_| InventoryCacheError::Inconsistent)?;
        for folder in folders.drain(..) {
            if let Some(depth) = category_depth(folder.category.key, skeleton, &mut depths) {
                ordered.push((depth, folder));
            }
        }
        ordered.sort_by_key(|(depth, folder)| (*depth, folder.category.key));

        let mut candidate = inventory.clone();
        let mut applied = 0usize;
        for (_, folder) in ordered {
            let key = folder.category.key;
            if excluded.contains(&key) {
                continue;
            }
            let Some(login_category) = matching_skeleton_category(&folder, skeleton) else {
                continue;
            };
            let children = restore_children(folder.children, key, skeleton);
            candidate.apply_folder_fetch(key, login_category.server_version, folder.category.server_descendent_count, children)?;
            applied = applied.saturating_add(1);
        }
        *inventory = candidate;
        Ok(applied)
    }
}

fn matching_skeleton_category<'a>(folder: &CachedFolder, skeleton: &'a InventorySnapshot) -> Option<&'a Category> {
    skeleton
        .category(folder.category.key)
        .filter(|category| category.server_version.is_known() && category.server_version == folder.category.server_version)
}

fn restore_children(children: Vec<InventoryChild>, parent: CategoryKey, skeleton: &InventorySnapshot) -> Vec<InventoryChild> {
    children
        .into_iter()
        .filter_map(|child| match child {
            InventoryChild::Category(category) => {
                skeleton.category(category.key).filter(|category| category.parent == Some(parent)).cloned().map(InventoryChild::Category)
            }
            item => Some(item),
        })
        .collect()
}

pub(super) struct CachedFolder {
    category: Category,
    children: Vec<InventoryChild>,
}

impl InventoryCache {
    pub fn open(
        grid_identity: &str,
        agent_id: Uuid,
        library_owner: Option<Uuid>,
    ) -> Result<(Self, InventoryCacheOpenStatus), InventoryCacheError> {
        let root = default_cache_dir()?;
        Self::open_at_root(root, grid_identity, agent_id, library_owner)
    }

    pub fn open_at_root(
        cache_root: impl AsRef<Path>,
        grid_identity: &str,
        agent_id: Uuid,
        library_owner: Option<Uuid>,
    ) -> Result<(Self, InventoryCacheOpenStatus), InventoryCacheError> {
        if !valid_identity(grid_identity) || agent_id.is_nil() || library_owner.is_some_and(|owner| owner.is_nil()) {
            return Err(InventoryCacheError::InvalidIdentity);
        }
        let grid_hash = hex_lower(&Sha256::digest(grid_identity.as_bytes()));
        let path = cache_root.as_ref().join("inventory").join(grid_hash).join(format!("{agent_id}.sqlite"));
        let migration_identity = grid_identity.to_owned();
        let (db, status) =
            SqliteDb::open(path, move |connection| schema::migrate(connection, &migration_identity, agent_id, library_owner))?;
        Ok((Self { db, agent_id, library_owner, invalidated: AtomicBool::new(false) }, status))
    }

    pub fn path(&self) -> &Path {
        self.db.path()
    }

    pub async fn load(&self, inventory: &Inventory) -> Result<InventoryCacheLoad, InventoryCacheError> {
        let roots = inventory.roots();
        let limits = inventory.limits();
        let agent_id = self.agent_id;
        let library_owner = self.library_owner;
        let receipt = self.db.read(0, move |transaction| read::read_tree(transaction, roots, agent_id, library_owner, limits))?;
        let load = receipt.wait().await?;
        if load.discarded {
            self.discard().await?;
        }
        Ok(load)
    }

    pub fn submit_bootstrap(&self, snapshot: &InventorySnapshot) -> Result<InventoryCacheWrite, InventoryCacheError> {
        let batches = match write::bootstrap(snapshot) {
            Ok(batches) => batches,
            Err(error) => {
                self.mark_generation_invalid();
                return Err(error.into());
            }
        };
        self.submit(batches).map(InventoryCacheWrite)
    }

    pub fn submit_changeset(
        &self,
        changes: &ChangeSet,
        before: &InventorySnapshot,
        after: &InventorySnapshot,
    ) -> Result<InventoryCacheWrite, InventoryCacheError> {
        let batches = match write::changeset(changes, before, after) {
            Ok(batches) => batches,
            Err(error) => {
                self.mark_generation_invalid();
                return Err(error.into());
            }
        };
        self.submit(batches).map(InventoryCacheWrite)
    }

    pub fn affected_categories(changes: &ChangeSet, before: &InventorySnapshot, after: &InventorySnapshot) -> Vec<CategoryKey> {
        write::affected_categories(changes, before, after)
    }

    pub async fn discard(&self) -> Result<(), InventoryCacheError> {
        self.invalidated.store(true, Ordering::Relaxed);
        self.db.discard()?.wait().await?;
        self.invalidated.store(false, Ordering::Relaxed);
        Ok(())
    }

    pub async fn close(self) -> Result<(), InventoryCacheError> {
        let Self { db, invalidated, .. } = self;
        let result = async {
            if invalidated.load(Ordering::Relaxed) || db.had_write_failure() {
                return Ok(());
            }
            db.read(0, |_| Ok(()))?.wait().await?;
            if invalidated.load(Ordering::Relaxed) || db.had_write_failure() {
                return Ok(());
            }
            let batch =
                SqliteBatch::new(1, 8, |transaction| transaction.execute("UPDATE meta SET clean_shutdown=1 WHERE id=1", []).map(|_| ()))?;
            db.submit_batches(vec![batch])?.wait().await?;
            Ok(())
        }
        .await;
        db.shutdown()?;
        result
    }

    fn submit(&self, batches: Vec<SqliteBatch>) -> Result<SqliteReceipt<()>, InventoryCacheError> {
        match self.db.submit_batches(batches) {
            Ok(receipt) => Ok(receipt),
            Err(error) => {
                self.mark_generation_invalid();
                Err(InventoryCacheError::Sqlite(error))
            }
        }
    }

    fn mark_generation_invalid(&self) {
        self.invalidated.store(true, Ordering::Relaxed);
        let batch = SqliteBatch::new(1, 8, |transaction: &Transaction<'_>| {
            transaction.execute("UPDATE meta SET cache_generation=cache_generation+1 WHERE id=1", []).map(|_| ())
        });
        match batch.and_then(|batch| self.db.submit_control_batch(batch)) {
            Ok(receipt) => drop(receipt),
            Err(error) => drop(error),
        }
    }
}

fn valid_identity(identity: &str) -> bool {
    !identity.is_empty()
        && identity.len() <= 64
        && identity.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_'))
}

fn category_depth(key: CategoryKey, snapshot: &InventorySnapshot, depths: &mut HashMap<CategoryKey, usize>) -> Option<usize> {
    if let Some(depth) = depths.get(&key) {
        return Some(*depth);
    }
    let mut current = key;
    let mut path = Vec::new();
    loop {
        if let Some(depth) = depths.get(&current).copied() {
            let mut next_depth = depth;
            for child in path.into_iter().rev() {
                next_depth = next_depth.checked_add(1)?;
                depths.insert(child, next_depth);
            }
            return depths.get(&key).copied();
        }
        let category = snapshot.category(current)?;
        path.push(current);
        match category.parent {
            Some(parent) => current = parent,
            None => {
                let mut next_depth = 0usize;
                for child in path.into_iter().rev() {
                    depths.insert(child, next_depth);
                    next_depth = next_depth.checked_add(1)?;
                }
                return depths.get(&key).copied();
            }
        }
        if path.len() > snapshot.categories().count() {
            return None;
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
