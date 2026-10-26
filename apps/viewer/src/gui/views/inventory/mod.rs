use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use ::inventory::{AssetType, CategoryKey, FolderType, InventoryNodeId, InventoryNodeKey, InventorySnapshot, InventoryType, ItemKey};
use futures::{StreamExt, stream::FuturesUnordered};
use gpui::{Context, Entity, IntoElement, Render, Task, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list};
use gpui_component::{
    Disableable as _, IndexPath, Theme,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    select::{Select, SelectEvent, SelectItem, SelectState},
};
use tokio::sync::{mpsc, oneshot, watch};

use crate::inventory::{
    InventoryEditHandle, InventoryEditOutcome, InventoryEditRejection, InventoryFetchHandle, InventoryFetchOutcome, InventoryLoadingStatus,
    browser::{self, SearchResults, SearchTypeFilter, TreeRows},
};

const TREE_ROW_HEIGHT: f32 = 28.0;
const SEARCH_ROW_HEIGHT: f32 = 48.0;
const INDENT_WIDTH: f32 = 16.0;
const REQUEST_COMPLETION_CAPACITY: usize = 1024;

type RequestCompletion = (CategoryKey, u64, oneshot::Receiver<InventoryFetchOutcome>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TreeKind {
    Personal,
    Library,
}

#[derive(Clone)]
struct TypeOption {
    title: gpui::SharedString,
    filter: SearchTypeFilter,
}

impl SelectItem for TypeOption {
    type Value = SearchTypeFilter;

    fn title(&self) -> gpui::SharedString {
        self.title.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.filter
    }
}

#[derive(Clone)]
struct DestinationOption {
    title: gpui::SharedString,
    folder: CategoryKey,
    disabled: bool,
}

impl SelectItem for DestinationOption {
    type Value = CategoryKey;

    fn title(&self) -> gpui::SharedString {
        self.title.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.folder
    }

    fn disabled(&self) -> bool {
        self.disabled
    }
}

#[derive(Clone, Copy)]
enum NameEdit {
    CreateFolder(CategoryKey),
    RenameCategory(CategoryKey),
    RenameItem(ItemKey),
}

enum WindowEdit {
    CreateFolder(CategoryKey, String),
    RenameCategory(CategoryKey, String),
    RenameItem(ItemKey, String),
    MoveItem(ItemKey, CategoryKey),
    MoveFolder(CategoryKey, CategoryKey),
    DeleteItem(ItemKey),
    DeleteFolder(CategoryKey),
    EmptyTrash,
}

impl WindowEdit {
    async fn submit(self, edits: InventoryEditHandle) -> InventoryEditOutcome {
        match self {
            Self::CreateFolder(parent, name) => edits.create_folder(parent, name, FolderType::None).await,
            Self::RenameCategory(category, name) => edits.rename_category(category, name).await,
            Self::RenameItem(item, name) => edits.rename_item(item, name).await,
            Self::MoveItem(item, destination) => edits.move_item(item, destination).await,
            Self::MoveFolder(folder, destination) => edits.move_folder(folder, destination).await,
            Self::DeleteItem(item) => edits.delete_item(item).await,
            Self::DeleteFolder(folder) => edits.delete_folder(folder).await,
            Self::EmptyTrash => edits.empty_trash().await,
        }
    }
}

#[derive(Default)]
struct FolderRequestState {
    next_ticket: u64,
    pending: HashMap<CategoryKey, u64>,
    failed: HashSet<CategoryKey>,
}

impl FolderRequestState {
    fn begin(&mut self, folder: CategoryKey) -> u64 {
        self.next_ticket = self.next_ticket.wrapping_add(1);
        let ticket = self.next_ticket;
        self.pending.insert(folder, ticket);
        self.failed.remove(&folder);
        ticket
    }

    fn finish(&mut self, folder: CategoryKey, ticket: u64, outcome: InventoryFetchOutcome) -> bool {
        if self.pending.get(&folder) != Some(&ticket) {
            return false;
        }
        self.pending.remove(&folder);
        match outcome {
            InventoryFetchOutcome::Succeeded => {
                self.failed.remove(&folder);
            }
            InventoryFetchOutcome::Failed => {
                self.failed.insert(folder);
            }
        }
        true
    }

    fn retain_snapshot(&mut self, snapshot: &InventorySnapshot) {
        self.pending.retain(|folder, _| snapshot.category(*folder).is_some() && !snapshot.is_complete(*folder));
        self.failed.retain(|folder| snapshot.category(*folder).is_some() && !snapshot.is_complete(*folder));
    }

    fn is_pending(&self, folder: CategoryKey) -> bool {
        self.pending.contains_key(&folder)
    }

    fn is_failed(&self, folder: CategoryKey) -> bool {
        self.failed.contains(&folder)
    }

    fn mark_failed(&mut self, folder: CategoryKey) {
        self.failed.insert(folder);
    }
}

#[derive(Default)]
struct SearchRefresh {
    latest_revision: Option<u64>,
    running_revision: Option<u64>,
}

impl SearchRefresh {
    fn snapshot_changed(&mut self, revision: u64) -> Option<u64> {
        self.latest_revision = Some(revision);
        if self.running_revision.is_some() {
            return None;
        }
        self.running_revision = Some(revision);
        Some(revision)
    }

    fn finished(&mut self, revision: u64) -> Option<u64> {
        if self.running_revision != Some(revision) {
            return None;
        }
        self.running_revision = None;
        let latest = self.latest_revision.filter(|latest| *latest != revision)?;
        self.running_revision = Some(latest);
        Some(latest)
    }

    fn reset(&mut self) {
        self.latest_revision = None;
        self.running_revision = None;
    }
}

pub(crate) struct InventoryScreen {
    snapshots: Option<InventorySnapshot>,
    fetch: InventoryFetchHandle,
    edits: InventoryEditHandle,
    search_input: Entity<InputState>,
    edit_name_input: Entity<InputState>,
    type_filter: Entity<SelectState<Vec<TypeOption>>>,
    destination_select: Option<Entity<SelectState<Vec<DestinationOption>>>>,
    search_query: String,
    _search_subscription: gpui::Subscription,
    _destination_subscription: Option<gpui::Subscription>,
    _type_subscription: gpui::Subscription,
    snapshot_task: Option<Task<()>>,
    status_task: Option<Task<()>>,
    row_task: Option<Task<()>>,
    search_task: Option<Task<()>>,
    row_cancel: Option<Arc<AtomicBool>>,
    search_cancel: Option<Arc<AtomicBool>>,
    row_generation: u64,
    search_generation: u64,
    search_refresh: SearchRefresh,
    request_completions: mpsc::Sender<RequestCompletion>,
    rows: TreeRows,
    results: SearchResults,
    rows_building: bool,
    searching: bool,
    active_tree: TreeKind,
    expanded: HashSet<CategoryKey>,
    folder_requests: FolderRequestState,
    request_error: Option<String>,
    loading_status: InventoryLoadingStatus,
    tree_scroll: UniformListScrollHandle,
    search_scroll: UniformListScrollHandle,
    type_filter_value: SearchTypeFilter,
    selected_node: Option<InventoryNodeKey>,
    name_edit: Option<NameEdit>,
    edit_pending: bool,
    edit_task: Option<Task<()>>,
    edit_status: Option<(String, bool)>,
    confirm_empty_trash: bool,
}

impl InventoryScreen {
    pub(crate) fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        mut snapshots: watch::Receiver<Option<InventorySnapshot>>,
        fetch: InventoryFetchHandle,
    ) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search inventory"));
        let edit_name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Folder name"));
        let filter_options = type_options();
        let type_filter = cx.new(|cx| SelectState::new(filter_options, Some(IndexPath::default().row(0)), window, cx));
        let search_subscription = cx.subscribe(&search_input, |screen, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                screen.search_changed(cx);
            }
        });
        let type_subscription = cx.subscribe(&type_filter, |screen, _, event: &SelectEvent<Vec<TypeOption>>, cx| {
            if let SelectEvent::Confirm(Some(filter)) = event {
                screen.type_filter_value = *filter;
                screen.search_changed(cx);
            }
        });
        let edits = fetch.edits();
        let snapshot = snapshots.borrow_and_update().clone();
        let mut status_updates = fetch.status();
        let loading_status = status_updates.borrow_and_update().clone();
        let (request_completions, mut request_completion_events) = mpsc::channel(REQUEST_COMPLETION_CAPACITY);
        let mut screen = Self {
            snapshots: snapshot,
            fetch,
            edits,
            search_input,
            edit_name_input,
            type_filter,
            destination_select: None,
            _search_subscription: search_subscription,
            _destination_subscription: None,
            _type_subscription: type_subscription,
            search_query: String::new(),
            snapshot_task: None,
            status_task: None,
            row_task: None,
            search_task: None,
            row_cancel: None,
            search_cancel: None,
            row_generation: 0,
            search_generation: 0,
            search_refresh: SearchRefresh::default(),
            request_completions,
            rows: TreeRows::default(),
            results: SearchResults { items: Vec::new(), truncated: false },
            rows_building: false,
            searching: false,
            active_tree: TreeKind::Personal,
            expanded: HashSet::new(),
            folder_requests: FolderRequestState::default(),
            request_error: None,
            loading_status,
            tree_scroll: UniformListScrollHandle::default(),
            search_scroll: UniformListScrollHandle::default(),
            type_filter_value: SearchTypeFilter::All,
            selected_node: None,
            name_edit: None,
            edit_pending: false,
            edit_task: None,
            edit_status: None,
            confirm_empty_trash: false,
        };

        if let Some(root) = screen.active_root() {
            screen.request_folder(root);
            screen.rebuild_rows(cx);
        }

        screen.snapshot_task = Some(cx.spawn(async move |this, cx| {
            loop {
                if snapshots.changed().await.is_err() {
                    break;
                }
                let snapshot = snapshots.borrow_and_update().clone();
                let Some(snapshot) = snapshot else { continue };
                if this.update(cx, |screen, cx| screen.apply_snapshot(snapshot, cx)).is_err() {
                    break;
                }
            }
        }));

        screen.status_task = Some(cx.spawn(async move |this, cx| {
            let mut completions = FuturesUnordered::new();
            loop {
                tokio::select! {
                    changed = status_updates.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let status = status_updates.borrow_and_update().clone();
                        if this.update(cx, |screen, cx| screen.apply_status(status, cx)).is_err() {
                            break;
                        }
                    }
                    Some((folder, ticket, completion)) = request_completion_events.recv() => {
                        completions.push(async move {
                            (folder, ticket, completion.await.unwrap_or(InventoryFetchOutcome::Failed))
                        });
                    }
                    Some((folder, ticket, outcome)) = completions.next(), if !completions.is_empty() => {
                        if this.update(cx, |screen, cx| screen.apply_request_outcome(folder, ticket, outcome, cx)).is_err() {
                            break;
                        }
                    }
                    else => break,
                }
            }
        }));

        screen
    }

    fn active_root(&self) -> Option<CategoryKey> {
        self.snapshots.as_ref().and_then(|snapshot| match self.active_tree {
            TreeKind::Personal => Some(snapshot.roots().personal()),
            TreeKind::Library => snapshot.roots().library(),
        })
    }

    fn selected_category(&self) -> Option<CategoryKey> {
        let node = self.selected_node?;
        let InventoryNodeId::Category(id) = node.id() else { return None };
        Some(CategoryKey::new(node.owner(), id))
    }

    fn selected_item(&self) -> Option<ItemKey> {
        let node = self.selected_node?;
        let InventoryNodeId::Item(id) = node.id() else { return None };
        Some(ItemKey::new(node.owner(), id))
    }

    fn selected_is_personal(&self) -> bool {
        if self.active_tree != TreeKind::Personal {
            return false;
        }
        let Some(snapshot) = &self.snapshots else { return false };
        if let Some(node) = self.selected_node { node.owner() == snapshot.roots().personal().owner() } else { true }
    }

    fn new_folder_parent(&self) -> Option<CategoryKey> {
        let snapshot = self.snapshots.as_ref()?;
        if !self.selected_is_personal() {
            return None;
        }
        match self.selected_node.map(InventoryNodeKey::id) {
            Some(InventoryNodeId::Category(id)) => {
                let key = CategoryKey::new(self.selected_node?.owner(), id);
                let category = snapshot.category(key)?;
                (snapshot.roots().contains(key) || !category.preferred_folder_type.is_protected()).then_some(key)
            }
            Some(InventoryNodeId::Item(id)) => {
                let item = snapshot.item(ItemKey::new(self.selected_node?.owner(), id))?;
                let parent = snapshot.category(item.parent)?;
                (snapshot.roots().contains(parent.key) || !parent.preferred_folder_type.is_protected()).then_some(parent.key)
            }
            None => Some(snapshot.roots().personal()),
        }
    }

    fn selected_node_is_editable(&self) -> bool {
        let Some(snapshot) = &self.snapshots else { return false };
        if !self.selected_is_personal() {
            return false;
        }
        match self.selected_node.map(InventoryNodeKey::id) {
            Some(InventoryNodeId::Category(id)) => {
                let Some(node) = self.selected_node else { return false };
                let key = CategoryKey::new(node.owner(), id);
                snapshot
                    .category(key)
                    .is_some_and(|category| !snapshot.roots().contains(key) && !category.preferred_folder_type.is_protected())
            }
            Some(InventoryNodeId::Item(id)) => {
                let Some(node) = self.selected_node else { return false };
                let key = ItemKey::new(node.owner(), id);
                snapshot
                    .item(key)
                    .and_then(|item| snapshot.category(item.parent))
                    .is_some_and(|parent| snapshot.roots().contains(parent.key) || !parent.preferred_folder_type.is_protected())
            }
            None => false,
        }
    }

    fn selected_is_trash(&self) -> bool {
        self.selected_category()
            .and_then(|key| self.snapshots.as_ref()?.category(key))
            .is_some_and(|category| category.preferred_folder_type == FolderType::Trash)
            && self.selected_is_personal()
    }

    fn select_node(&mut self, node: InventoryNodeKey, cx: &mut Context<Self>) {
        self.selected_node = Some(node);
        self.name_edit = None;
        self.destination_select = None;
        self._destination_subscription = None;
        self.confirm_empty_trash = false;
        cx.notify();
    }

    fn begin_name_edit(&mut self, mode: NameEdit, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_name_input.update(cx, |input, cx| input.set_value(value.to_owned(), window, cx));
        self.name_edit = Some(mode);
        self.destination_select = None;
        self._destination_subscription = None;
        self.confirm_empty_trash = false;
        cx.notify();
    }

    fn open_move_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.selected_node.filter(|_| self.selected_node_is_editable()) else { return };
        let Some(snapshot) = &self.snapshots else { return };
        let owner = snapshot.roots().personal().owner();
        let root = snapshot.roots().personal();
        let mut destinations = vec![DestinationOption { title: "My Inventory".into(), folder: root, disabled: false }];
        for row in &self.rows.rows {
            let InventoryNodeId::Category(id) = row.node.id() else { continue };
            if row.node.owner() != owner {
                continue;
            }
            let key = CategoryKey::new(owner, id);
            let Some(category) = snapshot.category(key) else { continue };
            let label = format!("{}{}", "  ".repeat(row.depth.min(12)), category.name);
            destinations.push(DestinationOption {
                title: label.into(),
                folder: key,
                disabled: category.preferred_folder_type.is_protected()
                    || matches!(source.id(), InventoryNodeId::Category(source_id) if source_id == id)
                    || matches!(source.id(), InventoryNodeId::Category(source_id) if folder_contains(snapshot, CategoryKey::new(owner, source_id), key)),
            });
        }
        let select = cx.new(|cx| SelectState::new(destinations, None, window, cx));
        let subscription = cx.subscribe(&select, |screen, _, event: &SelectEvent<Vec<DestinationOption>>, cx| {
            if let SelectEvent::Confirm(Some(destination)) = event {
                screen.submit_move(*destination, cx);
            }
        });
        self.name_edit = None;
        self._destination_subscription = Some(subscription);
        self.destination_select = Some(select);
        self.confirm_empty_trash = false;
        cx.notify();
    }

    fn submit_move(&mut self, destination: CategoryKey, cx: &mut Context<Self>) {
        let Some(node) = self.selected_node.filter(|_| self.selected_node_is_editable()) else { return };
        self.destination_select = None;
        self._destination_subscription = None;
        match node.id() {
            InventoryNodeId::Item(id) => self.start_edit(WindowEdit::MoveItem(ItemKey::new(node.owner(), id), destination), cx),
            InventoryNodeId::Category(id) => {
                self.start_edit(WindowEdit::MoveFolder(CategoryKey::new(node.owner(), id), destination), cx);
            }
        }
    }

    fn save_name_edit(&mut self, cx: &mut Context<Self>) {
        let Some(mode) = self.name_edit.take() else { return };
        let value = self.edit_name_input.read(cx).value().to_string();
        let edit = match mode {
            NameEdit::CreateFolder(parent) => WindowEdit::CreateFolder(parent, value),
            NameEdit::RenameCategory(category) => WindowEdit::RenameCategory(category, value),
            NameEdit::RenameItem(item) => WindowEdit::RenameItem(item, value),
        };
        self.start_edit(edit, cx);
    }

    fn start_edit(&mut self, edit: WindowEdit, cx: &mut Context<Self>) {
        if self.edit_pending {
            return;
        }
        self.edit_pending = true;
        self.edit_status = Some(("Updating inventory…".into(), false));
        self.confirm_empty_trash = false;
        let handle = self.edits.clone();
        self.edit_task = Some(cx.spawn(async move |this, cx| {
            let outcome = edit.submit(handle).await;
            if this.update(cx, |screen, cx| screen.finish_edit(outcome, cx)).is_err() {
                tracing::trace!("inventory edit finished after its window closed");
            }
        }));
        cx.notify();
    }

    fn finish_edit(&mut self, outcome: InventoryEditOutcome, cx: &mut Context<Self>) {
        self.edit_pending = false;
        self.name_edit = None;
        self.destination_select = None;
        self._destination_subscription = None;
        let (message, failed) = edit_status(outcome);
        self.edit_status = Some((message.into(), failed));
        if let InventoryEditOutcome::Created(category) = outcome
            && self.active_tree == TreeKind::Personal
        {
            self.selected_node = Some(InventoryNodeKey::new(category.owner(), InventoryNodeId::Category(category.id())));
            if let Some(parent) =
                self.snapshots.as_ref().and_then(|snapshot| snapshot.category(category)).and_then(|category| category.parent)
            {
                self.expanded.insert(parent);
                self.rebuild_rows(cx);
            }
        }
        cx.notify();
    }

    fn search_active(&self) -> bool {
        self.type_filter_value != SearchTypeFilter::All || !self.search_query.is_empty()
    }

    fn search_changed(&mut self, cx: &mut Context<Self>) {
        self.search_query = self.search_input.read(cx).value().to_string();
        self.selected_node = None;
        self.name_edit = None;
        self.destination_select = None;
        self._destination_subscription = None;
        self.confirm_empty_trash = false;
        if let Some(cancel) = self.search_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.search_task.take();
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_refresh.reset();
        self.results = SearchResults { items: Vec::new(), truncated: false };
        self.searching = self.search_active();

        if !self.searching {
            self.rebuild_rows(cx);
            cx.notify();
            return;
        }
        let Some(revision) = self.snapshots.as_ref().map(|snapshot| snapshot.revision().get()) else {
            cx.notify();
            return;
        };
        if let Some(revision) = self.search_refresh.snapshot_changed(revision) {
            self.start_search(revision, cx);
        }
    }

    fn start_search(&mut self, revision: u64, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshots.clone() else { return };
        let query = self.search_query.clone();
        let filter = self.type_filter_value;
        let generation = self.search_generation;
        let background = cx.background_executor().clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.search_cancel = Some(cancel);
        self.searching = true;
        self.search_task = Some(cx.spawn(async move |this, cx| {
            let results =
                background.spawn(async move { browser::search(&snapshot, &query, filter, || worker_cancel.load(Ordering::Acquire)) }).await;
            if this
                .update(cx, |screen, cx| {
                    if screen.search_generation == generation {
                        if let Some(results) = results {
                            screen.results = results;
                        }
                        screen.search_cancel = None;
                        if let Some(next_revision) = screen.search_refresh.finished(revision) {
                            screen.start_search(next_revision, cx);
                        } else {
                            screen.searching = false;
                            cx.notify();
                        }
                    }
                })
                .is_err()
            {
                tracing::trace!("inventory search finished after its window closed");
            }
        }));
        cx.notify();
    }

    fn apply_snapshot(&mut self, snapshot: InventorySnapshot, cx: &mut Context<Self>) {
        if self.snapshots.as_ref().is_some_and(|current| current.revision() == snapshot.revision()) {
            return;
        }
        let revision = snapshot.revision().get();
        self.snapshots = Some(snapshot);
        if let Some(snapshot) = &self.snapshots {
            self.expanded.retain(|key| snapshot.category(*key).is_some());
            self.folder_requests.retain_snapshot(snapshot);
            if self.selected_node.is_some_and(|node| !snapshot_contains_node(snapshot, node)) {
                self.selected_node = None;
            }
        }
        if self.search_active() {
            if let Some(revision) = self.search_refresh.snapshot_changed(revision) {
                self.start_search(revision, cx);
            }
        } else {
            self.rebuild_rows(cx);
        }
        cx.notify();
    }

    fn apply_status(&mut self, status: InventoryLoadingStatus, cx: &mut Context<Self>) {
        if let Some((folder, _)) = &status.last_failure {
            self.folder_requests.mark_failed(*folder);
        }
        self.loading_status = status;
        cx.notify();
    }

    fn apply_request_outcome(&mut self, folder: CategoryKey, ticket: u64, outcome: InventoryFetchOutcome, cx: &mut Context<Self>) {
        if self.folder_requests.finish(folder, ticket, outcome) {
            cx.notify();
        }
    }

    fn set_tree(&mut self, tree: TreeKind, cx: &mut Context<Self>) {
        self.active_tree = tree;
        self.selected_node = None;
        self.name_edit = None;
        self.destination_select = None;
        self._destination_subscription = None;
        self.confirm_empty_trash = false;
        if let Some(root) = self.active_root() {
            self.request_folder(root);
        }
        if !self.search_active() {
            self.rebuild_rows(cx);
        }
        cx.notify();
    }

    fn toggle_folder(&mut self, folder: CategoryKey, cx: &mut Context<Self>) {
        if self.expanded.remove(&folder) {
            self.rebuild_rows(cx);
            cx.notify();
            return;
        }
        self.expanded.insert(folder);
        self.request_folder(folder);
        self.rebuild_rows(cx);
        cx.notify();
    }

    fn request_folder(&mut self, folder: CategoryKey) {
        if self.snapshots.as_ref().is_none_or(|snapshot| snapshot.is_complete(folder)) || self.folder_requests.is_pending(folder) {
            return;
        }
        match self.fetch.request_folder(folder) {
            Ok(completion) => {
                let ticket = self.folder_requests.begin(folder);
                self.request_error = None;
                if self.request_completions.try_send((folder, ticket, completion)).is_err() {
                    self.folder_requests.finish(folder, ticket, InventoryFetchOutcome::Failed);
                    self.request_error = Some("Could not track folder fetch completion".into());
                }
            }
            Err(error) => {
                self.request_error = Some(error.to_string());
            }
        }
    }

    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = self.row_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.row_task.take();
        self.row_generation = self.row_generation.wrapping_add(1);
        let generation = self.row_generation;
        let Some(snapshot) = self.snapshots.clone() else {
            self.rows = TreeRows::default();
            self.rows_building = false;
            return;
        };
        let Some(root) = self.active_root() else {
            self.rows = TreeRows::default();
            self.rows_building = false;
            return;
        };
        let expanded = self.expanded.clone();
        let background = cx.background_executor().clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.row_cancel = Some(cancel);
        self.rows_building = true;
        self.row_task = Some(cx.spawn(async move |this, cx| {
            let rows = background
                .spawn(async move {
                    browser::build_visible_rows_cancellable(&snapshot, root, &expanded, || worker_cancel.load(Ordering::Acquire))
                })
                .await;
            if this
                .update(cx, |screen, cx| {
                    if screen.row_generation == generation
                        && let Some(rows) = rows
                    {
                        screen.rows = rows;
                        screen.rows_building = false;
                        screen.row_cancel = None;
                        cx.notify();
                    }
                })
                .is_err()
            {
                tracing::trace!("inventory tree update finished after its window closed");
            }
        }));
    }

    fn render_tree_row(&self, index: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(snapshot) = &self.snapshots else { return div().into_any_element() };
        let Some(row) = self.rows.rows.get(index) else { return div().into_any_element() };
        let InventoryNodeId::Category(id) = row.node.id() else {
            let InventoryNodeId::Item(id) = row.node.id() else { return div().into_any_element() };
            let Some(item) = snapshot.item(::inventory::ItemKey::new(row.node.owner(), id)) else { return div().into_any_element() };
            let node = row.node;
            let theme = Theme::global(cx).colors;
            return div()
                .id(format!("inventory-item-{}", id.uuid()))
                .h(px(TREE_ROW_HEIGHT))
                .flex()
                .items_center()
                .gap_2()
                .pl(px((row.depth as f32) * INDENT_WIDTH + INDENT_WIDTH))
                .when(self.selected_node == Some(row.node), |view| view.bg(theme.group_box))
                .child(div().w(px(38.)).text_size(px(11.)).text_color(theme.muted_foreground).child(asset_type_label(item.asset_type)))
                .child(div().min_w_0().flex_1().text_color(theme.foreground).child(clip_text(&item.name, 80)))
                .cursor_pointer()
                .on_click(cx.listener(move |screen, _, _, cx| screen.select_node(node, cx)))
                .into_any_element();
        };

        let key = CategoryKey::new(row.node.owner(), id);
        let Some(category) = snapshot.category(key) else { return div().into_any_element() };
        let expanded = self.expanded.contains(&key);
        let complete = snapshot.is_complete(key);
        let child_count = snapshot.child_count(key);
        let has_disclosure = child_count > 0 || !complete;
        let node = row.node;
        let disclosure = if has_disclosure { if expanded { "▾" } else { "▸" } } else { "" };
        let state = if self.folder_requests.is_pending(key) {
            "Loading…"
        } else if self.folder_requests.is_failed(key) {
            "Fetch failed"
        } else if !complete {
            "Incomplete"
        } else {
            ""
        };
        let theme = Theme::global(cx).colors;
        let row_view = div()
            .h(px(TREE_ROW_HEIGHT))
            .flex()
            .items_center()
            .gap_2()
            .pl(px((row.depth as f32) * INDENT_WIDTH))
            .when(self.selected_node == Some(row.node), |view| view.bg(theme.group_box))
            .child(div().w(px(20.)).text_color(theme.muted_foreground).child(disclosure))
            .child(div().w(px(38.)).text_size(px(11.)).text_color(theme.muted_foreground).child("DIR"))
            .child(div().min_w_0().flex_1().text_color(theme.foreground).child(clip_text(&category.name, 80)))
            .child(div().flex_none().text_size(px(11.)).text_color(theme.muted_foreground).child(state));
        row_view
            .id(format!("inventory-folder-{}", id.uuid()))
            .cursor_pointer()
            .on_click(cx.listener(move |screen, _, _, cx| {
                screen.select_node(node, cx);
                if has_disclosure {
                    screen.toggle_folder(key, cx);
                }
            }))
            .into_any_element()
    }

    fn selected_can_delete(&self) -> bool {
        if !self.selected_node_is_editable() {
            return false;
        }
        if let Some(item) = self.selected_item() {
            return self.snapshots.as_ref().and_then(|snapshot| snapshot.item(item)).is_some_and(|item| {
                self.snapshots
                    .as_ref()
                    .and_then(|snapshot| snapshot.category_with_type(item.key.owner(), FolderType::Trash))
                    .is_none_or(|trash| item.parent != trash.key)
            });
        }
        self.selected_category().is_some_and(|key| {
            self.snapshots.as_ref().is_some_and(|snapshot| {
                snapshot.category(key).is_some_and(|category| {
                    snapshot.category_with_type(key.owner(), FolderType::Trash).is_none_or(|trash| category.parent != Some(trash.key))
                })
            })
        })
    }

    fn begin_selected_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(category) = self.selected_category()
            && let Some(name) =
                self.snapshots.as_ref().and_then(|snapshot| snapshot.category(category)).map(|category| category.name.to_string())
        {
            self.begin_name_edit(NameEdit::RenameCategory(category), &name, window, cx);
        } else if let Some(item) = self.selected_item()
            && let Some(name) = self.snapshots.as_ref().and_then(|snapshot| snapshot.item(item)).map(|item| item.name.to_string())
        {
            self.begin_name_edit(NameEdit::RenameItem(item), &name, window, cx);
        }
    }

    fn delete_selected(&mut self, cx: &mut Context<Self>) {
        if !self.selected_can_delete() {
            return;
        }
        if let Some(item) = self.selected_item() {
            self.start_edit(WindowEdit::DeleteItem(item), cx);
        } else if let Some(category) = self.selected_category() {
            self.start_edit(WindowEdit::DeleteFolder(category), cx);
        }
    }

    fn render_edit_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let editing_available = !self.edit_pending && !self.search_active();
        let create_enabled = editing_available && self.new_folder_parent().is_some();
        let selected_enabled = editing_available && self.selected_node_is_editable();
        let delete_enabled = editing_available && self.selected_can_delete();
        let empty_trash_enabled = !self.edit_pending && self.selected_is_trash();
        let mut actions = div()
            .flex()
            .items_center()
            .gap_2()
            .child(Button::new("inventory-new-folder").label("New Folder").h(px(30.)).disabled(!create_enabled).on_click(cx.listener(
                |screen, _, window, cx| {
                    if let Some(parent) = screen.new_folder_parent() {
                        screen.begin_name_edit(NameEdit::CreateFolder(parent), "", window, cx);
                    }
                },
            )))
            .child(
                Button::new("inventory-rename")
                    .label("Rename")
                    .h(px(30.))
                    .disabled(!selected_enabled)
                    .on_click(cx.listener(|screen, _, window, cx| screen.begin_selected_rename(window, cx))),
            )
            .child(
                Button::new("inventory-delete")
                    .label("Move to Trash")
                    .h(px(30.))
                    .disabled(!delete_enabled)
                    .on_click(cx.listener(|screen, _, _, cx| screen.delete_selected(cx))),
            )
            .child(
                Button::new("inventory-move")
                    .label("Move to…")
                    .h(px(30.))
                    .disabled(!selected_enabled)
                    .on_click(cx.listener(|screen, _, window, cx| screen.open_move_picker(window, cx))),
            )
            .child(
                Button::new("inventory-empty-trash")
                    .label("Empty Trash")
                    .h(px(30.))
                    .with_variant(if self.confirm_empty_trash {
                        gpui_component::button::ButtonVariant::Danger
                    } else {
                        gpui_component::button::ButtonVariant::Secondary
                    })
                    .disabled(!empty_trash_enabled)
                    .on_click(cx.listener(|screen, _, _, cx| {
                        screen.confirm_empty_trash = !screen.confirm_empty_trash;
                        cx.notify();
                    })),
            );

        if self.name_edit.is_some() {
            actions = actions
                .child(div().w(px(220.)).child(Input::new(&self.edit_name_input).h(px(30.))))
                .child(
                    Button::new("inventory-edit-save")
                        .label("Save")
                        .h(px(30.))
                        .disabled(self.edit_pending)
                        .on_click(cx.listener(|screen, _, _, cx| screen.save_name_edit(cx))),
                )
                .child(Button::new("inventory-edit-cancel").label("Cancel").h(px(30.)).disabled(self.edit_pending).on_click(cx.listener(
                    |screen, _, _, cx| {
                        screen.name_edit = None;
                        cx.notify();
                    },
                )));
        }
        if let Some(destination) = &self.destination_select {
            actions = actions.child(div().w(px(240.)).child(Select::new(destination).h(px(30.)).placeholder("Choose destination"))).child(
                Button::new("inventory-move-cancel").label("Cancel").h(px(30.)).on_click(cx.listener(|screen, _, _, cx| {
                    screen.destination_select = None;
                    screen._destination_subscription = None;
                    cx.notify();
                })),
            );
        }
        if self.confirm_empty_trash {
            actions = actions
                .child(
                    div().text_size(px(12.)).text_color(Theme::global(cx).colors.danger).child("Permanently delete all contents of Trash?"),
                )
                .child(
                    Button::new("inventory-empty-trash-confirm")
                        .label("Delete all")
                        .h(px(30.))
                        .with_variant(gpui_component::button::ButtonVariant::Danger)
                        .disabled(!empty_trash_enabled)
                        .on_click(cx.listener(|screen, _, _, cx| {
                            if screen.selected_is_trash() {
                                screen.start_edit(WindowEdit::EmptyTrash, cx);
                            }
                        })),
                )
                .child(Button::new("inventory-empty-trash-cancel").label("Cancel").h(px(30.)).on_click(cx.listener(|screen, _, _, cx| {
                    screen.confirm_empty_trash = false;
                    cx.notify();
                })));
        }
        actions
    }

    fn render_search_result(&self, index: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(result) = self.results.items.get(index) else { return div().into_any_element() };
        let theme = Theme::global(cx).colors;
        let source = if result.library { "Library" } else { "Inventory" };
        div()
            .h(px(SEARCH_ROW_HEIGHT))
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .px_2()
            .child(div().w(px(48.)).text_size(px(11.)).text_color(theme.muted_foreground).child(asset_type_label(result.asset_type)))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .child(div().text_color(theme.foreground).child(clip_text(&result.name, 100)))
                    .child(div().text_size(px(11.)).text_color(theme.muted_foreground).child(clip_text(&result.folder_path, 120))),
            )
            .child(div().flex_none().text_size(px(11.)).text_color(theme.muted_foreground).child(source))
            .into_any_element()
    }

    fn render_status(&self) -> String {
        if self.search_active() {
            if self.searching {
                return "Searching inventory…".into();
            }
            let truncated = if self.results.truncated { " · results truncated" } else { "" };
            return format!("{} results{truncated}", self.results.items.len());
        }
        if self.rows_building {
            return "Building folder list…".into();
        }
        if let Some(root) = self.active_root() {
            if self.folder_requests.is_pending(root) {
                return "Loading inventory folders…".into();
            }
            if self.folder_requests.is_failed(root) {
                return "The inventory root could not be loaded".into();
            }
            if self.snapshots.as_ref().is_some_and(|snapshot| !snapshot.is_complete(root)) {
                return "Inventory contents are incomplete".into();
            }
        }
        let tree_truncated = if self.rows.truncated { " · use search to find remaining rows" } else { "" };
        format!(
            "{} of {} folders complete · {} failed{tree_truncated}",
            self.loading_status.folders_complete, self.loading_status.folders_known, self.loading_status.folders_failed
        )
    }
}

