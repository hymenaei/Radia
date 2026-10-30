use std::collections::{HashMap, HashSet};

use ::inventory::{
    CategoryKey, ChangeSet, Inventory, InventoryChange, InventoryChild, InventoryDelta, InventoryError, InventoryNodeId, InventoryNodeKey,
    InventorySnapshot, ServerVersion,
};
use futures::{FutureExt, pin_mut};
use network::{AgentSession, capabilities::Client};
use protocol::lludp::Message;
use tracing::Instrument;

use crate::{
    inventory::{
        AisDelta, FolderApplyOutcome, FolderFetch, InventoryEditEvent, InventoryFetchError, InventoryFetchEvent, InventoryLoadingStatus,
        MoveExpectation, favorites::load_favorites,
    },
    session::{
        RegionLookup,
        coordinator::{
            inventory_bootstrap::InventoryBootstrapReport,
            inventory_intake::{self, InventoryIntakeError, InventoryIntakeOutcome},
            presentation::{FavoritesUpdate, ViewerUpdate},
        },
    },
};

use super::SessionDriver;

impl SessionDriver {
    pub(super) fn handle_inventory_udp(&mut self, message: &Message) {
        let Some(agent_id) = self.active_session.as_ref().map(|session| session.agent_id) else { return };
        let Some(before) = self.inventory.as_ref().map(Inventory::snapshot) else {
            if self.inventory_bootstrap.is_some() && inventory_intake::is_inventory_udp_message(message, agent_id) {
                self.inventory_bootstrap_dirty = true;
            }
            return;
        };
        let Some(inventory) = self.inventory.as_mut() else { return };
        let result = inventory_intake::apply_udp(inventory, agent_id, message);
        self.apply_inventory_intake_result(result, before);
    }

    pub(super) fn handle_inventory_event_queue(&mut self, name: &str, body: &llsd::Value) {
        let Some(agent_id) = self.active_session.as_ref().map(|session| session.agent_id) else { return };
        let Some(before) = self.inventory.as_ref().map(Inventory::snapshot) else {
            if self.inventory_bootstrap.is_some() && inventory_intake::is_valid_inventory_event(name, body, agent_id) {
                self.inventory_bootstrap_dirty = true;
            }
            return;
        };
        let Some(inventory) = self.inventory.as_mut() else { return };
        let result = inventory_intake::apply_event_queue(inventory, agent_id, name, body);
        self.apply_inventory_intake_result(result, before);
    }

    fn apply_inventory_intake_result(
        &mut self,
        result: Result<Option<InventoryIntakeOutcome>, InventoryIntakeError>,
        before: InventorySnapshot,
    ) {
        match result {
            Ok(Some(outcome)) => {
                self.note_rejected_inventory_blocks(outcome.rejected_blocks);
                if !outcome.changes.is_empty()
                    && let Some(snapshot) = self.inventory.as_ref().map(Inventory::snapshot)
                {
                    self.process_inventory_changes(&outcome.changes, &snapshot);
                    self.inventory_changes.send_replace(Some(snapshot.clone()));
                    self.request_inventory_refetches(&outcome.changes, &snapshot);
                    self.publish_inventory_status();
                    self.inventory_cache.record_change(&self.runtime, outcome.changes, before, snapshot);
                }
            }
            Ok(None) => {}
            Err(error) => self.note_rejected_inventory_blocks_with_error(error),
        }
    }

    fn note_rejected_inventory_blocks(&mut self, count: usize) {
        self.rejected_inventory_blocks = self.rejected_inventory_blocks.saturating_add(count as u64);
        if count > 0 && self.rejected_inventory_blocks / 64 != (self.rejected_inventory_blocks - count as u64) / 64 {
            tracing::warn!(rejected_blocks = self.rejected_inventory_blocks, "inventory updates contain rejected blocks");
        }
    }

