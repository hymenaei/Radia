mod read;
mod schema;
mod write;

use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt, fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use rusqlite::{Transaction, params};
use storage::{
    StorageError, default_cache_dir,
    sqlite::{SqliteBatch, SqliteControl, SqliteDb, SqliteError, SqliteReceipt},
};
use uuid::Uuid;

use crate::mutation::CachedFolderApply;
use crate::{Category, CategoryKey, ChangeSet, Inventory, InventoryChild, InventoryError, InventoryLimits, InventorySnapshot};

pub use storage::sqlite::OpenStatus as InventoryCacheOpenStatus;

const MAX_READ_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const MAX_RESTORE_UNITS_PER_TURN: usize = 128;
pub(crate) const MAX_RESTORE_ROWS_PER_TURN: usize = 512;
static LEGACY_CLEANUP_LOGGED: AtomicBool = AtomicBool::new(false);

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

#[derive(Clone)]
pub(super) struct CacheIdentity {
    pub(super) grid_id: String,
    pub(super) personal_owner: Uuid,
    pub(super) library_owner: Option<Uuid>,
}

impl CacheIdentity {
    fn owners(&self) -> impl Iterator<Item = Uuid> {
        std::iter::once(self.personal_owner).chain(self.library_owner)
    }
}

pub struct InventoryCache {
    db: SqliteDb,
    identity: CacheIdentity,
    invalidated: Arc<AtomicBool>,
}

impl fmt::Debug for InventoryCache {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("InventoryCache").field("db", &self.db).finish_non_exhaustive()
    }
}

pub struct InventoryCacheLoad {
    folders: Arc<[CachedFolder]>,
    discarded: bool,
}

pub struct InventoryCacheRestore {
    units: Vec<InventoryCacheRestoreUnit>,
    unit_index: usize,
    child_offset: usize,
    unit_conflict: bool,
}

struct InventoryCacheRestoreUnit {
    metadata: Category,
    children: Vec<InventoryChild>,
    complete: bool,
    expected_child_count: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InventoryCacheRestoreProgress {
    pub units_processed: usize,
    pub units_applied: usize,
    pub rows_processed: usize,
    pub changed: bool,
    pub finished: bool,
}

pub struct InventoryCacheWrite {
    result: Result<SqliteReceipt<()>, SqliteError>,
    control: SqliteControl,
    identity: CacheIdentity,
    owners: Vec<Uuid>,
}

impl fmt::Debug for InventoryCacheWrite {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InventoryCacheWrite")
    }
}

impl InventoryCacheWrite {
    pub async fn wait(self) -> Result<(), InventoryCacheError> {
        let result = match self.result {
            Ok(receipt) => receipt.wait().await,
            Err(error) => Err(error),
        };
        if result.is_err() {
            invalidate_scopes(&self.control, &self.identity, &self.owners).await?;
        }
        result.map_err(Into::into)
    }
}

impl InventoryCacheLoad {
    pub fn was_discarded(&self) -> bool {
        self.discarded
    }

    pub fn prepare_restore(self, skeleton: &InventorySnapshot) -> Result<InventoryCacheRestore, InventoryCacheError> {
        let mut depths = HashMap::new();
        let mut ordered = Vec::new();
        ordered.try_reserve_exact(self.folders.len()).map_err(|_| InventoryCacheError::Inconsistent)?;
        for folder in self.folders.iter() {
            if let Some(depth) = category_depth(folder.category.key, skeleton, &mut depths) {
                ordered.push((depth, folder));
            }
        }
        ordered.sort_by_key(|(depth, folder)| (*depth, folder.category.key));
        let mut units = Vec::new();
        units.try_reserve_exact(ordered.len()).map_err(|_| InventoryCacheError::Inconsistent)?;
        for (_, folder) in ordered {
            let key = folder.category.key;
            let Some(login_category) = skeleton.category(key) else {
                continue;
            };
            let (children, children_changed) = restore_children(&folder.children, key, skeleton);
            let version_matches =
                login_category.server_version.is_known() && login_category.server_version == folder.category.server_version;
            let complete = version_matches && folder.complete && !children_changed;
            let expected_child_count = children.len();
            units.push(InventoryCacheRestoreUnit { metadata: login_category.clone(), children, complete, expected_child_count });
        }
        Ok(InventoryCacheRestore { units, unit_index: 0, child_offset: 0, unit_conflict: false })
    }
}