impl Render for InventoryScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx).colors;
        let search_active = self.search_active();
        let tab_row = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                Button::new("inventory-tab-personal")
                    .label("My Inventory")
                    .with_variant(if self.active_tree == TreeKind::Personal {
                        gpui_component::button::ButtonVariant::Primary
                    } else {
                        gpui_component::button::ButtonVariant::Secondary
                    })
                    .h(px(30.))
                    .on_click(cx.listener(|screen, _, _, cx| screen.set_tree(TreeKind::Personal, cx))),
            )
            .child(
                Button::new("inventory-tab-library")
                    .label("Library · read-only")
                    .with_variant(if self.active_tree == TreeKind::Library {
                        gpui_component::button::ButtonVariant::Primary
                    } else {
                        gpui_component::button::ButtonVariant::Secondary
                    })
                    .h(px(30.))
                    .on_click(cx.listener(|screen, _, _, cx| screen.set_tree(TreeKind::Library, cx))),
            );

        let content = if search_active {
            if self.results.items.is_empty() && !self.searching {
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.muted_foreground)
                    .child("No matching items")
            } else {
                div().flex_1().min_h_0().child(
                    uniform_list(
                        "inventory-search-results",
                        self.results.items.len(),
                        cx.processor(|screen, range: Range<usize>, _window, cx| {
                            range.map(|index| screen.render_search_result(index, cx)).collect::<Vec<_>>()
                        }),
                    )
                    .size_full()
                    .track_scroll(&self.search_scroll),
                )
            }
        } else if self.active_root().is_none() {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .child("This grid did not provide a Library.")
        } else if self.rows.rows.is_empty() {
            div().flex_1().min_h_0().flex().items_center().justify_center().text_color(theme.muted_foreground).child("This folder is empty")
        } else {
            div().flex_1().min_h_0().child(
                uniform_list(
                    if self.active_tree == TreeKind::Personal { "personal-inventory-tree" } else { "library-inventory-tree" },
                    self.rows.rows.len(),
                    cx.processor(|screen, range: Range<usize>, _window, cx| {
                        range.map(|index| screen.render_tree_row(index, cx)).collect::<Vec<_>>()
                    }),
                )
                .size_full()
                .track_scroll(&self.tree_scroll),
            )
        };

        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_2()
            .bg(theme.background)
            .text_color(theme.foreground)
            .p_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().min_w_0().flex_1().child(Input::new(&self.search_input).h(px(36.)).focus_bordered(false)))
                    .child(div().w(px(230.)).child(Select::new(&self.type_filter).h(px(36.)))),
            )
            .child(tab_row)
            .child(self.render_edit_actions(cx))
            .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child(self.render_status()))
            .when(self.request_error.is_some(), |view| {
                view.child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.danger)
                        .child(format!("Could not request folder: {}", self.request_error.as_deref().unwrap_or_default())),
                )
            })
            .when_some(self.edit_status.as_ref(), |view, (message, failed)| {
                view.child(
                    div().text_size(px(12.)).text_color(if *failed { theme.danger } else { theme.muted_foreground }).child(message.clone()),
                )
            })
            .child(content)
    }
}