    fn note_rejected_inventory_blocks_with_error(&mut self, error: InventoryIntakeError) {
        self.rejected_inventory_blocks = self.rejected_inventory_blocks.saturating_add(1);
        if self.rejected_inventory_blocks.is_multiple_of(64) {
            tracing::warn!(rejected_blocks = self.rejected_inventory_blocks, %error, "inventory updates were rejected");
        }
    }

    pub(super) fn finish_inventory_bootstrap(&mut self, inventory: Inventory, report: InventoryBootstrapReport) {
        if report.rejected_personal_entries > 0 || report.rejected_library_entries > 0 {
            tracing::warn!(
                personal_rejected = report.rejected_personal_entries,
                library_rejected = report.rejected_library_entries,
                "login inventory skeleton contained rejected folders"
            );
        }
        if self.inventory_bootstrap_dirty {
            self.inventory_cache.invalidate_restore();
            tracing::warn!(
                "early inventory updates arrived before login inventory was ready; cached contents will be refreshed from the simulator"
            );
        }
        if let Some(session) = &self.active_session {
            let library_owner = inventory.roots().library().map(|root| root.owner().id());
            self.inventory_cache.start(&self.runtime, self.credentials.grid, session.agent_id, library_owner, &inventory);
        }
        self.inventory_changes.send_replace(Some(inventory.snapshot()));
        self.inventory = Some(inventory);
        self.inventory_bootstrap_dirty = false;
        if let Some(capabilities) = self.active_capabilities.clone() {
            self.start_inventory_edits(capabilities.clone());
            self.start_inventory_fetch(capabilities);
        }
        if let Some(simulator_id) = self.primary_simulator
            && let Some(session) = self.active_session.clone()
        {
            self.publish_session_ready_update(simulator_id, session);
        }
        self.start_favorites_load_for_active();
    }

    pub(super) fn start_favorites_load_for_active(&self) {
        if !self.favorites_enabled {
            return;
        }
        let (Some(session), Some(capabilities), Some(lookup)) =
            (self.active_session.clone(), self.active_capabilities.clone(), self.region_lookup.clone())
        else {
            return;
        };
        let inventory = match self.inventory.as_ref() {
            Some(inventory) => Some(inventory.snapshot()),
            None if self.inventory_bootstrap.is_some() => return,
            None => None,
        };
        self.spawn_favorites_load(session, capabilities, lookup, inventory);
    }

    fn spawn_favorites_load(
        &self,
        session: AgentSession,
        capabilities: Client,
        lookup: RegionLookup,
        inventory: Option<InventorySnapshot>,
    ) {
        let mailbox = self.mailbox.clone();
        let username = self.credentials.username.clone();
        let cancelled = self.cancellation.clone();
        drop(
            self.runtime.spawn(
                async move {
                    let load = load_favorites(&capabilities, &lookup, &session, inventory.as_ref()).fuse();
                    pin_mut!(load);
                    let cancelled = cancelled.fuse();
                    pin_mut!(cancelled);
                    futures::select! {
                        result = load => mailbox.publish(ViewerUpdate::Favorites(FavoritesUpdate {
                            grid: session.grid,
                            agent_id: session.agent_id,
                            session_id: session.session_id,
                            username,
                            result,
                        })),
                        _ = cancelled => {}
                    }
                }
                .in_current_span(),
            ),
        );
    }

    fn start_inventory_fetch(&mut self, capabilities: Client) {
        let Some(inventory) = &self.inventory else { return };
        let snapshot = inventory.snapshot();
        self.inventory_changes.send_replace(Some(snapshot.clone()));
        self.inventory_known = snapshot.categories().map(|category| category.key).collect();
        self.inventory_complete = self.inventory_known.iter().copied().filter(|key| snapshot.is_complete(*key)).collect();
        self.inventory_failed.clear();
        self.inventory_background_done = false;
        self.inventory_last_failure = None;
        self.publish_inventory_status();
        let model = self.inventory_changes.subscribe();
        self.inventory_fetch.start(&self.runtime, capabilities, snapshot, model, self.cancellation.clone());
    }

