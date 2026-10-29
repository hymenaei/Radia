use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use ::inventory::{
    CategoryKey, ChangeSet, Inventory, InventoryCache, InventoryCacheOpenStatus, InventoryCacheRestore, InventoryCacheWrite,
    InventoryChange, InventorySnapshot,
};
use network::Grid;
use tokio::{runtime::Handle, sync::mpsc};
use uuid::Uuid;

const MAX_PENDING_WRITES: usize = 32;
const MAX_PENDING_ROWS: usize = 65_536;
const MAX_PENDING_BYTES: usize = 16 * 1024 * 1024;

pub(super) struct InventoryCacheSession {
    event_sender: mpsc::UnboundedSender<InventoryCacheEvent>,
    event_receiver: mpsc::UnboundedReceiver<InventoryCacheEvent>,
    warning_emitted: Arc<AtomicBool>,
    cache: Option<InventoryCache>,
    pending_writes: Vec<PendingWrite>,
    pending_rows: usize,
    pending_bytes: usize,
    open_started: bool,
    pending_overflow: bool,
    disabled: bool,
    restore: Option<PendingRestore>,
    restore_invalidated: bool,
}

struct PendingRestore {
    restore: InventoryCacheRestore,
    excluded: HashSet<CategoryKey>,
}

struct PendingWrite {
    changes: ChangeSet,
    before: InventorySnapshot,
    after: InventorySnapshot,
    rows: usize,
    bytes: usize,
}

pub(super) enum InventoryCacheEvent {
    Ready { cache: InventoryCache, restore: InventoryCacheRestore, status: InventoryCacheOpenStatus },
    OpenFailed(String),
    RestoreBatch,
}

pub(super) struct InventoryCacheEventOutcome {
    pub(super) snapshot: Option<InventorySnapshot>,
}

impl InventoryCacheSession {
    pub(super) fn new() -> Self {
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        Self {
            event_sender,
            event_receiver,
            warning_emitted: Arc::new(AtomicBool::new(false)),
            cache: None,
            pending_writes: Vec::new(),
            pending_rows: 0,
            pending_bytes: 0,
            open_started: false,
            pending_overflow: false,
            disabled: false,
            restore: None,
            restore_invalidated: false,
        }
    }

    pub(super) fn start(&mut self, runtime: &Handle, grid: Grid, agent_id: Uuid, library_owner: Option<Uuid>, inventory: &Inventory) {
        let grid_identity = grid.id();
        self.start_with_open(runtime, inventory, move || InventoryCache::open(grid_identity, agent_id, library_owner));
    }

    fn start_with_open(
        &mut self,
        runtime: &Handle,
        inventory: &Inventory,
        open: impl FnOnce() -> Result<(InventoryCache, InventoryCacheOpenStatus), ::inventory::InventoryCacheError> + Send + 'static,
    ) {
        if self.open_started || self.disabled {
            return;
        }
        self.open_started = true;
        let invalidate_cache = self.restore_invalidated;
        let inventory = inventory.clone();
        let skeleton = inventory.snapshot();
        let sender = self.event_sender.clone();
        drop(runtime.spawn(async move {
            let opened = tokio::task::spawn_blocking(open).await;
            let event = match opened {
                Ok(Ok((cache, status))) => {
                    if invalidate_cache && let Err(error) = cache.discard().await {
                        InventoryCacheEvent::OpenFailed(error.to_string())
                    } else {
                        match cache.load(&inventory).await {
                            Ok(load) => {
                                let prepared = tokio::task::spawn_blocking(move || load.prepare_restore(&skeleton)).await;
                                match prepared {
                                    Ok(Ok(restore)) => InventoryCacheEvent::Ready { cache, restore, status },
                                    Ok(Err(error)) => InventoryCacheEvent::OpenFailed(error.to_string()),
                                    Err(error) => InventoryCacheEvent::OpenFailed(error.to_string()),
                                }
                            }
                            Err(error) => InventoryCacheEvent::OpenFailed(error.to_string()),
                        }
                    }
                }
                Ok(Err(error)) => InventoryCacheEvent::OpenFailed(error.to_string()),
                Err(error) => InventoryCacheEvent::OpenFailed(error.to_string()),
            };
            if let Err(send_error) = sender.send(event) {
                drop(send_error);
            }
        }));
    }