fn snapshot_contains_node(snapshot: &InventorySnapshot, node: InventoryNodeKey) -> bool {
    match node.id() {
        InventoryNodeId::Category(id) => snapshot.category(CategoryKey::new(node.owner(), id)).is_some(),
        InventoryNodeId::Item(id) => snapshot.item(ItemKey::new(node.owner(), id)).is_some(),
    }
}

fn folder_contains(snapshot: &InventorySnapshot, ancestor: CategoryKey, descendant: CategoryKey) -> bool {
    let mut current = Some(descendant);
    let mut visited = HashSet::new();
    while let Some(folder) = current {
        if folder == ancestor {
            return true;
        }
        if !visited.insert(folder) {
            return true;
        }
        current = snapshot.category(folder).and_then(|category| category.parent);
    }
    false
}

fn edit_status(outcome: InventoryEditOutcome) -> (&'static str, bool) {
    match outcome {
        InventoryEditOutcome::Applied => ("Inventory updated", false),
        InventoryEditOutcome::Created(_) => ("Folder created", false),
        InventoryEditOutcome::Rejected(rejection) => (
            match rejection {
                InventoryEditRejection::QueueFull => "Too many inventory edits are queued; try again",
                InventoryEditRejection::SessionEnded => "The inventory session has ended",
                InventoryEditRejection::MissingCategory => "That folder no longer exists",
                InventoryEditRejection::MissingItem => "That item no longer exists",
                InventoryEditRejection::MissingTrash => "The Trash folder is unavailable",
                InventoryEditRejection::DifferentOwner => "That entry belongs to another inventory",
                InventoryEditRejection::RootFolder => "Inventory root folders cannot be changed",
                InventoryEditRejection::ProtectedFolder => "System folders cannot be changed",
                InventoryEditRejection::InvalidName => "Enter a valid folder or item name",
                InventoryEditRejection::FolderCycle => "A folder cannot be moved into itself",
            },
            true,
        ),
        InventoryEditOutcome::ServerError => ("The inventory server could not complete that change", true),
        InventoryEditOutcome::Gone => ("The entry is gone; its parent folder is being refreshed", true),
        InventoryEditOutcome::TimedOut => ("The update timed out; affected folders are being refreshed", true),
    }
}