    fn start_inventory_edits(&mut self, capabilities: Client) {
        if self.inventory.is_none() {
            return;
        }
        let Some(session) = self.active_session.clone() else { return };
        let model = self.inventory_changes.subscribe();
        self.inventory_edits.start(&self.runtime, capabilities, self.session_handle.clone(), session, model, self.cancellation.clone());
    }

    pub(super) fn handle_inventory_edit_event(&mut self, event: InventoryEditEvent) {
        match event {
            InventoryEditEvent::Apply { confirmation, applied } => {
                let result = self.apply_inventory_confirmation(confirmation);
                if applied.send(result).is_err() {
                    tracing::trace!("inventory edit worker stopped before apply acknowledgement");
                }
            }
            InventoryEditEvent::RefreshFolders { folders, refreshed } => {
                let folder_count = folders.len();
                let refreshed_inventory = self.mark_inventory_folders_incomplete(folders);
                if !refreshed_inventory {
                    tracing::debug!(folder_count, "inventory folder refresh could not be queued after an edit failure");
                }
                if refreshed.send(refreshed_inventory).is_err() {
                    tracing::trace!("inventory edit worker stopped before folder refresh acknowledgement");
                }
            }
            InventoryEditEvent::MoveStarted { expectation, confirmed, registered } => {
                self.inventory_pending_moves.insert(expectation.node, (expectation, confirmed));
                if registered.send(()).is_err() {
                    tracing::trace!("inventory edit worker stopped before move registration acknowledgement");
                }
            }
            InventoryEditEvent::MoveAborted { expectation, removed } => {
                if self.inventory_pending_moves.get(&expectation.node).is_some_and(|(current, _)| *current == expectation) {
                    self.inventory_pending_moves.remove(&expectation.node);
                }
                if removed.send(()).is_err() {
                    tracing::trace!("inventory edit worker stopped before move abort acknowledgement");
                }
            }
            InventoryEditEvent::MoveTimedOut { expectation, refreshed } => {
                if self.inventory_pending_moves.get(&expectation.node).is_some_and(|(current, _)| *current == expectation) {
                    self.inventory_pending_moves.remove(&expectation.node);
                }
                let applied = self.mark_inventory_folders_incomplete([expectation.from, expectation.to]);
                if refreshed.send(applied).is_err() {
                    tracing::trace!("inventory edit worker stopped before move timeout acknowledgement");
                }
            }
        }
    }

    fn apply_inventory_confirmation(&mut self, confirmation: AisDelta) -> bool {
        let Some(current) = self.inventory.as_ref().map(Inventory::snapshot) else { return false };
        self.apply_inventory_edit_delta(confirmation.into_delta(&current))
    }

    fn apply_inventory_edit_delta(&mut self, delta: InventoryDelta) -> bool {
        let Some(before) = self.inventory.as_ref().map(Inventory::snapshot) else { return false };
        let Some(inventory) = self.inventory.as_mut() else { return false };
        let changes = match inventory.apply_delta(delta) {
            Ok(changes) => changes,
            Err(error) => {
                tracing::warn!(%error, "inventory edit confirmation could not be applied");
                return false;
            }
        };
        if changes.is_empty() {
            return true;
        }
        let Some(snapshot) = self.inventory.as_ref().map(Inventory::snapshot) else { return false };
        self.process_inventory_changes(&changes, &snapshot);
        self.inventory_changes.send_replace(Some(snapshot.clone()));
        self.request_inventory_refetches(&changes, &snapshot);
        self.publish_inventory_status();
        self.inventory_cache.record_change(&self.runtime, changes, before, snapshot);
        true
    }

    pub(super) fn process_inventory_changes(&mut self, changes: &ChangeSet, snapshot: &InventorySnapshot) {
        resolve_pending_inventory_moves(&mut self.inventory_pending_moves, changes);
        self.track_inventory_changes(changes, snapshot);
    }