    pub(super) async fn next_event(&mut self) -> Option<InventoryCacheEvent> {
        if self.restore.is_some() {
            return Some(InventoryCacheEvent::RestoreBatch);
        }
        self.event_receiver.recv().await
    }

    pub(super) fn invalidate_restore(&mut self) {
        self.restore_invalidated = true;
        self.restore = None;
    }

    pub(super) fn record_change(&mut self, runtime: &Handle, changes: ChangeSet, before: InventorySnapshot, after: InventorySnapshot) {
        if changes.is_empty() || self.disabled {
            return;
        }
        if let Some(restore) = &mut self.restore {
            restore.excluded.extend(InventoryCache::affected_categories(&changes, &before, &after));
        }
        if let Some(cache) = &self.cache {
            match cache.submit_changeset(&changes, &before, &after) {
                Ok(receipt) => monitor_write(runtime, receipt, Arc::clone(&self.warning_emitted)),
                Err(error) => self.warn_once(&error),
            }
            return;
        }
        if !self.open_started || self.pending_overflow {
            return;
        }
        let (rows, bytes) = estimate_write(&changes);
        let next_rows = self.pending_rows.saturating_add(rows);
        let next_bytes = self.pending_bytes.saturating_add(bytes);
        if self.pending_writes.len() >= MAX_PENDING_WRITES || next_rows > MAX_PENDING_ROWS || next_bytes > MAX_PENDING_BYTES {
            self.pending_writes.clear();
            self.pending_rows = 0;
            self.pending_bytes = 0;
            self.pending_overflow = true;
            return;
        }
        self.pending_rows = next_rows;
        self.pending_bytes = next_bytes;
        self.pending_writes.push(PendingWrite { changes, before, after, rows, bytes });
    }

    pub(super) fn handle_event(
        &mut self,
        runtime: &Handle,
        event: InventoryCacheEvent,
        inventory: &mut Inventory,
    ) -> InventoryCacheEventOutcome {
        match event {
            InventoryCacheEvent::OpenFailed(error) => {
                self.warn_once(&error);
                self.disabled = true;
                self.clear_pending_writes();
                InventoryCacheEventOutcome { snapshot: None }
            }
            InventoryCacheEvent::Ready { cache, restore, status } => {
                self.handle_ready(runtime, cache, restore, status, inventory);
                InventoryCacheEventOutcome { snapshot: None }
            }
            InventoryCacheEvent::RestoreBatch => {
                let before = inventory.revision();
                let result = self.restore.as_mut().map(|restore| restore.restore.apply_next(inventory, &restore.excluded));
                match result {
                    Some(Ok(progress)) => {
                        if progress.finished {
                            self.restore = None;
                            tracing::debug!("inventory cache restore finished");
                        }
                        InventoryCacheEventOutcome { snapshot: progress.changed.then(|| inventory.snapshot()) }
                    }
                    Some(Err(error)) => {
                        self.restore = None;
                        self.warn_once(&error);
                        self.discard_cache(runtime);
                        InventoryCacheEventOutcome { snapshot: (inventory.revision() != before).then(|| inventory.snapshot()) }
                    }
                    None => InventoryCacheEventOutcome { snapshot: None },
                }
            }
        }
    }

    pub(super) async fn close(mut self) {
        if let Some(cache) = self.cache.take()
            && let Err(error) = cache.close().await
        {
            self.warn_once(&error);
        }
    }

    fn handle_ready(
        &mut self,
        runtime: &Handle,
        cache: InventoryCache,
        restore: InventoryCacheRestore,
        status: InventoryCacheOpenStatus,
        inventory: &mut Inventory,
    ) {
        if self.pending_overflow {
            self.disabled = true;
            self.clear_pending_writes();
            self.discard_in_background(runtime, cache);
            return;
        }
        tracing::debug!(?status, "inventory cache opened");
        let mut excluded = HashSet::new();
        for pending in &self.pending_writes {
            excluded.extend(InventoryCache::affected_categories(&pending.changes, &pending.before, &pending.after));
        }
        self.cache = Some(cache);
        if let Some(cache) = &self.cache {
            let warning_emitted = Arc::clone(&self.warning_emitted);
            for pending in self.pending_writes.drain(..) {
                match cache.submit_changeset(&pending.changes, &pending.before, &pending.after) {
                    Ok(receipt) => monitor_write(runtime, receipt, Arc::clone(&self.warning_emitted)),
                    Err(error) if !warning_emitted.swap(true, Ordering::Relaxed) => {
                        tracing::warn!(%error, "inventory cache write failed");
                    }
                    Err(_) => {}
                }
                self.pending_rows = self.pending_rows.saturating_sub(pending.rows);
                self.pending_bytes = self.pending_bytes.saturating_sub(pending.bytes);
            }
            let snapshot = inventory.snapshot();
            match cache.submit_bootstrap(&snapshot) {
                Ok(receipt) => monitor_write(runtime, receipt, Arc::clone(&self.warning_emitted)),
                Err(error) => self.warn_once(&error),
            }
        }
        if !self.restore_invalidated {
            self.restore = Some(PendingRestore { restore, excluded });
        }
    }