fn type_options() -> Vec<TypeOption> {
    let mut options = vec![TypeOption { title: "All types".into(), filter: SearchTypeFilter::All }];
    let asset_types = [
        AssetType::Texture,
        AssetType::Sound,
        AssetType::CallingCard,
        AssetType::Landmark,
        AssetType::Script,
        AssetType::Clothing,
        AssetType::Object,
        AssetType::Notecard,
        AssetType::Category,
        AssetType::LslText,
        AssetType::LslBytecode,
        AssetType::TextureTga,
        AssetType::Bodypart,
        AssetType::SoundWav,
        AssetType::ImageTga,
        AssetType::ImageJpeg,
        AssetType::Animation,
        AssetType::Gesture,
        AssetType::Simstate,
        AssetType::Link,
        AssetType::LinkFolder,
        AssetType::MarketplaceFolder,
        AssetType::Widget,
        AssetType::Person,
        AssetType::Mesh,
        AssetType::Settings,
        AssetType::Material,
        AssetType::Gltf,
        AssetType::GltfBinary,
        AssetType::None,
    ];
    options.extend(
        asset_types
            .into_iter()
            .map(|filter| TypeOption { title: format!("Asset · {:?}", filter).into(), filter: SearchTypeFilter::Asset(filter) }),
    );
    options.extend((0..=5).map(|index| {
        let filter = AssetType::Reserved(index);
        TypeOption { title: format!("Asset · Reserved {index}").into(), filter: SearchTypeFilter::Asset(filter) }
    }));
    let inventory_types = [
        InventoryType::Texture,
        InventoryType::Sound,
        InventoryType::CallingCard,
        InventoryType::Landmark,
        InventoryType::Object,
        InventoryType::Notecard,
        InventoryType::Category,
        InventoryType::RootCategory,
        InventoryType::Lsl,
        InventoryType::Snapshot,
        InventoryType::Attachment,
        InventoryType::Wearable,
        InventoryType::Animation,
        InventoryType::Gesture,
        InventoryType::Mesh,
        InventoryType::Widget,
        InventoryType::Person,
        InventoryType::Settings,
        InventoryType::Material,
        InventoryType::Gltf,
        InventoryType::GltfBinary,
        InventoryType::None,
    ];
    options.extend(
        inventory_types
            .into_iter()
            .map(|filter| TypeOption { title: format!("Inventory · {:?}", filter).into(), filter: SearchTypeFilter::Inventory(filter) }),
    );
    options
}