impl InventoryCacheRestore {
    pub fn apply_next(
        &mut self,
        inventory: &mut Inventory,
        excluded: &HashSet<CategoryKey>,
    ) -> Result<InventoryCacheRestoreProgress, InventoryCacheError> {
        let starting_revision = inventory.revision();
        let mut progress = InventoryCacheRestoreProgress::default();
        while self.unit_index < self.units.len()
            && progress.units_processed < MAX_RESTORE_UNITS_PER_TURN
            && progress.rows_processed < MAX_RESTORE_ROWS_PER_TURN
        {
            let unit = &self.units[self.unit_index];
            if excluded.contains(&unit.metadata.key) {
                self.advance_unit();
                progress.units_processed = progress.units_processed.saturating_add(1);
                continue;
            }
            let remaining_rows = MAX_RESTORE_ROWS_PER_TURN.saturating_sub(progress.rows_processed);
            let available = unit.children.len().saturating_sub(self.child_offset);
            let chunk_len = available.min(remaining_rows);
            let end = self.child_offset.saturating_add(chunk_len);
            let final_chunk = end == unit.children.len();
            let result = inventory.apply_cached_folder(
                &unit.metadata,
                &unit.children[self.child_offset..end],
                unit.complete && !self.unit_conflict,
                unit.expected_child_count,
                final_chunk,
            )?;
            match result {
                CachedFolderApply::Skipped => {
                    self.advance_unit();
                    progress.units_processed = progress.units_processed.saturating_add(1);
                }
                CachedFolderApply::Applied { conflict } => {
                    self.unit_conflict |= conflict;
                    self.child_offset = end;
                    progress.rows_processed = progress.rows_processed.saturating_add(chunk_len);
                    if final_chunk {
                        self.advance_unit();
                        progress.units_processed = progress.units_processed.saturating_add(1);
                        progress.units_applied = progress.units_applied.saturating_add(1);
                    }
                }
            }
            if chunk_len == 0 && !final_chunk {
                return Err(InventoryCacheError::Inconsistent);
            }
        }
        progress.changed = inventory.revision() != starting_revision;
        progress.finished = self.unit_index == self.units.len();
        Ok(progress)
    }

    fn advance_unit(&mut self) {
        self.unit_index = self.unit_index.saturating_add(1);
        self.child_offset = 0;
        self.unit_conflict = false;
    }
}

fn restore_children(children: &[InventoryChild], parent: CategoryKey, skeleton: &InventorySnapshot) -> (Vec<InventoryChild>, bool) {
    let mut restored = Vec::new();
    let mut categories = HashSet::new();
    let mut changed = false;
    for child in children {
        match child {
            InventoryChild::Category(category) => {
                if let Some(current) = skeleton.category(category.key).filter(|current| current.parent == Some(parent)) {
                    categories.insert(current.key);
                    restored.push(InventoryChild::Category(current.clone()));
                } else {
                    changed = true;
                }
            }
            InventoryChild::Item(item) => restored.push(InventoryChild::Item(item.clone())),
        }
    }
    for category in skeleton.categories().filter(|category| category.parent == Some(parent)) {
        if categories.insert(category.key) {
            restored.push(InventoryChild::Category(category.clone()));
            changed = true;
        }
    }
    (restored, changed)
}

pub(super) struct CachedFolder {
    category: Category,
    children: Vec<InventoryChild>,
    complete: bool,
}

impl InventoryCache {
    pub fn open(
        grid_id: &str,
        agent_id: Uuid,
        library_owner: Option<Uuid>,
    ) -> Result<(Self, InventoryCacheOpenStatus), InventoryCacheError> {
        let root = default_cache_dir()?;
        Self::open_at_root(root, grid_id, agent_id, library_owner)
    }

    pub fn open_at_root(
        cache_root: impl AsRef<Path>,
        grid_id: &str,
        agent_id: Uuid,
        library_owner: Option<Uuid>,
    ) -> Result<(Self, InventoryCacheOpenStatus), InventoryCacheError> {
        if !valid_identity(grid_id) || agent_id.is_nil() || library_owner.is_some_and(|owner| owner.is_nil() || owner == agent_id) {
            return Err(InventoryCacheError::InvalidIdentity);
        }
        let cache_root = cache_root.as_ref();
        discard_legacy_layout(&cache_root.join("inventory"));
        let identity = CacheIdentity { grid_id: grid_id.to_owned(), personal_owner: agent_id, library_owner };
        let migration_identity = identity.clone();
        let (db, status) =
            SqliteDb::open(cache_root.join("inventory.sqlite"), move |connection| schema::migrate(connection, &migration_identity))?;
        Ok((Self { db, identity, invalidated: Arc::new(AtomicBool::new(false)) }, status))
    }

    pub fn path(&self) -> &Path {
        self.db.path()
    }

    pub async fn load(&self, inventory: &Inventory) -> Result<InventoryCacheLoad, InventoryCacheError> {
        if self.invalidated.load(Ordering::Relaxed) {
            return Ok(discarded());
        }
        let identity = self.identity.clone();
        let roots = inventory.roots();
        let limits = inventory.limits();
        let receipt = self.db.read(0, move |transaction| read::read_tree(transaction, &identity, roots, limits))?;
        let load = receipt.wait().await?;
        if load.discarded {
            self.invalidated.store(true, Ordering::Relaxed);
            let owners = self.identity.owners().collect::<Vec<_>>();
            invalidate_scopes(&self.db.control_handle(), &self.identity, &owners).await?;
        }
        Ok(load)
    }

    pub fn submit_bootstrap(&self, snapshot: &InventorySnapshot) -> Result<InventoryCacheWrite, InventoryCacheError> {
        let batches = write::bootstrap(snapshot, &self.identity)?;
        Ok(self.submit(batches, Vec::new()))
    }