    fn clear_pending_writes(&mut self) {
        self.pending_writes.clear();
        self.pending_rows = 0;
        self.pending_bytes = 0;
    }

    fn discard_cache(&mut self, runtime: &Handle) {
        self.disabled = true;
        if let Some(cache) = self.cache.take() {
            self.discard_in_background(runtime, cache);
        }
    }

    fn discard_in_background(&mut self, runtime: &Handle, cache: InventoryCache) {
        let warning_emitted = Arc::clone(&self.warning_emitted);
        drop(runtime.spawn(async move {
            if let Err(error) = cache.discard().await
                && !warning_emitted.swap(true, Ordering::Relaxed)
            {
                tracing::warn!(%error, "inventory cache was discarded");
            }
            if let Err(error) = cache.close().await
                && !warning_emitted.swap(true, Ordering::Relaxed)
            {
                tracing::warn!(%error, "inventory cache shutdown failed");
            }
        }));
    }

    fn warn_once(&self, error: &impl std::fmt::Display) {
        if !self.warning_emitted.swap(true, Ordering::Relaxed) {
            tracing::warn!(%error, "inventory cache is unavailable for this session");
        }
    }
}

fn monitor_write(runtime: &Handle, receipt: InventoryCacheWrite, warning_emitted: Arc<AtomicBool>) {
    drop(runtime.spawn(async move {
        if let Err(error) = receipt.wait().await
            && !warning_emitted.swap(true, Ordering::Relaxed)
        {
            tracing::warn!(%error, "inventory cache write failed");
        }
    }));
}