fn asset_type_label(asset_type: AssetType) -> String {
    match asset_type {
        AssetType::Texture => "TEX".into(),
        AssetType::Sound | AssetType::SoundWav => "SND".into(),
        AssetType::CallingCard => "CARD".into(),
        AssetType::Landmark => "LMK".into(),
        AssetType::Script | AssetType::LslText | AssetType::LslBytecode => "SCRIPT".into(),
        AssetType::Clothing => "CLOTH".into(),
        AssetType::Object => "OBJ".into(),
        AssetType::Notecard => "NOTE".into(),
        AssetType::Category | AssetType::LinkFolder | AssetType::MarketplaceFolder => "DIR".into(),
        AssetType::TextureTga | AssetType::ImageTga => "TGA".into(),
        AssetType::Bodypart => "BODY".into(),
        AssetType::ImageJpeg => "JPEG".into(),
        AssetType::Animation => "ANIM".into(),
        AssetType::Gesture => "GEST".into(),
        AssetType::Simstate => "SIM".into(),
        AssetType::Link => "LINK".into(),
        AssetType::Widget => "WIDGET".into(),
        AssetType::Person => "PERSON".into(),
        AssetType::Mesh => "MESH".into(),
        AssetType::Reserved(index) => format!("R{index}"),
        AssetType::Settings => "SET".into(),
        AssetType::Material => "MAT".into(),
        AssetType::Gltf => "GLTF".into(),
        AssetType::GltfBinary => "GLB".into(),
        AssetType::None => "NONE".into(),
        AssetType::Unknown(value) => format!("?{value}"),
    }
}