    fn mark_inventory_folders_incomplete(&mut self, folders: impl IntoIterator<Item = CategoryKey>) -> bool {
        let Some(snapshot) = self.inventory.as_ref().map(Inventory::snapshot) else { return false };
        let folders = folders.into_iter().filter(|folder| snapshot.category(*folder).is_some()).collect::<HashSet<_>>();
        if folders.is_empty() {
            return false;
        }
        let delta = InventoryDelta {
            updated_category_versions: folders.into_iter().map(|folder| (folder, ServerVersion::UNKNOWN)).collect(),
            ..InventoryDelta::default()
        };
        self.apply_inventory_edit_delta(delta)
    }

    pub(super) fn handle_inventory_fetch(&mut self, event: InventoryFetchEvent) {
        match event {
            InventoryFetchEvent::Folder { fetch, applied } => {
                let folder = fetch.folder;
                let before = self.inventory.as_ref().map(Inventory::snapshot);
                let result = self.inventory.as_mut().map(|inventory| apply_inventory_fetch(inventory, fetch));
                let snapshot = self.inventory.as_ref().map(Inventory::snapshot);
                let mut rejected = false;
                match result {
                    Some(Ok(Some(changes))) => {
                        if let Some(snapshot) = snapshot.as_ref() {
                            self.process_inventory_changes(&changes, snapshot);
                            if snapshot.is_complete(folder) {
                                self.inventory_complete.insert(folder);
                            } else {
                                self.inventory_complete.remove(&folder);
                            }
                            self.inventory_failed.remove(&folder);
                            if self.inventory_last_failure.as_ref().is_some_and(|(failed, _)| *failed == folder) {
                                self.inventory_last_failure = None;
                            }
                            self.inventory_changes.send_replace(Some(snapshot.clone()));
                            if let Some(before) = before {
                                self.inventory_cache.record_change(&self.runtime, changes, before, snapshot.clone());
                            }
                        }
                    }
                    Some(Ok(None)) => {}
                    Some(Err(error)) => {
                        rejected = true;
                        self.record_inventory_failure(folder, InventoryFetchError::Model(error));
                    }
                    None => rejected = true,
                }
                let outcome = FolderApplyOutcome { rejected };
                if applied.send(outcome).is_err() {
                    tracing::debug!(folder = %folder.id().uuid(), "inventory fetch worker stopped before apply acknowledgement");
                }
                self.publish_inventory_status();
            }
            InventoryFetchEvent::Failed { folder, error } => {
                self.record_inventory_failure(folder, error);
                self.publish_inventory_status();
            }
            InventoryFetchEvent::BackgroundDone => {
                self.inventory_background_done = true;
                self.publish_inventory_status();
            }
        }
    }

    fn track_inventory_changes(&mut self, changes: &ChangeSet, snapshot: &InventorySnapshot) {
        for change in &changes.changes {
            match change {
                InventoryChange::CategoryUpserted(category) => {
                    self.inventory_known.insert(category.key);
                    if snapshot.is_complete(category.key) {
                        self.inventory_complete.insert(category.key);
                    } else {
                        self.inventory_complete.remove(&category.key);
                    }
                }
                InventoryChange::CategoryRemoved(key) => {
                    self.inventory_known.remove(key);
                    self.inventory_complete.remove(key);
                    self.inventory_failed.remove(key);
                }
                _ => {}
            }
        }
    }

    fn request_inventory_refetches(&mut self, changes: &ChangeSet, snapshot: &InventorySnapshot) {
        let handle = self.inventory_fetch.handle();
        let mut queue_failures = 0usize;
        for folder in &changes.refetch_categories {
            if snapshot.category(*folder).is_none() {
                continue;
            }
            self.inventory_complete.remove(folder);
            if handle.refetch_folder(*folder).is_err() {
                self.inventory_failed.insert(*folder);
                self.inventory_last_failure = Some((*folder, InventoryFetchError::RequestFailed));
                queue_failures = queue_failures.saturating_add(1);
            }
        }
        if queue_failures > 0 {
            tracing::warn!(queue_failures, "could not queue inventory folder refetches");
        }
    }