    pub fn submit_changeset(
        &self,
        changes: &ChangeSet,
        before: &InventorySnapshot,
        after: &InventorySnapshot,
    ) -> Result<InventoryCacheWrite, InventoryCacheError> {
        let owners = write::affected_categories(changes, before, after)
            .into_iter()
            .map(|key| key.owner().id())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let batches = write::changeset(changes, before, after, &self.identity)?;
        Ok(self.submit(batches, owners))
    }

    pub fn affected_categories(changes: &ChangeSet, before: &InventorySnapshot, after: &InventorySnapshot) -> Vec<CategoryKey> {
        write::affected_categories(changes, before, after)
    }

    pub async fn discard(&self) -> Result<(), InventoryCacheError> {
        self.invalidated.store(true, Ordering::Relaxed);
        let owners = self.identity.owners().collect::<Vec<_>>();
        invalidate_scopes(&self.db.control_handle(), &self.identity, &owners).await?;
        Ok(())
    }

    pub async fn close(self) -> Result<(), InventoryCacheError> {
        let Self { db, identity, .. } = self;
        let result = async {
            let barrier = SqliteBatch::new(0, 0, |_: &Transaction<'_>| Ok(()))?;
            db.submit_control_batch(barrier)?.wait().await?;
            if db.had_write_failure() {
                let owners = identity.owners().collect::<Vec<_>>();
                invalidate_scopes(&db.control_handle(), &identity, &owners).await?;
            }
            Ok::<(), InventoryCacheError>(())
        }
        .await;
        db.shutdown()?;
        result
    }

    fn submit(&self, batches: Vec<SqliteBatch>, owners: Vec<Uuid>) -> InventoryCacheWrite {
        InventoryCacheWrite {
            result: self.db.submit_batches(batches),
            control: self.db.control_handle(),
            identity: self.identity.clone(),
            owners,
        }
    }
}

async fn invalidate_scopes(control: &SqliteControl, identity: &CacheIdentity, owners: &[Uuid]) -> Result<(), InventoryCacheError> {
    if owners.is_empty() {
        return Ok(());
    }
    let grid_id = identity.grid_id.clone();
    let rows = owners.len();
    let bytes = rows.saturating_mul(32);
    loop {
        let batch_grid = grid_id.clone();
        let batch_owners = owners.to_vec();
        let batch = SqliteBatch::new(rows, bytes, move |transaction: &Transaction<'_>| {
            let mut statement =
                transaction.prepare("UPDATE scopes SET cache_generation=cache_generation+1 WHERE grid_id=?1 AND owner_id=?2")?;
            for owner in batch_owners {
                statement.execute(params![batch_grid, owner.as_bytes().as_slice()])?;
            }
            Ok(())
        })?;
        let receipt = match control.submit_control_batch(batch) {
            Ok(receipt) => receipt,
            Err(SqliteError::QueueFull) => return Err(SqliteError::QueueFull.into()),
            Err(error) => return Err(error.into()),
        };
        match receipt.wait().await {
            Ok(()) => return Ok(()),
            Err(SqliteError::Busy) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

fn valid_identity(identity: &str) -> bool {
    !identity.is_empty() && identity.len() <= 64 && identity.bytes().all(|byte| byte.is_ascii_graphic())
}

fn discard_legacy_layout(path: &Path) {
    let Ok(grids) = fs::read_dir(path) else { return };
    let mut skipped = false;
    for grid in grids.flatten() {
        let Ok(file_type) = grid.file_type() else { continue };
        if !file_type.is_dir() || !valid_legacy_grid_dir(&grid.file_name().to_string_lossy()) {
            continue;
        }
        let Ok(agents) = fs::read_dir(grid.path()) else { continue };
        for agent in agents.flatten() {
            let Ok(file_type) = agent.file_type() else { continue };
            let agent_path = agent.path();
            let Some(name) = agent_path.file_name().and_then(|name| name.to_str()) else { continue };
            if !file_type.is_file() || !legacy_database_name(name) {
                continue;
            }
            match SqliteDb::remove_if_idle(&agent_path) {
                Ok(true) => {}
                Ok(false) | Err(_) => skipped = true,
            }
        }
        if let Err(error) = fs::remove_dir(grid.path())
            && error.kind() != std::io::ErrorKind::NotFound
            && has_entries(&grid.path())
        {
            skipped = true;
        }
    }
    if let Err(error) = fs::remove_dir(path)
        && error.kind() != std::io::ErrorKind::NotFound
        && has_entries(path)
    {
        skipped = true;
    }
    if skipped && !LEGACY_CLEANUP_LOGGED.swap(true, Ordering::Relaxed) {
        tracing::debug!("old inventory cache directories remain because their maintenance locks cannot be removed safely");
    }
}

fn has_entries(path: &Path) -> bool {
    fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some())
}

fn valid_legacy_grid_dir(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn legacy_database_name(name: &str) -> bool {
    name.strip_suffix(".sqlite").is_some_and(|stem| Uuid::parse_str(stem).is_ok())
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

fn discarded() -> InventoryCacheLoad {
    InventoryCacheLoad { folders: Vec::new().into(), discarded: true }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