fn estimate_write(changes: &ChangeSet) -> (usize, usize) {
    let mut rows = changes.refetch_categories.len();
    let mut bytes = rows.saturating_mul(64);
    for change in &changes.changes {
        let row_bytes = match change {
            InventoryChange::CategoryUpserted(category) => 96usize.saturating_add(category.name.len()),
            InventoryChange::ItemUpserted(item) => 256usize.saturating_add(item.name.len()).saturating_add(item.description.len()),
            InventoryChange::NodeMoved { .. }
            | InventoryChange::CategoryRemoved(_)
            | InventoryChange::ItemRemoved(_)
            | InventoryChange::FolderFetchApplied { .. }
            | InventoryChange::VersionSet { .. } => 64,
        };
        rows = rows.saturating_add(1);
        bytes = bytes.saturating_add(row_bytes);
    }
    (rows, bytes)
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, sync::Arc};

    use ::inventory::{
        AssetType, Category, CategoryKey, FolderType, Inventory, InventoryChild, InventoryLimits, InventoryOwner, InventoryType, Item,
        ItemKey, PermissionMask, Permissions, SaleInfo, SaleType, ServerVersion,
    };
    use tempfile::tempdir;
    use tokio::runtime::Handle;
    use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
    use uuid::Uuid;

    use super::{InventoryCache, InventoryCacheEvent, InventoryCacheSession};

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }

    fn category(owner: InventoryOwner, id: u128, parent: Option<u128>, version: i32) -> Category {
        Category {
            key: CategoryKey::new(owner, FolderId::new(uuid(id))),
            parent: parent.map(|parent| CategoryKey::new(owner, FolderId::new(uuid(parent)))),
            preferred_folder_type: FolderType::None,
            name: Arc::from(format!("folder-{id}")),
            server_version: ServerVersion::from_raw(version),
            server_descendent_count: None,
        }
    }

    fn item(owner: InventoryOwner, id: u128, parent: CategoryKey) -> Item {
        Item {
            key: ItemKey::new(owner, ItemId::new(uuid(id))),
            parent,
            asset_id: AssetId::new(uuid(id + 100)),
            asset_type: AssetType::Notecard,
            inventory_type: InventoryType::Notecard,
            flags: 0,
            name: Arc::from("cached item"),
            description: Arc::from("cached description"),
            created_at: 0,
            permissions: Permissions {
                creator: AvatarId::new(Uuid::nil()),
                owner: AvatarId::new(Uuid::nil()),
                group: GroupId::new(Uuid::nil()),
                last_owner: AvatarId::new(Uuid::nil()),
                base: PermissionMask::new(0),
                owner_mask: PermissionMask::new(0),
                group_mask: PermissionMask::new(0),
                everyone: PermissionMask::new(0),
                next_owner: PermissionMask::new(0),
            },
            sale: SaleInfo { sale_type: SaleType::NotForSale, price: 0 },
        }
    }

    fn cache_skeleton(owner: InventoryOwner) -> (Inventory, CategoryKey, CategoryKey) {
        let root = category(owner, 1, None, 7);
        let child = category(owner, 2, Some(1), 4);
        let root_key = root.key;
        let child_key = child.key;
        let mut inventory = Inventory::new(root, None, InventoryLimits::default()).expect("valid inventory root");
        inventory.upsert_category(child).expect("valid child folder");
        (inventory, root_key, child_key)
    }

    #[tokio::test]
    async fn early_delta_invalidation_survives_the_next_login() {
        let directory = tempdir().expect("create inventory cache directory");
        let agent = uuid(10);
        let owner = InventoryOwner::new(agent);
        let (mut seeded, root, child) = cache_skeleton(owner);
        let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", agent, None).expect("open inventory cache");
        let bootstrap = seeded.snapshot();
        cache.submit_bootstrap(&bootstrap).expect("write login skeleton").wait().await.expect("skeleton write completes");

        let before_root = seeded.snapshot();
        let root_changes = seeded
            .apply_folder_fetch(root, ServerVersion::from_raw(7), Some(1), vec![InventoryChild::Category(category(owner, 2, Some(1), 4))])
            .expect("complete root folder");
        let after_root = seeded.snapshot();
        cache
            .submit_changeset(&root_changes, &before_root, &after_root)
            .expect("write root folder completion")
            .wait()
            .await
            .expect("root completion write succeeds");

        let before_child = seeded.snapshot();
        let stale_item = item(owner, 3, child);
        let stale_item_key = stale_item.key;
        let child_changes = seeded
            .apply_folder_fetch(child, ServerVersion::from_raw(4), Some(1), vec![InventoryChild::Item(stale_item)])
            .expect("complete child folder");
        let after_child = seeded.snapshot();
        cache
            .submit_changeset(&child_changes, &before_child, &after_child)
            .expect("write cached child contents")
            .wait()
            .await
            .expect("child completion write succeeds");
        assert!(after_child.is_complete(child));
        cache.close().await.expect("close seeded cache");

        let (mut next_login, next_root, next_child) = cache_skeleton(owner);
        let mut cache_session = InventoryCacheSession::new();
        cache_session.invalidate_restore();
        let cache_root = directory.path().to_path_buf();
        cache_session
            .start_with_open(&Handle::current(), &next_login, move || InventoryCache::open_at_root(cache_root, "agni", agent, None));
        let event = cache_session.next_event().await.expect("cache open event arrives");
        assert!(matches!(event, InventoryCacheEvent::Ready { .. }));
        cache_session.handle_event(&Handle::current(), event, &mut next_login);
        cache_session.close().await;

        let (cache, _) = InventoryCache::open_at_root(directory.path(), "agni", agent, None).expect("reopen inventory cache");
        let load = cache.load(&next_login).await.expect("load invalidated cache");
        let mut restore = load.prepare_restore(&next_login.snapshot()).expect("prepare next login restore");
        loop {
            let progress = restore.apply_next(&mut next_login, &HashSet::new()).expect("apply cached folder batch");
            if progress.finished {
                break;
            }
        }
        let snapshot = next_login.snapshot();
        assert_eq!(snapshot.roots().personal(), next_root);
        assert_eq!(snapshot.item(stale_item_key).map(|item| item.parent), Some(next_child));
        assert!(!snapshot.is_complete(next_child));
        cache.close().await.expect("close reopened cache");
    }
}