    fn record_inventory_failure(&mut self, folder: CategoryKey, error: InventoryFetchError) {
        self.inventory_failed.insert(folder);
        self.inventory_last_failure = Some((folder, error.clone()));
        tracing::warn!(owner = %folder.owner().id(), folder = %folder.id().uuid(), %error, "inventory folder fetch failed");
    }

    fn publish_inventory_status(&self) {
        self.inventory_fetch.publish(InventoryLoadingStatus {
            folders_known: self.inventory_known.len(),
            folders_complete: self.inventory_complete.len(),
            folders_failed: self.inventory_failed.len(),
            background_done: self.inventory_background_done,
            last_failure: self.inventory_last_failure.clone(),
        });
    }
}

pub(crate) fn resolve_pending_inventory_moves(
    pending: &mut HashMap<InventoryNodeKey, (MoveExpectation, tokio::sync::oneshot::Sender<()>)>,
    changes: &ChangeSet,
) {
    for change in &changes.changes {
        let InventoryChange::NodeMoved { node, from, to } = change else { continue };
        let Some((expectation, _)) = pending.get(node) else { continue };
        if expectation.from == *from
            && expectation.to == *to
            && let Some((_, confirmed)) = pending.remove(node)
            && confirmed.send(()).is_err()
        {
            tracing::trace!("inventory edit worker stopped before receiving move confirmation");
        }
    }
}

pub(super) fn cache_restore_move_changes(
    pending: &HashMap<InventoryNodeKey, (MoveExpectation, tokio::sync::oneshot::Sender<()>)>,
    before: &InventorySnapshot,
    after: &InventorySnapshot,
) -> ChangeSet {
    let changes = pending
        .iter()
        .filter_map(|(node, (expectation, _))| {
            (inventory_node_parent(before, *node) == Some(expectation.from) && inventory_node_parent(after, *node) == Some(expectation.to))
                .then_some(InventoryChange::NodeMoved { node: *node, from: expectation.from, to: expectation.to })
        })
        .collect();
    ChangeSet { revision: after.revision(), changes, refetch_categories: Vec::new() }
}

fn inventory_node_parent(snapshot: &InventorySnapshot, node: InventoryNodeKey) -> Option<CategoryKey> {
    match node.id() {
        InventoryNodeId::Category(id) => snapshot.category(CategoryKey::new(node.owner(), id)).and_then(|category| category.parent),
        InventoryNodeId::Item(id) => snapshot.item(::inventory::ItemKey::new(node.owner(), id)).map(|item| item.parent),
    }
}

pub(super) fn apply_inventory_fetch(inventory: &mut Inventory, mut fetch: FolderFetch) -> Result<Option<ChangeSet>, InventoryError> {
    let current = inventory.snapshot();
    let stale = current.is_complete(fetch.folder)
        && current.category(fetch.folder).is_some_and(|category| {
            category.server_version.is_known() && fetch.version.is_known() && category.server_version.raw() > fetch.version.raw()
        });
    if stale {
        return Ok(None);
    }
    for child in &mut fetch.children {
        let InventoryChild::Category(incoming) = child else { continue };
        let Some(existing) = current.category(incoming.key) else { continue };
        if current.is_complete(incoming.key)
            && existing.server_version.is_known()
            && incoming.server_version.is_known()
            && existing.server_version.raw() > incoming.server_version.raw()
        {
            incoming.server_version = existing.server_version;
            incoming.server_descendent_count = existing.server_descendent_count;
        }
    }
    inventory.apply_folder_fetch(fetch.folder, fetch.version, fetch.descendent_count, fetch.children).map(Some)
}
