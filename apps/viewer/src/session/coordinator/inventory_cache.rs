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
        }
    }

    pub(super) fn start(&mut self, runtime: &Handle, grid: Grid, agent_id: Uuid, library_owner: Option<Uuid>, inventory: &Inventory) {
        if self.open_started || self.disabled {
            return;
        }
        self.open_started = true;
        let grid_identity = grid.id();
        let inventory = inventory.clone();
        let skeleton = inventory.snapshot();
        let sender = self.event_sender.clone();
        drop(runtime.spawn(async move {
            let opened = tokio::task::spawn_blocking(move || InventoryCache::open(grid_identity, agent_id, library_owner)).await;
            let event = match opened {
                Ok(Ok((cache, status))) => match cache.load(&inventory).await {
                    Ok(load) => {
                        let prepared = tokio::task::spawn_blocking(move || load.prepare_restore(&skeleton)).await;
                        match prepared {
                            Ok(Ok(restore)) => InventoryCacheEvent::Ready { cache, restore, status },
                            Ok(Err(error)) => InventoryCacheEvent::OpenFailed(error.to_string()),
                            Err(error) => InventoryCacheEvent::OpenFailed(error.to_string()),
                        }
                    }
                    Err(error) => InventoryCacheEvent::OpenFailed(error.to_string()),
                },
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
        self.restore = Some(PendingRestore { restore, excluded });
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