fn clip_text(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let prefix = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() { format!("{prefix}…") } else { prefix }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::inventory::InventoryOwner;
    use utils::FolderId;
    use uuid::Uuid;

    #[test]
    fn repeated_fetch_failures_clear_their_request_and_allow_successful_retry() {
        let folder = CategoryKey::new(InventoryOwner::new(Uuid::from_u128(1)), FolderId::new(Uuid::from_u128(2)));
        let mut requests = FolderRequestState::default();

        let first = requests.begin(folder);
        assert!(requests.finish(folder, first, InventoryFetchOutcome::Failed));
        assert!(!requests.is_pending(folder));
        assert!(requests.is_failed(folder));

        let second = requests.begin(folder);
        assert_ne!(first, second);
        assert!(requests.finish(folder, second, InventoryFetchOutcome::Failed));
        assert!(!requests.is_pending(folder));
        assert!(requests.is_failed(folder));

        let third = requests.begin(folder);
        assert_ne!(second, third);
        assert!(requests.finish(folder, third, InventoryFetchOutcome::Succeeded));
        assert!(!requests.is_pending(folder));
        assert!(!requests.is_failed(folder));
    }

    #[test]
    fn inventory_revisions_coalesce_while_search_runs() {
        let mut refresh = SearchRefresh::default();

        assert_eq!(refresh.snapshot_changed(10), Some(10));
        assert_eq!(refresh.snapshot_changed(11), None);
        assert_eq!(refresh.snapshot_changed(12), None);
        assert_eq!(refresh.snapshot_changed(13), None);
        assert_eq!(refresh.running_revision, Some(10));

        assert_eq!(refresh.finished(10), Some(13));
        assert_eq!(refresh.running_revision, Some(13));
        assert_eq!(refresh.finished(13), None);
        assert_eq!(refresh.running_revision, None);
    }
}
