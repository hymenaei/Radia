use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use ::inventory::{AssetType, CategoryKey, InventoryNodeId, InventorySnapshot, InventoryType};
use futures::{StreamExt, stream::FuturesUnordered};
use gpui::{Context, Entity, IntoElement, Render, Task, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list};
use gpui_component::{
    IndexPath, Theme,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    select::{Select, SelectEvent, SelectItem, SelectState},
};
use tokio::sync::{mpsc, oneshot, watch};

use crate::inventory::{
    InventoryFetchHandle, InventoryFetchOutcome, InventoryLoadingStatus,
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
    search_input: Entity<InputState>,
    type_filter: Entity<SelectState<Vec<TypeOption>>>,
    search_query: String,
    _search_subscription: gpui::Subscription,
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
}

impl InventoryScreen {
    pub(crate) fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        mut snapshots: watch::Receiver<Option<InventorySnapshot>>,
        fetch: InventoryFetchHandle,
    ) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search inventory"));
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
        let snapshot = snapshots.borrow_and_update().clone();
        let mut status_updates = fetch.status();
        let loading_status = status_updates.borrow_and_update().clone();
        let (request_completions, mut request_completion_events) = mpsc::channel(REQUEST_COMPLETION_CAPACITY);
        let mut screen = Self {
            snapshots: snapshot,
            fetch,
            search_input,
            type_filter,
            _search_subscription: search_subscription,
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

    fn search_active(&self) -> bool {
        self.type_filter_value != SearchTypeFilter::All || !self.search_query.is_empty()
    }

    fn search_changed(&mut self, cx: &mut Context<Self>) {
        self.search_query = self.search_input.read(cx).value().to_string();
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
            self.folder_requests.retain_snapshot(&snapshot);
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
            let theme = Theme::global(cx).colors;
            return div()
                .h(px(TREE_ROW_HEIGHT))
                .flex()
                .items_center()
                .gap_2()
                .pl(px((row.depth as f32) * INDENT_WIDTH + INDENT_WIDTH))
                .child(div().w(px(38.)).text_size(px(11.)).text_color(theme.muted_foreground).child(asset_type_label(item.asset_type)))
                .child(div().min_w_0().flex_1().text_color(theme.foreground).child(clip_text(&item.name, 80)))
                .into_any_element();
        };

        let key = CategoryKey::new(row.node.owner(), id);
        let Some(category) = snapshot.category(key) else { return div().into_any_element() };
        let expanded = self.expanded.contains(&key);
        let complete = snapshot.is_complete(key);
        let child_count = snapshot.child_count(key);
        let has_disclosure = child_count > 0 || !complete;
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
            .child(div().w(px(20.)).text_color(theme.muted_foreground).child(disclosure))
            .child(div().w(px(38.)).text_size(px(11.)).text_color(theme.muted_foreground).child("DIR"))
            .child(div().min_w_0().flex_1().text_color(theme.foreground).child(clip_text(&category.name, 80)))
            .child(div().flex_none().text_size(px(11.)).text_color(theme.muted_foreground).child(state));
        if has_disclosure {
            row_view
                .id(format!("inventory-folder-{}", id.uuid()))
                .cursor_pointer()
                .on_click(cx.listener(move |screen, _, _, cx| screen.toggle_folder(key, cx)))
                .into_any_element()
        } else {
            row_view.into_any_element()
        }
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
            .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child(self.render_status()))
            .when(self.request_error.is_some(), |view| {
                view.child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.danger)
                        .child(format!("Could not request folder: {}", self.request_error.as_deref().unwrap_or_default())),
                )
            })
            .child(content)
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
