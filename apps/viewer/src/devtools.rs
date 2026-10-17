use std::{
    cmp::Ordering,
    num::NonZeroU32,
    sync::atomic::{AtomicU64, Ordering as AtomicOrdering},
    sync::{Arc, PoisonError, RwLock, TryLockError},
    time::{Duration, Instant},
};

use base::UUID;
use gpui::{
    AnyElement, App, ColorExt as _, Context, Div, Entity, FontWeight, Hsla, IntoElement, Pixels, Render, SharedString, Stateful,
    Subscription, Window, div, prelude::*, px,
};
use gpui_component::{
    ActiveTheme, Selectable, Sizable, StyledExt, Theme,
    button::{Button, ButtonVariants},
    chart::{AreaChart, LineChart},
    h_flex,
    input::{Input, InputEvent, InputState},
    table::{Column, ColumnSort, DataTable, TableDelegate, TableEvent, TableState},
    v_flex,
};
use math::{DVec3, Quat, Vec3};
use world::{
    Kind, LocalAddress, ObjectDetail, ObjectListItem, Parent, PoseError, PoseState, Region, WorldReadError, WorldRevision,
    WorldTransformState,
};

use crate::session::{CoverageHealth, CoverageSnapshot, WorldReader, WorldReaderError};

mod world_inspector;
#[cfg(target_os = "linux")]
use crate::window_chrome::linux_titlebar;
#[cfg(target_os = "windows")]
use crate::window_chrome::windows_titlebar;
use crate::window_geometry::observe;
use world_inspector::{Refresh, ResyncOutcome, WorldMirror, capture_snapshot_cancellable, refresh_after_resync};
#[cfg(test)]
pub(crate) use world_inspector::{
    WorldMirrorForTest, capture_snapshot_after_session_change_for_test, capture_snapshot_cancellable_for_test, capture_snapshot_for_test,
};

/// Cadence of a live capture. Pause stops this loop only; ingestion is untouched.
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
/// The poll loop wakes this often to notice pause, manual refresh and selection changes without waiting a full interval.
const POLL_TICK: Duration = Duration::from_millis(100);
const CHART_WARMUP: Duration = Duration::from_secs(2);
const CHART_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const HISTORY_LEN: usize = 120;
const LOCAL_ID_GLYPH_WIDTH: f32 = 8.5;
const LOCAL_ID_CELL_PADDING: f32 = 16.;
const PANEL_BORDER_WIDTH: Pixels = px(1.);

fn should_refresh_world(paused: bool, forced: bool, resync_failed: bool, interval_elapsed: bool, has_more_work: bool) -> bool {
    forced || (!paused && !resync_failed && (interval_elapsed || has_more_work))
}

#[derive(Clone)]
struct RefreshRequest {
    selected: Option<ObjectSelection>,
    paused: bool,
    forced: bool,
    generation: u64,
}

impl RefreshRequest {
    fn is_current(&self, paused: bool, selected: Option<ObjectSelection>, generation: u64) -> bool {
        (!paused || self.forced) && self.selected == selected && self.generation == generation
    }
}

#[derive(Clone, Default)]
pub(crate) struct WorldSource(Arc<RwLock<Option<WorldReader>>>);

impl WorldSource {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set(&self, world: Option<WorldReader>) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = world.map(|reader| reader.inspector());
    }

    fn try_reader(&self) -> Option<Option<WorldReader>> {
        match self.0.try_read() {
            Ok(world) => Some(world.clone()),
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(error)) => Some(error.into_inner().clone()),
        }
    }

    fn is_current(&self, expected: &WorldReader) -> bool {
        self.try_reader().flatten().is_some_and(|current| current.same_session(expected))
    }

    fn is_disconnected(&self) -> bool {
        matches!(self.try_reader(), Some(None))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Connection {
    Offline,
    Loading,
    Live,
    Error,
    ResyncFailed,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum BottomTab {
    #[default]
    Activity,
    Diagnostics,
}

#[derive(Default)]
struct WorldInspectorState {
    selected_object: Option<ObjectSelection>,
    filter: ObjectFilter,
    inspector: InspectorState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum KindFilter {
    #[default]
    All,
    Avatars,
    Objects,
}

/// Evaluated when rows are rebuilt, never in `render`.
#[derive(Clone, Default, PartialEq, Eq)]
struct ObjectFilter {
    /// Trimmed and lowercased.
    text: String,
    kind: KindFilter,
}

impl ObjectFilter {
    fn is_active(&self) -> bool {
        !self.text.is_empty() || self.kind != KindFilter::All
    }

    fn accepts(&self, object: &ObjectRow) -> bool {
        match self.kind {
            KindFilter::All => true,
            KindFilter::Avatars => matches!(object.kind, Kind::Avatar),
            KindFilter::Objects => matches!(object.kind, Kind::Object),
        }
    }

    fn accepts_text(&self, cells: &[String]) -> bool {
        self.text.is_empty() || cells.iter().any(|cell| cell.to_lowercase().contains(&self.text))
    }
}

pub(crate) struct DeveloperFloater {
    world_inspector: WorldInspectorState,
    snapshot: WorldSnapshot,
    paused: bool,
    capture_generation: Arc<AtomicU64>,
    /// One-shot request for a capture, honoured even while paused (manual refresh, selection change).
    refresh_requested: bool,
    connection: Connection,
    error_message: Option<String>,
    bottom_open: bool,
    bottom_tab: BottomTab,
    metrics: Metrics,
    filter_input: Entity<InputState>,
    row_generation: u64,
    objects: Entity<TableState<InspectorTable>>,
    _geometry_subscription: Subscription,
    _object_table_subscription: Subscription,
    _filter_subscription: Subscription,
}

impl DeveloperFloater {
    pub(crate) fn new(source: WorldSource, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let snapshot = WorldSnapshot::default();
        let world_inspector = WorldInspectorState::default();

        let objects = cx.new(|cx| {
            TableState::new(object_table(&snapshot, &world_inspector.filter), window, cx).col_selectable(false).col_movable(false)
        });
        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter by UUID, kind, region or local ID"));
        let capture_generation = Arc::new(AtomicU64::new(0));
        let active_capture_generation = Arc::clone(&capture_generation);

        let object_table_subscription = cx.subscribe(&objects, |view, table, event: &TableEvent, cx| match event {
            TableEvent::SelectRow(row_ix) => {
                if view.paused {
                    let selected_row = table.read(cx).delegate().row_for_object(view.world_inspector.selected_object);
                    table.update(cx, |table, cx| sync_selection(table, selected_row, cx));
                    return;
                }

                let selection = table.read(cx).delegate().object_selection(*row_ix);
                if let Some(selection) = selection {
                    view.select(selection, cx);
                }
            }
            TableEvent::ColumnWidthsChanged(widths) => {
                table.update(cx, |table, _| {
                    for (column, width) in table.delegate_mut().columns.iter_mut().zip(widths) {
                        if column.resizable {
                            column.width = *width;
                        }
                    }
                });
            }
            _ => {}
        });
        let filter_subscription = cx.subscribe(&filter_input, |view, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let text = input.read(cx).value().trim().to_lowercase();
                view.set_text_filter(text, cx);
            }
        });

        let refresh_source = source;
        let background = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let initial_time = background.now();
            let mut last_refresh = initial_time.checked_sub(REFRESH_INTERVAL).unwrap_or(initial_time);
            let mut mirror = WorldMirror::default();
            let mut resync_failed = false;
            loop {
                let Ok(mut request) = this.read_with(cx, |view, _| RefreshRequest {
                    selected: view.world_inspector.selected_object,
                    paused: view.paused,
                    forced: view.refresh_requested,
                    generation: view.capture_generation.load(AtomicOrdering::Relaxed),
                }) else {
                    break;
                };

                let source_changed = refresh_source.try_reader().flatten().is_some_and(|reader| mirror.is_for_different_session(&reader));
                if source_changed {
                    let previous_selection = request.selected;
                    request.selected = None;
                    mirror.reset();
                    resync_failed = false;
                    let _ = this.update(cx, |view, cx| {
                        if view.world_inspector.selected_object == previous_selection {
                            view.clear_selection(cx);
                        }
                    });
                    let now = background.now();
                    last_refresh = now.checked_sub(REFRESH_INTERVAL).unwrap_or(now);
                }

                if request.forced && resync_failed {
                    mirror.reset();
                    resync_failed = false;
                }

                let due = should_refresh_world(
                    request.paused,
                    request.forced,
                    resync_failed,
                    background.now().saturating_duration_since(last_refresh) >= REFRESH_INTERVAL,
                    mirror.has_more_work(),
                );
                if due {
                    if request.forced {
                        let _ = this.update(cx, |view, _| view.refresh_requested = false);
                    }
                    last_refresh = background.now();
                    let source = refresh_source.clone();
                    let capture_request = request.clone();
                    let capture_generation = Arc::clone(&active_capture_generation);
                    let current_mirror = std::mem::take(&mut mirror);
                    let (refresh, next_mirror) = background
                        .spawn(async move {
                            let Some(world) = source.try_reader() else {
                                return (Refresh::Busy, current_mirror);
                            };
                            let Some(world) = world else {
                                return (Refresh::Disconnected, current_mirror);
                            };
                            let mut mirror = current_mirror;
                            let capture_started = Instant::now();
                            let captured = capture_snapshot_cancellable(&world, &mut mirror, capture_request.selected, || {
                                capture_generation.load(AtomicOrdering::Relaxed) != capture_request.generation
                            })
                            .await;
                            let capture_time = capture_started.elapsed();
                            let refresh = match captured {
                                Ok(Some(snapshot)) if capture_generation.load(AtomicOrdering::Relaxed) == capture_request.generation => {
                                    Refresh::Captured(world.clone(), snapshot, capture_time)
                                }
                                Ok(Some(_) | None) => Refresh::Cancelled,
                                Err(WorldReaderError::Busy | WorldReaderError::Read(WorldReadError::StaleRevision)) => Refresh::Busy,
                                Err(WorldReaderError::Gone) => Refresh::Disconnected,
                                Err(WorldReaderError::Read(
                                    WorldReadError::ResyncRequired | WorldReadError::InvalidCursor | WorldReadError::StaleSession,
                                )) => match refresh_after_resync(&mut mirror) {
                                    ResyncOutcome::Retry => Refresh::Resync(world.clone()),
                                    ResyncOutcome::Failed => Refresh::ResyncFailed(world.clone()),
                                },
                                Err(error) => {
                                    let message = format!("{error:?}");
                                    tracing::warn!(%message, "could not refresh Developer Tools World view");
                                    Refresh::Failed(world.clone(), message)
                                }
                            };
                            (refresh, mirror)
                        })
                        .await;
                    mirror = next_mirror;

                    if mirror.take_selection_invalidated() {
                        let previous_selection = request.selected;
                        let _ = this.update(cx, |view, cx| {
                            if view.world_inspector.selected_object == previous_selection {
                                view.clear_selection(cx);
                            }
                        });
                    }

                    match refresh {
                        Refresh::Busy => {
                            if request.forced {
                                let request = request.clone();
                                let _ = this.update(cx, move |view, _| {
                                    if request.is_current(
                                        view.paused,
                                        view.world_inspector.selected_object,
                                        view.capture_generation.load(AtomicOrdering::Relaxed),
                                    ) {
                                        view.refresh_requested = true;
                                    }
                                });
                            }
                            let now = background.now();
                            last_refresh = now.checked_sub(REFRESH_INTERVAL).unwrap_or(now);
                        }
                        Refresh::Cancelled => {}
                        Refresh::Resync(world) => {
                            let source = refresh_source.clone();
                            let request = request.clone();
                            let _ = this.update(cx, move |view, cx| {
                                if request.is_current(
                                    view.paused,
                                    view.world_inspector.selected_object,
                                    view.capture_generation.load(AtomicOrdering::Relaxed),
                                ) && source.is_current(&world)
                                {
                                    view.connection = Connection::Loading;
                                    view.error_message = None;
                                    view.update_snapshot(WorldSnapshot::default(), None, cx);
                                    view.refresh_requested = true;
                                }
                            });
                            let now = background.now();
                            last_refresh = now.checked_sub(REFRESH_INTERVAL).unwrap_or(now);
                        }
                        Refresh::ResyncFailed(world) => {
                            let source = refresh_source.clone();
                            let request = request.clone();
                            resync_failed = this
                                .update(cx, move |view, cx| {
                                    if request.is_current(
                                        view.paused,
                                        view.world_inspector.selected_object,
                                        view.capture_generation.load(AtomicOrdering::Relaxed),
                                    ) && source.is_current(&world)
                                    {
                                        view.connection = Connection::ResyncFailed;
                                        view.refresh_requested = false;
                                        view.update_snapshot(WorldSnapshot::default(), None, cx);
                                        true
                                    } else {
                                        false
                                    }
                                })
                                .unwrap_or(false);
                        }
                        Refresh::Failed(world, message) => {
                            let source = refresh_source.clone();
                            let request = request.clone();
                            let _ = this.update(cx, move |view, cx| {
                                if request.is_current(
                                    view.paused,
                                    view.world_inspector.selected_object,
                                    view.capture_generation.load(AtomicOrdering::Relaxed),
                                ) && source.is_current(&world)
                                {
                                    view.connection = Connection::Error;
                                    view.error_message = Some(message);
                                    cx.notify();
                                }
                            });
                        }
                        Refresh::Disconnected => {
                            let source = refresh_source.clone();
                            let request = request.clone();
                            let _ = this.update(cx, move |view, cx| {
                                if request.is_current(
                                    view.paused,
                                    view.world_inspector.selected_object,
                                    view.capture_generation.load(AtomicOrdering::Relaxed),
                                ) && source.is_disconnected()
                                {
                                    view.connection = Connection::Offline;
                                    view.error_message = None;
                                    view.world_inspector.selected_object = None;
                                    view.update_snapshot(WorldSnapshot::default(), None, cx);
                                }
                            });
                        }
                        Refresh::Captured(world, snapshot, capture_time) => {
                            let source = refresh_source.clone();
                            let request = request.clone();
                            let _ = this.update(cx, move |view, cx| {
                                if request.is_current(
                                    view.paused,
                                    view.world_inspector.selected_object,
                                    view.capture_generation.load(AtomicOrdering::Relaxed),
                                ) && source.is_current(&world)
                                {
                                    view.connection = Connection::Live;
                                    view.error_message = None;
                                    view.update_snapshot(snapshot, Some(capture_time), cx);
                                }
                            });
                        }
                    }
                }
                background.timer(POLL_TICK).await;
            }
        })
        .detach();

        let geometry_subscription = observe("developer-tools", window, cx);
        Self {
            world_inspector,
            snapshot,
            paused: false,
            capture_generation,
            refresh_requested: false,
            connection: Connection::Loading,
            error_message: None,
            bottom_open: true,
            bottom_tab: BottomTab::Activity,
            metrics: Metrics::default(),
            filter_input,
            row_generation: 0,
            objects,
            _geometry_subscription: geometry_subscription,
            _object_table_subscription: object_table_subscription,
            _filter_subscription: filter_subscription,
        }
    }

    #[cfg(test)]
    pub(crate) fn captured_world_state(&self) -> (Option<WorldRevision>, usize) {
        (self.snapshot.revision, self.snapshot.object_count)
    }

    #[cfg(test)]
    pub(crate) fn toggle_pause_for_test(&mut self, cx: &mut Context<Self>) {
        self.toggle_pause(cx);
    }

    fn request_refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh_requested = true;
        cx.notify();
    }

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        self.paused = !self.paused;
        if self.paused {
            self.capture_generation.fetch_add(1, AtomicOrdering::Relaxed);
        }
        let selectable = !self.paused;
        if !self.paused {
            self.metrics.resume();
            self.refresh_requested = true;
        }
        self.objects.update(cx, |table, cx| {
            table.row_selectable = selectable;
            cx.notify();
        });
        cx.notify();
    }

    fn select(&mut self, selection: ObjectSelection, cx: &mut Context<Self>) {
        if !self.paused && self.world_inspector.selected_object != Some(selection) {
            self.world_inspector.selected_object = Some(selection);
            self.world_inspector.inspector = inspector_state(&self.snapshot, self.world_inspector.selected_object);
            let selected = self.world_inspector.selected_object;
            self.objects.update(cx, |table, _| table.delegate_mut().selected = selected);
            self.refresh_requested = true;
            cx.notify();
        }
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.world_inspector.selected_object = None;
        self.world_inspector.inspector = inspector_state(&self.snapshot, None);
        self.objects.update(cx, |table, cx| {
            table.delegate_mut().selected = None;
            sync_selection(table, None, cx);
        });
        cx.notify();
    }

    fn set_bottom_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let tab = if index == 1 { BottomTab::Diagnostics } else { BottomTab::Activity };
        if self.bottom_tab != tab {
            self.bottom_tab = tab;
            cx.notify();
        }
    }

    fn toggle_bottom(&mut self, cx: &mut Context<Self>) {
        self.bottom_open = !self.bottom_open;
        cx.notify();
    }

    fn set_text_filter(&mut self, text: String, cx: &mut Context<Self>) {
        if self.world_inspector.filter.text != text {
            self.world_inspector.filter.text = text;
            self.refilter(cx);
        }
    }

    fn set_kind_filter(&mut self, index: usize, cx: &mut Context<Self>) {
        let kind = match index {
            1 => KindFilter::Avatars,
            2 => KindFilter::Objects,
            _ => KindFilter::All,
        };
        if self.world_inspector.filter.kind != kind {
            self.world_inspector.filter.kind = kind;
            self.refilter(cx);
        }
    }

    fn object_empty_message(&self) -> String {
        if self.connection == Connection::ResyncFailed {
            "World changes outpaced the inspector. Select Refresh to try again.".into()
        } else if self.connection == Connection::Loading {
            "Loading World data…".into()
        } else if self.connection == Connection::Offline {
            "Not connected to a session.".into()
        } else if self.connection == Connection::Error {
            self.error_message.clone().unwrap_or_else(|| "Could not read World data.".into())
        } else if self.snapshot.objects.len() < self.snapshot.object_count {
            "Loading World objects…".into()
        } else if self.world_inspector.filter.is_active() {
            "No objects match the current filter.".into()
        } else {
            "No objects observed.".into()
        }
    }

    /// Rebuild the object rows from the current snapshot and filter, keeping the highlighted row on the selected object.
    fn refilter(&mut self, cx: &mut Context<Self>) {
        self.row_generation = self.row_generation.saturating_add(1);
        let generation = self.row_generation;
        let snapshot = self.snapshot.clone();
        let filter = self.world_inspector.filter.clone();
        let sort = self.objects.read(cx).delegate().sort;
        let sort_generation = self.objects.read(cx).delegate().sort_generation;
        let empty_message = self.object_empty_message();
        let background = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let (rows, width) = background
                .spawn(async move {
                    let mut rows = object_table_rows(&snapshot, &filter);
                    if let Some((column, sort)) = sort {
                        sort_inspector_rows(&mut rows, column, sort);
                    }
                    let width = local_id_column_width(&rows);
                    (rows, width)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.row_generation != generation {
                    return;
                }
                if view.objects.read(cx).delegate().sort_generation != sort_generation {
                    view.refilter(cx);
                    return;
                }
                let selected = view.world_inspector.selected_object;
                view.objects.update(cx, |table, cx| {
                    let delegate = table.delegate_mut();
                    delegate.set_rows(rows, sort, width);
                    delegate.selected = selected;
                    delegate.empty_message = empty_message;
                    let selected_row = table.delegate().row_for_object(selected);
                    sync_selection(table, selected_row, cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn update_snapshot(&mut self, snapshot: WorldSnapshot, capture: Option<Duration>, cx: &mut Context<Self>) {
        let table_changed =
            !Arc::ptr_eq(&self.snapshot.region_labels, &snapshot.region_labels) || !Arc::ptr_eq(&self.snapshot.objects, &snapshot.objects);
        self.snapshot = snapshot;
        if let (Some(capture), false) = (capture, self.paused) {
            self.metrics.record(&self.snapshot, capture, Instant::now());
        }
        self.world_inspector.inspector = inspector_state(&self.snapshot, self.world_inspector.selected_object);
        if table_changed {
            self.refilter(cx);
        } else {
            cx.notify();
        }
    }
}

/// Move the table highlight to `row` (or clear it) without emitting redundant selection events.
fn sync_selection(table: &mut TableState<InspectorTable>, row: Option<usize>, cx: &mut Context<TableState<InspectorTable>>) {
    match (row, table.selected_row()) {
        (Some(row_ix), current) if current != Some(row_ix) => table.set_selected_row(row_ix, cx),
        (None, Some(_)) => table.clear_selection(cx),
        _ => {}
    }
}

#[derive(Clone)]
struct ChartPoint {
    elapsed: SharedString,
    object_count: usize,
    objects: f64,
    revision_rate: f64,
    capture_ms: f64,
    capture_peak_ms: f64,
}

#[derive(Default)]
struct Metrics {
    chart: Vec<ChartPoint>,
    started_at: Option<Instant>,
    last_sample_at: Option<Instant>,
    sampled_revision: Option<u64>,
    capture_ms_total: f64,
    capture_peak_ms: f64,
    capture_count: usize,
    resume_pending: bool,
}

impl Metrics {
    fn record(&mut self, snapshot: &WorldSnapshot, capture: Duration, now: Instant) {
        let revision = snapshot.revision.map(|revision| revision.get());
        let started_at = *self.started_at.get_or_insert(now);
        if self.last_sample_at.is_none() || self.resume_pending {
            self.last_sample_at = Some(now);
            self.sampled_revision = revision;
            self.capture_ms_total = 0.0;
            self.capture_peak_ms = 0.0;
            self.capture_count = 0;
            self.resume_pending = false;
        }

        let capture_ms = capture.as_secs_f64() * 1000.0;
        self.capture_ms_total += capture_ms;
        self.capture_peak_ms = self.capture_peak_ms.max(capture_ms);
        self.capture_count += 1;

        let elapsed = now.saturating_duration_since(started_at);
        let sample_elapsed = now.saturating_duration_since(self.last_sample_at.expect("activity sampling starts on first capture"));
        if elapsed < CHART_WARMUP || sample_elapsed < CHART_SAMPLE_INTERVAL {
            return;
        }

        let revision_rate = match (self.sampled_revision, revision) {
            (Some(previous), Some(current)) => revision_rate(previous, current, sample_elapsed),
            _ => 0.0,
        };
        self.chart.push(ChartPoint {
            elapsed: elapsed_label(elapsed).into(),
            object_count: snapshot.object_count,
            objects: snapshot.object_count as f64,
            revision_rate,
            capture_ms: self.capture_ms_total / self.capture_count as f64,
            capture_peak_ms: self.capture_peak_ms,
        });
        if self.chart.len() > HISTORY_LEN {
            self.chart.remove(0);
        }
        self.last_sample_at = Some(now);
        self.sampled_revision = revision;
        self.capture_ms_total = 0.0;
        self.capture_peak_ms = 0.0;
        self.capture_count = 0;
    }

    fn resume(&mut self) {
        self.resume_pending = self.started_at.is_some();
    }

    fn latest(&self) -> Option<&ChartPoint> {
        self.chart.last()
    }

    fn peak_capture_ms(&self) -> Option<f64> {
        self.chart.iter().map(|point| point.capture_peak_ms).reduce(f64::max)
    }
}

fn revision_rate(previous: u64, current: u64, elapsed: Duration) -> f64 {
    current.saturating_sub(previous) as f64 / elapsed.as_secs_f64().max(0.001)
}

fn elapsed_label(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// Every surface comes from the app theme tokens (no hardcoded grays), so a theme change restyles the floater.
#[derive(Clone)]
struct Palette {
    background: Hsla,
    foreground: Hsla,
    muted: Hsla,
    /// Panels and cards; matches the table background (`group_box`).
    surface: Hsla,
    /// Segmented-control track and chip fills.
    raised: Hsla,
    hairline: Hsla,
    /// Active segment (the theme's `primary` is a neutral raised gray).
    selected: Hsla,
    panel_radius: Pixels,
    input: Hsla,
    mono: SharedString,
}

impl Palette {
    fn new(cx: &App) -> Self {
        let theme = Theme::global(cx);
        let colors = theme.colors;
        Self {
            background: colors.background,
            foreground: colors.foreground,
            muted: colors.muted_foreground,
            surface: colors.group_box,
            raised: colors.muted,
            hairline: colors.border,
            selected: colors.primary,
            panel_radius: theme.radius_tokens().xl,
            input: colors.input,
            mono: theme.mono_font_family.clone(),
        }
    }
}

impl Render for DeveloperFloater {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(cx);
        let content = div().size_full().flex().flex_col().bg(p.background).text_color(p.foreground).text_sm();

        #[cfg(target_os = "windows")]
        let content = {
            let mut theme = Theme::global(cx).colors;
            theme.danger_hover = theme.secondary_hover;
            theme.danger_active = theme.secondary_active;
            content.child(windows_titlebar(
                window.is_maximized(),
                div().flex().items_center().gap_3().child(div().text_sm().font_semibold().child("Developer Tools")).into_any_element(),
                div().into_any_element(),
                theme,
            ))
        };

        #[cfg(target_os = "linux")]
        let content = {
            let titlebar_content =
                div().flex().items_center().gap_3().child(div().text_sm().font_semibold().child("Developer Tools")).into_any_element();

            if let Some(titlebar) = linux_titlebar(window, titlebar_content, div().into_any_element()) {
                content.child(titlebar)
            } else {
                content
            }
        };
        content.child(self.render_header(&p, cx)).child(self.render_body(&p, cx)).child(self.render_bottom(&p, cx))
    }
}

impl DeveloperFloater {
    fn render_header(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let pause_label = if self.paused { "Resume updates" } else { "Pause updates" };
        let pause = Button::new("developer-pause")
            .ghost()
            .compact()
            .selected(self.paused)
            .child(pause_label)
            .on_click(cx.listener(|view, _, _, cx| view.toggle_pause(cx)));
        let refresh = Button::new("developer-refresh")
            .ghost()
            .compact()
            .child("Refresh")
            .on_click(cx.listener(|view, _, _, cx| view.request_refresh(cx)));
        let filter = div()
            .w(px(300.))
            .h(px(30.))
            .flex()
            .items_center()
            .px_3()
            .rounded_full()
            .bg(p.input)
            .border_1()
            .border_color(p.hairline)
            .child(Input::new(&self.filter_input).appearance(false));

        div()
            .h(px(52.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .child(div().flex_1())
            .child(filter)
            .child(refresh)
            .child(pause)
    }

    fn render_body(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        self.render_world_tool(p, cx)
    }

    fn render_world_tool(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let kind_tabs = segmented(
            "developer-kind-filter",
            vec!["All".into(), "Avatars".into(), "Objects".into()],
            match self.world_inspector.filter.kind {
                KindFilter::All => 0,
                KindFilter::Avatars => 1,
                KindFilter::Objects => 2,
            },
            p,
            cx,
            |view, index, cx| view.set_kind_filter(index, cx),
        );
        let objects_header = panel_header("Objects", p).child(kind_tabs);
        let objects = panel(p).flex_1().child(objects_header).child(table_body(
            "developer-objects",
            DataTable::new(&self.objects).small().bordered(false).scrollbar_visible(true, true),
            p,
        ));

        let inspector = self.render_inspector(p);
        div().flex_1().min_h_0().flex().gap_2().px_3().pb_3().child(objects).child(inspector)
    }

    fn render_inspector(&self, p: &Palette) -> impl IntoElement {
        let header = panel_header("Inspector", p);

        let body = match &self.world_inspector.inspector {
            InspectorState::Empty => {
                empty_state("Select an object", "Pick a row to inspect its identity, hierarchy, transform and ownership.", p)
            }
            InspectorState::Updating => empty_state("Updating details…", "Fetching the latest state for this object.", p),
            InspectorState::Removed => empty_state("Object is no longer available", "This selection belongs to a removed World entity.", p),
            InspectorState::Error(error) => empty_state("Details unavailable", error, p),
            InspectorState::Ready(groups) => {
                let mut next_id = 0usize;
                let mut sections = Vec::with_capacity(groups.len());
                for group in groups {
                    let mut rows = Vec::with_capacity(group.properties.len());
                    for property in &group.properties {
                        next_id += 1;
                        rows.push(property_row(next_id, property, p));
                    }
                    sections.push(
                        v_flex()
                            .w_full()
                            .min_w_0()
                            .gap_0p5()
                            .child(div().px_2().pt_3().pb_1().text_xs().font_semibold().text_color(p.muted).child(group.title))
                            .children(rows),
                    );
                }
                div()
                    .id("developer-inspector")
                    .flex_1()
                    .w_full()
                    .min_w_0()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_1()
                    .pb_2()
                    .child(v_flex().w_full().min_w_0().children(sections))
                    .into_any_element()
            }
        };

        let panel = panel(p).w(px(340.)).flex_shrink_0().bg(p.surface).child(header).child(body).id("developer-inspector-panel");
        #[cfg(test)]
        let panel = panel.debug_selector(|| "developer-inspector-panel".to_owned());
        panel
    }

    fn render_bottom(&self, p: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let tabs = segmented(
            "developer-bottom-tab",
            vec!["Activity".into(), "Diagnostics".into()],
            match self.bottom_tab {
                BottomTab::Activity => 0,
                BottomTab::Diagnostics => 1,
            },
            p,
            cx,
            |view, index, cx| view.set_bottom_tab(index, cx),
        );
        let toggle = Button::new("developer-bottom-toggle")
            .ghost()
            .compact()
            .child(if self.bottom_open { "Hide" } else { "Show" })
            .on_click(cx.listener(|view, _, _, cx| view.toggle_bottom(cx)));
        let header = div()
            .h(px(38.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .border_b_1()
            .border_color(p.hairline)
            .child(tabs)
            .child(div().flex_1())
            .child(toggle);

        let bottom = panel(p)
            .id("developer-bottom-panel")
            .h(px(if self.bottom_open { 214. } else { 38. }))
            .flex_shrink_0()
            .mx_3()
            .mb_3()
            .child(header);
        #[cfg(test)]
        let bottom = bottom.debug_selector(|| "developer-bottom-panel".to_owned());
        if self.bottom_open {
            let body = match self.bottom_tab {
                BottomTab::Activity => self.render_activity(p),
                BottomTab::Diagnostics => self.render_diagnostics(p),
            };
            bottom.child(body)
        } else {
            bottom
        }
    }

    fn render_activity(&self, p: &Palette) -> Div {
        let data = &self.metrics.chart;
        if data.len() < 2 {
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(p.muted)
                .child("Collecting samples…");
        }

        let objects = AreaChart::new(data.clone())
            .id("chart-objects")
            .x(|point| point.elapsed.clone())
            .y(|point| point.objects)
            .name("Objects")
            .stroke(p.muted)
            .fill(p.muted.opacity(0.18))
            .natural()
            .tick_margin(20);
        let revisions = LineChart::new(data.clone())
            .id("chart-revisions")
            .x(|point| point.elapsed.clone())
            .y(|point| point.revision_rate)
            .name("Revisions/s")
            .stroke(p.muted)
            .natural()
            .tick_margin(20);
        let capture = LineChart::new(data.clone())
            .id("chart-capture")
            .x(|point| point.elapsed.clone())
            .y(|point| point.capture_ms)
            .name("Capture (ms)")
            .stroke(p.muted)
            .natural()
            .tick_margin(20);

        let latest = self.metrics.latest();
        let object_count = latest.map_or_else(|| "—".into(), |point| format_count(point.object_count));
        let revision_rate = latest.map_or_else(|| "—".into(), |point| format!("{:.1}", point.revision_rate));
        let capture_ms = latest.map_or_else(|| "—".into(), |point| format!("{:.2} ms", point.capture_ms));
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .gap_4()
            .p_3()
            .child(chart_cell("Objects", object_count, objects, p))
            .child(chart_cell("Revisions / s", revision_rate, revisions, p))
            .child(chart_cell("Capture latency", capture_ms, capture, p))
    }

    fn render_diagnostics(&self, p: &Palette) -> Div {
        let revision = self.snapshot.revision.map_or_else(|| "—".to_owned(), |revision| revision.get().to_string());
        let coverage = self.snapshot.coverage.as_ref().map_or_else(
            || "—".to_owned(),
            |coverage| {
                let health = match coverage.health {
                    CoverageHealth::NoKnownLoss => "No known loss",
                    CoverageHealth::Uncertain => "Uncertain",
                    CoverageHealth::Degraded => "Degraded",
                };
                let local_losses = coverage.local_losses.iter().fold(0_u64, |total, loss| total.saturating_add(loss.count));
                format!(
                    "{health} at revision {}; {} local loss events ({} overflow); {} pending; {} degraded",
                    coverage.world_revision.get(),
                    local_losses,
                    coverage.local_overflow_count,
                    coverage.recovery.pending + coverage.recovery.in_flight,
                    coverage.recovery.degraded
                )
            },
        );
        let selected = self.world_inspector.selected_object.map_or_else(|| "None".to_owned(), |selection| selection.id.to_string());
        let capture = self.metrics.latest().map_or_else(|| "—".to_owned(), |point| format!("{:.2} ms", point.capture_ms));
        let peak_capture = self.metrics.peak_capture_ms().map_or_else(|| "—".to_owned(), |peak| format!("{peak:.2} ms"));
        let status = match self.connection {
            Connection::Live => "Connected",
            Connection::Offline => "Disconnected",
            Connection::Loading => "Loading",
            Connection::Error => "Error",
            Connection::ResyncFailed => "Refresh required",
        };
        let capture_state = if self.paused { "Paused" } else { "Updating" };
        let buffered = format!("{} / {} captures", self.metrics.chart.len(), HISTORY_LEN);
        let diagnostics = [
            ("Connection", status.to_owned()),
            ("Capture", capture_state.to_owned()),
            ("Revision", revision),
            ("Coverage", coverage),
            ("Regions", self.snapshot.region_labels.len().to_string()),
            ("Objects", format_count(self.snapshot.object_count)),
            ("Selected object", selected),
            ("Last capture", capture),
            ("Peak capture", peak_capture),
            ("History", buffered),
        ];

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_wrap()
            .content_start()
            .gap_x_6()
            .gap_y_3()
            .p_3()
            .children(diagnostics.into_iter().map(|(label, value)| diagnostic_cell(label, value, p)))
    }
}

fn panel(p: &Palette) -> Div {
    div()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .rounded(p.panel_radius)
        .border(PANEL_BORDER_WIDTH)
        .border_color(p.hairline)
        .bg(p.surface)
        .overflow_hidden()
}

fn panel_header(title: &'static str, p: &Palette) -> Div {
    div()
        .h(px(38.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .px_3()
        .border_b_1()
        .border_color(p.hairline)
        .child(div().text_xs().font_semibold().child(title))
}

fn table_body(id: &'static str, table: impl IntoElement, p: &Palette) -> Stateful<Div> {
    let radius = table_inner_corner_radius(p.panel_radius);
    let body = div().id(id);
    #[cfg(test)]
    let body = body.debug_selector(|| format!("table-body-{id}"));
    let corners = div().absolute().inset(-radius).border(radius).border_color(p.background).rounded_b(radius * 2.);
    let border = div().absolute().inset(-PANEL_BORDER_WIDTH).border(PANEL_BORDER_WIDTH).border_color(p.hairline).rounded_b(p.panel_radius);

    body.relative().flex_1().min_h_0().rounded_b(radius).overflow_hidden().child(table).child(corners).child(border)
}

fn table_inner_corner_radius(panel_radius: Pixels) -> Pixels {
    (panel_radius - PANEL_BORDER_WIDTH).max(px(0.))
}

/// Capsule segmented control in the macOS style. `on_select` receives the clicked index.
fn segmented(
    id: &'static str,
    labels: Vec<SharedString>,
    selected: usize,
    p: &Palette,
    cx: &mut Context<DeveloperFloater>,
    on_select: fn(&mut DeveloperFloater, usize, &mut Context<DeveloperFloater>),
) -> Stateful<Div> {
    let (foreground, muted, selected_bg) = (p.foreground, p.muted, p.selected);
    let control = div().id(id).flex().items_center().gap(px(2.)).p(px(2.)).rounded_lg().bg(p.raised).border_1().border_color(p.hairline);
    #[cfg(test)]
    let control = control.debug_selector(|| format!("{id}-control"));
    let mut control = control;
    for (index, label) in labels.into_iter().enumerate() {
        let active = index == selected;
        control = control.child(
            Button::new((id, index))
                .ghost()
                .compact()
                .selected(active)
                .toggled(active)
                .h(px(22.))
                .px_2()
                .rounded_md()
                .text_xs()
                .whitespace_nowrap()
                .when(active, |item| item.bg(selected_bg).text_color(foreground).shadow_sm())
                .when(!active, |item| item.text_color(muted))
                .on_click(cx.listener(move |view, _, _, cx| on_select(view, index, cx)))
                .child(label),
        );
    }
    control
}

fn chart_cell(title: &'static str, value: String, chart: impl IntoElement, p: &Palette) -> Div {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(div().text_xs().text_color(p.muted).child(title))
                .child(div().font_family(p.mono.clone()).text_xs().child(value)),
        )
        .child(div().h(px(120.)).w_full().child(chart))
}

fn diagnostic_cell(label: &'static str, value: String, p: &Palette) -> Div {
    v_flex()
        .flex_1()
        .min_w(px(150.))
        .gap_1()
        .child(div().text_xs().text_color(p.muted).child(label))
        .child(div().font_family(p.mono.clone()).text_xs().truncate().child(value))
}

fn format_count(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped.chars().rev().collect()
}

fn empty_state(title: impl Into<SharedString>, hint: impl Into<SharedString>, p: &Palette) -> AnyElement {
    v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_1()
        .px_6()
        .child(div().text_sm().font_semibold().child(title.into()))
        .child(div().text_xs().text_color(p.muted).text_center().child(hint.into()))
        .into_any_element()
}

/// Property rows put labels above values so long protocol identifiers get the full panel width.
fn property_row(index: usize, property: &Property, p: &Palette) -> AnyElement {
    let value = match &property.value {
        PropertyValue::Text(text) => div().w_full().min_w_0().whitespace_normal().child(text.clone()),
        PropertyValue::Id(text) => div().w_full().min_w_0().whitespace_normal().font_family(p.mono.clone()).child(text.clone()),
        PropertyValue::Vector(components) => vector_chips(components, p),
        PropertyValue::Warning(text) => div().w_full().min_w_0().whitespace_normal().text_xs().text_color(p.foreground).child(text.clone()),
        PropertyValue::Unknown => div().italic().text_color(p.muted.opacity(0.8)).child("Unknown"),
    };
    let label = property.label;
    let value = div().id(("property-value", index)).min_w_0().w_full().child(value);
    #[cfg(test)]
    let value = value.debug_selector(|| format!("inspector-property-value-{index}"));
    let heading = h_flex()
        .w_full()
        .min_w_0()
        .items_center()
        .justify_between()
        .gap_2()
        .child(div().min_w_0().text_xs().text_color(p.muted).child(label));
    let row = v_flex().id(("property", index)).w_full().min_w_0().gap_1().px_2().py(px(5.)).rounded_md().child(heading).child(value);
    row.into_any_element()
}

fn vector_chips(components: &[String], p: &Palette) -> Div {
    let axes = ["X", "Y", "Z", "W"];
    div().flex().flex_wrap().gap(px(4.)).children(components.iter().zip(axes).map(|(value, axis)| {
        div()
            .h(px(20.))
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(6.))
            .rounded_md()
            .bg(p.raised)
            .font_family(p.mono.clone())
            .text_xs()
            .child(div().text_color(p.muted).font_weight(FontWeight::SEMIBOLD).child(axis))
            .child(div().child(value.clone()))
    }))
}

#[derive(Clone, Copy)]
enum CellKind {
    Text,
    Uuid,
    Number,
}

struct InspectorTable {
    columns: Vec<Column>,
    cell_kinds: Vec<CellKind>,
    rows: Arc<[InspectorRow]>,
    selected: Option<ObjectSelection>,
    sort: Option<(usize, ColumnSort)>,
    data_generation: u64,
    sort_generation: u64,
    empty_message: String,
}

impl InspectorTable {
    fn object_selection(&self, row_ix: usize) -> Option<ObjectSelection> {
        self.rows.get(row_ix).map(|row| row.selection)
    }

    fn row_for_object(&self, selected: Option<ObjectSelection>) -> Option<usize> {
        let selected = selected?;
        self.rows.iter().position(|row| row.selection == selected)
    }

    fn set_rows(&mut self, rows: Vec<InspectorRow>, sort: Option<(usize, ColumnSort)>, width: f32) {
        self.data_generation = self.data_generation.saturating_add(1);
        self.sort_generation = self.sort_generation.saturating_add(1);
        self.rows = Arc::from(rows);
        self.sort = sort;
        self.columns[3].width = px(width);
    }

    #[cfg(test)]
    fn sort_rows(&mut self, column: usize, sort: ColumnSort) {
        self.sort = Some((column, sort));
        self.apply_sort();
    }

    #[cfg(test)]
    fn apply_sort(&mut self) {
        let Some((column, sort)) = self.sort else {
            return;
        };
        sort_inspector_rows(Arc::make_mut(&mut self.rows), column, sort);
    }
}

impl TableDelegate for InspectorTable {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let mut column = self.columns.get(col_ix).cloned().unwrap_or_default();
        if let Some((sorted_column, sort)) = self.sort
            && col_ix == sorted_column
        {
            column.sort = Some(sort);
        }
        column
    }

    fn render_th(&mut self, col_ix: usize, _: &mut Window, _: &mut Context<TableState<Self>>) -> impl IntoElement {
        let name = self.columns.get(col_ix).map_or_else(SharedString::default, |column| column.name.clone());
        let label = div().min_w_0().whitespace_nowrap().truncate().child(name);
        #[cfg(test)]
        let label = label.debug_selector(|| format!("inspector-header-{col_ix}"));
        label
    }

    fn perform_sort(&mut self, col_ix: usize, sort: ColumnSort, _: &mut Window, cx: &mut Context<TableState<Self>>) {
        self.sort = Some((col_ix, sort));
        self.sort_generation = self.sort_generation.saturating_add(1);
        let data_generation = self.data_generation;
        let sort_generation = self.sort_generation;
        let mut rows = Arc::clone(&self.rows);
        let background = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let rows = background
                .spawn(async move {
                    sort_inspector_rows(Arc::make_mut(&mut rows), col_ix, sort);
                    rows
                })
                .await;
            let _ = this.update(cx, |table, cx| {
                let delegate = table.delegate_mut();
                if delegate.data_generation != data_generation || delegate.sort_generation != sort_generation {
                    return;
                }
                delegate.rows = rows;
                let row = delegate.row_for_object(delegate.selected);
                sync_selection(table, row, cx);
                table.refresh(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn render_tr(&mut self, row_ix: usize, _: &mut Window, _: &mut Context<TableState<Self>>) -> Stateful<Div> {
        match self.rows.get(row_ix) {
            Some(row) => div().id(row.key.clone()),
            None => div().id(("row", row_ix)),
        }
    }

    fn render_td(&mut self, row_ix: usize, col_ix: usize, _: &mut Window, cx: &mut Context<TableState<Self>>) -> impl IntoElement {
        let value = self.rows.get(row_ix).and_then(|row| row.cells.get(col_ix)).cloned().unwrap_or_default();
        let kind = self.cell_kinds.get(col_ix).copied().unwrap_or(CellKind::Text);
        let theme = cx.theme();
        let colors = theme.colors;
        let mono = theme.mono_font_family.clone();

        let cell = div().min_w_0().whitespace_nowrap().truncate().px_2();
        #[cfg(test)]
        let cell = cell.debug_selector(|| format!("inspector-cell-{row_ix}-{col_ix}"));
        match kind {
            CellKind::Text => cell.child(value),
            CellKind::Number => cell.font_family(mono).text_color(colors.muted_foreground).child(value),
            CellKind::Uuid => cell.font_family(mono).text_color(colors.foreground).child(value),
        }
    }

    fn render_empty(&mut self, _: &mut Window, cx: &mut Context<TableState<Self>>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(self.empty_message.clone())
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        self.rows.get(row_ix).and_then(|row| row.cells.get(col_ix)).cloned().unwrap_or_default()
    }
}

fn object_table(snapshot: &WorldSnapshot, filter: &ObjectFilter) -> InspectorTable {
    let rows = object_table_rows(snapshot, filter);
    InspectorTable {
        columns: vec![
            Column::new("uuid", "UUID").width(px(280.)).sortable(),
            Column::new("kind", "Kind").width(px(96.)).sortable(),
            Column::new("region", "Region").width(px(128.)).sortable(),
            Column::new("local_id", "Local ID").width(px(local_id_column_width(&rows))).text_right().sortable().resizable(false),
        ],
        cell_kinds: vec![CellKind::Uuid, CellKind::Text, CellKind::Text, CellKind::Number],
        rows: Arc::from(rows),
        selected: None,
        sort: None,
        data_generation: 0,
        sort_generation: 0,
        empty_message: "No objects observed.".into(),
    }
}

fn local_id_column_width(rows: &[InspectorRow]) -> f32 {
    let longest = rows.iter().map(|row| row.cells[3].len()).max().unwrap_or(0).max("Local ID".len());
    longest as f32 * LOCAL_ID_GLYPH_WIDTH + LOCAL_ID_CELL_PADDING
}

fn sort_inspector_rows(rows: &mut [InspectorRow], column: usize, sort: ColumnSort) {
    rows.sort_by(|left, right| {
        let ordering = match column {
            0 => left.selection.id.cmp(&right.selection.id),
            1 | 2 => left.cells.get(column).cmp(&right.cells.get(column)),
            3 => left.local_id.cmp(&right.local_id),
            _ => Ordering::Equal,
        };
        let ordering = if sort == ColumnSort::Descending { ordering.reverse() } else { ordering };
        ordering.then_with(|| left.selection.id.cmp(&right.selection.id))
    });
}

fn object_table_rows(snapshot: &WorldSnapshot, filter: &ObjectFilter) -> Vec<InspectorRow> {
    snapshot
        .objects
        .iter()
        .filter(|object| filter.accepts(object))
        .map(|object| InspectorRow {
            key: format!("object-{}", object.selection.id),
            selection: object.selection,
            local_id: object.local_id,
            cells: vec![
                object.selection.id.to_string(),
                kind_label(object.kind),
                snapshot.region_label(object.region),
                object.local_id.to_string(),
            ],
        })
        .filter(|row| filter.accepts_text(&row.cells))
        .collect()
}

fn kind_label(kind: Kind) -> String {
    match kind {
        Kind::Avatar => "Avatar".into(),
        Kind::Object => "Object".into(),
        Kind::Unknown(code) => format!("Unknown ({code})"),
    }
}

#[derive(Default)]
enum InspectorState {
    #[default]
    Empty,
    Updating,
    Removed,
    Error(String),
    Ready(Vec<PropertyGroup>),
}

struct PropertyGroup {
    title: &'static str,
    properties: Vec<Property>,
}

struct Property {
    label: &'static str,
    value: PropertyValue,
}

#[derive(Clone, Debug, PartialEq)]
enum PropertyValue {
    Text(String),
    /// A UUID; copyable.
    Id(String),
    /// Three (XYZ) or four (XYZW) components.
    Vector(Vec<String>),
    /// Unresolved or inconsistent state worth drawing attention to.
    Warning(String),
    Unknown,
}

impl PropertyValue {
    fn text(&self) -> String {
        match self {
            Self::Text(text) | Self::Id(text) | Self::Warning(text) => text.clone(),
            Self::Vector(components) => components.join(", "),
            Self::Unknown => "Unknown".into(),
        }
    }
}

fn inspector_state(snapshot: &WorldSnapshot, selected: Option<ObjectSelection>) -> InspectorState {
    if let Some(selection) = selected {
        let Some(details) = snapshot.details.as_ref().filter(|details| details.selection == selection) else {
            return if snapshot.objects.iter().any(|object| object.selection == selection) {
                snapshot.details_error.clone().map_or(InspectorState::Updating, InspectorState::Error)
            } else {
                InspectorState::Removed
            };
        };
        return InspectorState::Ready(property_groups(snapshot, details));
    }

    InspectorState::Empty
}

fn property_groups(snapshot: &WorldSnapshot, details: &ObjectDetails) -> Vec<PropertyGroup> {
    let property = |label, value| Property { label, value };
    let (world_position, world_rotation, world_scale, world_state) = world_transform_values(&details.world_transform);
    vec![
        PropertyGroup {
            title: "Identity",
            properties: vec![
                property("UUID", PropertyValue::Id(details.selection.id.to_string())),
                property("Kind", PropertyValue::Text(kind_label(details.kind))),
                property("Region", PropertyValue::Text(snapshot.region_label(details.address.region()))),
                property("Local ID", PropertyValue::Text(details.address.local_id().to_string())),
            ],
        },
        PropertyGroup {
            title: "Hierarchy",
            properties: vec![
                property("Parent", parent_value(&details.parent)),
                property(
                    "Attachment point",
                    details.attachment_point.map_or(PropertyValue::Unknown, |point| PropertyValue::Text(point.get().to_string())),
                ),
                property("Sit target", sit_target_value(details.sit_target)),
            ],
        },
        PropertyGroup {
            title: "Local transform",
            properties: vec![
                property("Position", vector3(details.position)),
                property("Rotation", quaternion(details.rotation)),
                property("Scale", vector3(details.scale)),
            ],
        },
        PropertyGroup {
            title: "Global transform",
            properties: vec![
                property("State", world_state),
                property("Position", world_position),
                property("Rotation", world_rotation),
                property("Scale", world_scale),
            ],
        },
        PropertyGroup {
            title: "Motion",
            properties: vec![
                property("Velocity", vector3(details.velocity)),
                property("Acceleration", vector3(details.acceleration)),
                property("Angular velocity", vector3(details.angular_velocity)),
            ],
        },
        PropertyGroup {
            title: "Ownership",
            properties: vec![property("Owner", id_value(details.owner)), property("Creator", id_value(details.creator))],
        },
    ]
}

fn sit_target_value(value: Option<world::SitTarget>) -> PropertyValue {
    value.map_or(PropertyValue::Unknown, |target| {
        PropertyValue::Text(format!(
            "position ({}) · rotation ({})",
            vector3(Some(target.position)).text(),
            quaternion(Some(target.rotation)).text()
        ))
    })
}

fn world_transform_values(
    transform: &Result<WorldTransformState, PoseError>,
) -> (PropertyValue, PropertyValue, PropertyValue, PropertyValue) {
    let unknown = || PropertyValue::Unknown;
    match transform {
        Ok(WorldTransformState::Resolved(transform)) => (
            dvector3(Some(transform.pose.position)),
            quaternion(Some(transform.pose.rotation)),
            vector3(Some(transform.scale)),
            PropertyValue::Text("Resolved".into()),
        ),
        Ok(WorldTransformState::MissingScale(pose)) => (
            dvector3(Some(pose.position)),
            quaternion(Some(pose.rotation)),
            unknown(),
            PropertyValue::Warning("Resolved pose; scale unavailable".into()),
        ),
        Ok(WorldTransformState::Unresolved(PoseState::Resolved(pose))) => (
            dvector3(Some(pose.position)),
            quaternion(Some(pose.rotation)),
            unknown(),
            PropertyValue::Warning("Pose is available but the full transform is unresolved".into()),
        ),
        Ok(WorldTransformState::Unresolved(PoseState::Unresolved(reason))) => {
            (unknown(), unknown(), unknown(), PropertyValue::Warning(format!("Unresolved: {reason:?}")))
        }
        Ok(WorldTransformState::Unresolved(PoseState::Provisional { pose, reason })) => (
            dvector3(Some(pose.position)),
            quaternion(Some(pose.rotation)),
            unknown(),
            PropertyValue::Warning(format!("Provisional: {reason:?}")),
        ),
        Err(error) => (unknown(), unknown(), unknown(), PropertyValue::Warning(format!("Pose error: {error:?}"))),
    }
}

fn parent_value(parent: &str) -> PropertyValue {
    if parent == "Unknown" {
        PropertyValue::Unknown
    } else if parent == "Root" {
        PropertyValue::Text(parent.into())
    } else if parent.starts_with("Pending") || parent.starts_with("Invalid") || parent.starts_with("Resolved object") {
        PropertyValue::Warning(parent.into())
    } else {
        PropertyValue::Id(parent.into())
    }
}

fn id_value(value: Option<UUID>) -> PropertyValue {
    value.map_or(PropertyValue::Unknown, |id| PropertyValue::Id(id.to_string()))
}

fn vector3(value: Option<Vec3>) -> PropertyValue {
    value.map_or(PropertyValue::Unknown, |value| {
        PropertyValue::Vector(vec![format_component(value.x), format_component(value.y), format_component(value.z)])
    })
}

fn dvector3(value: Option<DVec3>) -> PropertyValue {
    value.map_or(PropertyValue::Unknown, |value| {
        PropertyValue::Vector(vec![format_component64(value.x), format_component64(value.y), format_component64(value.z)])
    })
}

fn quaternion(value: Option<Quat>) -> PropertyValue {
    value.map_or(PropertyValue::Unknown, |value| {
        PropertyValue::Vector(vec![
            format_component(value.x),
            format_component(value.y),
            format_component(value.z),
            format_component(value.w),
        ])
    })
}

fn format_component(value: f32) -> String {
    let value = if value.abs() < 0.0005 { 0.0 } else { value };
    format!("{value:.3}").trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn format_component64(value: f64) -> String {
    let value = if value.abs() < 0.0005 { 0.0 } else { value };
    format!("{value:.3}").trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[derive(Clone, Default, PartialEq)]
struct WorldSnapshot {
    revision: Option<WorldRevision>,
    coverage: Option<CoverageSnapshot>,
    region_labels: Arc<[RegionLabel]>,
    object_count: usize,
    objects: Arc<[ObjectRow]>,
    details: Option<ObjectDetails>,
    details_error: Option<String>,
}

impl WorldSnapshot {
    fn region_label(&self, region: Region) -> String {
        match self.region_labels.binary_search_by_key(&region, |row| row.region) {
            Ok(index) => self.region_labels[index].label.clone(),
            Err(_) => format!("Region {}", region.get()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct RegionLabel {
    region: Region,
    label: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ObjectSelection {
    entity: world::Entity,
    id: UUID,
}

#[derive(Clone, Debug, PartialEq)]
struct ObjectRow {
    selection: ObjectSelection,
    kind: Kind,
    region: Region,
    local_id: NonZeroU32,
}

#[derive(Clone, Debug, PartialEq)]
struct ObjectDetails {
    selection: ObjectSelection,
    kind: Kind,
    address: LocalAddress,
    parent: String,
    world_transform: Result<WorldTransformState, PoseError>,
    attachment_point: Option<world::AttachmentPoint>,
    sit_target: Option<world::SitTarget>,
    position: Option<Vec3>,
    rotation: Option<Quat>,
    scale: Option<Vec3>,
    velocity: Option<Vec3>,
    acceleration: Option<Vec3>,
    angular_velocity: Option<Vec3>,
    owner: Option<UUID>,
    creator: Option<UUID>,
}

#[derive(Clone, Debug, PartialEq)]
struct InspectorRow {
    key: String,
    selection: ObjectSelection,
    local_id: NonZeroU32,
    cells: Vec<String>,
}

fn object_row(item: ObjectListItem) -> ObjectRow {
    ObjectRow {
        selection: ObjectSelection { entity: item.entity, id: item.id },
        kind: item.kind,
        region: item.address.region(),
        local_id: item.address.local_id(),
    }
}

fn object_details(selection: ObjectSelection, detail: ObjectDetail) -> ObjectDetails {
    ObjectDetails {
        selection,
        kind: detail.summary.kind,
        address: detail.summary.address,
        parent: parent_label(selection.entity, detail.summary.parent, detail.parent_id),
        world_transform: detail.world_transform,
        attachment_point: detail.attachment_point,
        sit_target: detail.sit_target,
        position: detail.summary.local_transform.position(),
        rotation: detail.summary.local_transform.rotation(),
        scale: detail.summary.local_transform.scale(),
        velocity: detail.linear_motion.velocity,
        acceleration: detail.linear_motion.acceleration,
        angular_velocity: detail.angular_velocity,
        owner: detail.properties.owner_id,
        creator: detail.properties.creator_id,
    }
}

fn parent_label(child: world::Entity, parent: Parent, parent_id: Option<UUID>) -> String {
    match parent {
        Parent::Unknown => "Unknown".into(),
        Parent::Root => "Root".into(),
        Parent::Pending { address, expected_uuid } => {
            let expected = expected_uuid.map_or_else(|| "unknown".into(), |id| id.to_string());
            format!("Pending {}:{} (expected {expected})", address.region().get(), address.local_id())
        }
        Parent::Resolved(parent) if parent == child => "Invalid self-parent".into(),
        Parent::Resolved(_) => parent_id.map_or_else(|| "Resolved object is unavailable".into(), |id| id.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        num::NonZeroU32,
        sync::Arc,
        time::{Duration, Instant},
    };

    use base::UUID;
    use gpui::prelude::{ParentElement as _, Styled as _};
    use gpui::{AppContext as _, Context, Entity, IntoElement, Modifiers, Render, TestAppContext, Window, div, point, px};
    use gpui_component::{
        Sizable as _,
        table::{ColumnSort, DataTable, TableState},
    };
    use world::{
        ApplyOutcome::{Introduced, RegionAdopted},
        Kind, LocalAddress, ObjectObservation,
        OrderEvidence::Ordered,
        ParentUpdate::Root,
        PoseState, Region, UnresolvedReason, UpdateSource, World, WorldLimits, WorldTransformState,
    };

    use math::{Quat, Vec3};

    use super::{
        DeveloperFloater, HISTORY_LEN, InspectorTable, KindFilter, LOCAL_ID_CELL_PADDING, LOCAL_ID_GLYPH_WIDTH, Metrics, ObjectDetails,
        ObjectFilter, ObjectRow, ObjectSelection, Palette, PropertyValue, RefreshRequest, RegionLabel, WorldSnapshot, WorldSource,
        inspector_state, local_id_column_width, object_table, object_table_rows, parent_value, property_groups, should_refresh_world,
        table_body, table_inner_corner_radius,
    };

    struct InspectorHarness {
        objects: Entity<TableState<InspectorTable>>,
    }

    impl Render for InspectorHarness {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let p = Palette::new(cx);
            div().size_full().flex().flex_col().child(div().flex_1().min_h_0().flex().gap_2().p_2().child(super::panel(&p).flex_1().child(
                table_body("developer-objects", DataTable::new(&self.objects).small().bordered(false).scrollbar_visible(true, true), &p),
            )))
        }
    }

    #[gpui::test]
    fn object_table_renders_rows(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let snapshot = sample_snapshot();
            let objects = cx.new(|cx| TableState::new(object_table(&snapshot, &ObjectFilter::default()), window, cx).col_selectable(false));
            InspectorHarness { objects }
        });
        let rows = cx.update(|_, app| harness.read(app).objects.read(app).dump(app).1);
        assert_eq!(rows[0], ["00000000-0000-0000-0000-000000000007", "Avatar", "Mauve", "41"]);

        cx.update(|window, app| _ = window.draw(app));

        let cell = cx.debug_bounds("inspector-cell-0-0").expect("object UUID cell should render");
        assert!(cell.size.width > px(0.) && cell.size.height > px(0.));

        let header = cx.debug_bounds("inspector-header-0").expect("sortable UUID header should render");
        assert!(header.size.width > px(0.) && header.size.width < px(80.));

        let body = cx.debug_bounds("table-body-developer-objects").expect("object table body should render");
        assert!(body.size.width > px(0.) && body.size.height > px(0.));
    }

    #[test]
    fn pending_mirror_work_refreshes_until_paused() {
        assert!(!should_refresh_world(true, false, false, false, true));
        assert!(should_refresh_world(true, true, false, false, true));
        assert!(should_refresh_world(false, false, false, false, true));
        assert!(!should_refresh_world(false, false, true, false, true));
    }

    #[test]
    fn stale_refresh_results_are_rejected() {
        let snapshot = sample_snapshot();
        let selected = snapshot.objects[0].selection;
        let request = RefreshRequest { selected: Some(selected), paused: false, forced: false, generation: 4 };

        assert!(request.is_current(false, Some(selected), 4));
        assert!(!request.is_current(true, Some(selected), 4));
        assert!(!request.is_current(false, None, 4));
        assert!(!request.is_current(false, Some(selected), 5));

        let forced = RefreshRequest { paused: true, forced: true, ..request };
        assert!(forced.is_current(true, Some(selected), 4));
    }

    #[test]
    fn inner_radius_tracks_panel() {
        assert_eq!(table_inner_corner_radius(px(12.)), px(11.));
        assert_eq!(table_inner_corner_radius(px(8.)), px(7.));
        assert_eq!(table_inner_corner_radius(px(1.)), px(0.));
    }

    #[gpui::test]
    fn sort_button_is_reachable_in_the_header(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let snapshot = sample_snapshot();
            let objects = cx.new(|cx| TableState::new(object_table(&snapshot, &ObjectFilter::default()), window, cx).col_selectable(false));
            InspectorHarness { objects }
        });
        cx.update(|window, app| _ = window.draw(app));

        let body = cx.debug_bounds("table-body-developer-objects").expect("object table body should render");
        let header = cx.debug_bounds("inspector-header-0").expect("UUID header should render");
        let sort_button = point(body.origin.x + px(266.), header.origin.y + header.size.height / 2.);
        cx.simulate_click(sort_button, Modifiers::default());

        let sort = cx.update(|_, app| harness.read(app).objects.read(app).delegate().sort);
        assert_eq!(sort, Some((0, ColumnSort::Descending)));
    }

    #[gpui::test]
    fn kind_filter_button_updates_filter(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let source = WorldSource::new();
        let (floater, cx) = cx.add_window_view(|window, cx| DeveloperFloater::new(source, window, cx));
        cx.update(|window, app| _ = window.draw(app));

        let control = cx.debug_bounds("developer-kind-filter-control").expect("kind filter should render");
        cx.simulate_click(
            point(control.origin.x + control.size.width * 0.45, control.origin.y + control.size.height / 2.),
            Modifiers::default(),
        );

        let selected = cx.update(|_, app| floater.read(app).world_inspector.filter.kind);
        assert_eq!(selected, KindFilter::Avatars);
    }

    #[gpui::test]
    fn activity_panel_remains_without_regions_or_load_more(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let source = WorldSource::new();
        let (_, cx) = cx.add_window_view(|window, cx| DeveloperFloater::new(source, window, cx));
        cx.update(|window, app| _ = window.draw(app));

        assert!(cx.debug_bounds("table-body-developer-objects").is_some());
        assert!(cx.debug_bounds("table-body-developer-regions").is_none());
        assert!(cx.debug_bounds("developer-query-more").is_none());
        assert!(cx.debug_bounds("developer-bottom-panel").is_some());
        assert!(cx.debug_bounds("developer-bottom-tab-control").is_some());
    }

    #[gpui::test]
    fn inspector_values_stay_inside_panel(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let source = WorldSource::new();
        let (floater, cx) = cx.add_window_view(|window, cx| DeveloperFloater::new(source, window, cx));
        let mut snapshot = sample_snapshot();
        let object = snapshot.objects[0].clone();
        let details = ObjectDetails {
            selection: object.selection,
            kind: object.kind,
            address: LocalAddress::new(object.region, object.local_id),
            parent: UUID::from_u128(0x8b4a_9dcd_2d30_0961_3b2d_8f64_1101_cabc).to_string(),
            world_transform: Ok(WorldTransformState::Unresolved(PoseState::Unresolved(UnresolvedReason::MissingRegionOrigin))),
            attachment_point: None,
            sit_target: None,
            position: None,
            rotation: None,
            scale: None,
            velocity: None,
            acceleration: None,
            angular_velocity: None,
            owner: None,
            creator: None,
        };
        snapshot.details = Some(details);
        floater.update(cx, |view, cx| {
            view.snapshot = snapshot;
            view.world_inspector.selected_object = Some(object.selection);
            view.world_inspector.inspector = inspector_state(&view.snapshot, Some(object.selection));
            cx.notify();
        });
        cx.update(|window, app| _ = window.draw(app));

        let panel = cx.debug_bounds("developer-inspector-panel").expect("inspector panel should render");
        for selector in ["inspector-property-value-1", "inspector-property-value-5"] {
            let value = cx.debug_bounds(selector).expect("identifier should render in inspector");
            assert!(value.origin.x >= panel.origin.x);
            assert!(value.origin.x + value.size.width <= panel.origin.x + panel.size.width);
            assert!(value.size.width >= px(200.), "identifier should get the Inspector's available width");
        }
    }

    #[test]
    fn activity_samples_once_per_second() {
        let start = Instant::now();
        let snapshot = sample_snapshot();
        let mut metrics = Metrics::default();

        metrics.record(&snapshot, Duration::from_millis(1), start);
        metrics.record(&snapshot, Duration::from_millis(3), start + Duration::from_secs(1));
        assert!(metrics.chart.is_empty());

        metrics.record(&snapshot, Duration::from_millis(5), start + Duration::from_secs(2));
        assert_eq!(metrics.chart.len(), 1);
        assert_eq!(metrics.chart[0].capture_ms, 3.0);
        metrics.record(&snapshot, Duration::from_millis(7), start + Duration::from_millis(2500));
        assert_eq!(metrics.chart.len(), 1);

        metrics.record(&snapshot, Duration::from_millis(9), start + Duration::from_secs(3));
        assert_eq!(metrics.chart.len(), 2);
        assert_eq!(metrics.chart[1].capture_ms, 8.0);
    }

    #[test]
    fn activity_history_is_bounded() {
        let mut metrics = Metrics::default();
        let snapshot = sample_snapshot();
        let start = Instant::now();

        for second in 0..(HISTORY_LEN + 3) {
            metrics.record(&snapshot, Duration::from_micros(250), start + Duration::from_secs(second as u64));
        }

        assert_eq!(metrics.chart.len(), HISTORY_LEN);
        assert_eq!(metrics.chart.first().map(|point| point.elapsed.as_ref()), Some("0:03"));
        assert_eq!(metrics.chart.last().map(|point| point.elapsed.as_ref()), Some("2:02"));
        assert_eq!(metrics.peak_capture_ms(), Some(0.25));
    }

    const SNAPSHOT_OBJECTS: usize = 300;

    fn limits() -> WorldLimits {
        WorldLimits {
            max_region_registrations: 1,
            max_entities: SNAPSHOT_OBJECTS,
            max_entities_per_region: SNAPSHOT_OBJECTS,
            max_tombstones_per_region: 1,
            max_tombstones: 1,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(1),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(1),
            max_journal_entries: SNAPSHOT_OBJECTS + 2,
            max_journal_changes: SNAPSHOT_OBJECTS + 2,
            max_region_name_bytes: 32,
            max_recovery_targets: 1,
        }
    }

    fn sample_snapshot() -> WorldSnapshot {
        WorldSnapshot {
            region_labels: Arc::from(vec![
                RegionLabel { region: Region::new(1), label: "Mauve".into() },
                RegionLabel { region: Region::new(2), label: "Ochre".into() },
            ]),
            object_count: 3,
            objects: Arc::from(vec![
                object_row(7, Kind::Avatar, Region::new(1), 41),
                object_row(8, Kind::Object, Region::new(1), 42),
                object_row(9, Kind::Object, Region::new(2), 43),
            ]),
            ..WorldSnapshot::default()
        }
    }

    fn object_row(id: u128, kind: Kind, region: Region, local_id: u32) -> ObjectRow {
        let id = UUID::from_u128(id);
        let local_id = NonZeroU32::new(local_id).expect("nonzero local ID");
        let mut world = World::new(limits());
        assert_eq!(world.adopt_region(region).expect("adopt row region").outcome, RegionAdopted);
        assert!(matches!(
            world
                .observe_object(ObjectObservation {
                    source: UpdateSource::new(region, 1, Ordered(1), 0),
                    id,
                    local_id,
                    kind,
                    parent: Root,
                    pose: None,
                    linear_motion: None,
                    angular_velocity: None,
                    scale: None,
                    attachment_point: None,
                    shape: None,
                    appearance: None,
                    surface_appearance: None,
                    render_materials: None,
                })
                .expect("introduce row object")
                .outcome,
            Introduced(_)
        ));
        ObjectRow { selection: ObjectSelection { entity: world.entity(id).expect("row entity"), id }, kind, region, local_id }
    }

    #[test]
    fn object_rows_separate_identity_columns() {
        let snapshot = sample_snapshot();

        let rows = object_table_rows(&snapshot, &ObjectFilter::default());

        assert_eq!(rows[0].cells, ["00000000-0000-0000-0000-000000000007", "Avatar", "Mauve", "41"]);
        assert_eq!(rows[0].selection.id, UUID::from_u128(7));
    }

    #[test]
    fn object_filters_combine_kind_and_text() {
        let snapshot = sample_snapshot();
        let ids = |filter: &ObjectFilter| object_table_rows(&snapshot, filter).iter().map(|row| row.selection.id).collect::<Vec<_>>();

        assert_eq!(ids(&ObjectFilter::default()).len(), 3);
        assert_eq!(ids(&ObjectFilter { kind: KindFilter::Avatars, ..ObjectFilter::default() }), [UUID::from_u128(7)]);
        assert_eq!(ids(&ObjectFilter { text: "ochre".into(), ..ObjectFilter::default() }), [UUID::from_u128(9)]);
        assert!(ids(&ObjectFilter { kind: KindFilter::Avatars, text: "ochre".into() }).is_empty());
    }

    #[test]
    fn local_id_sort_survives_refresh() {
        let region = Region::new(1);
        let snapshot = WorldSnapshot {
            region_labels: Arc::from(vec![RegionLabel { region, label: "Mauve".into() }]),
            object_count: 3,
            objects: Arc::from(vec![
                object_row(1, Kind::Object, region, 10),
                object_row(2, Kind::Object, region, 2),
                object_row(3, Kind::Object, region, 1_000_000_000),
            ]),
            ..WorldSnapshot::default()
        };
        let mut table = object_table(&snapshot, &ObjectFilter::default());
        assert!(table.columns.iter().all(|column| column.sort == Some(ColumnSort::Default)));
        table.selected = Some(snapshot.objects[1].selection);
        assert!(!table.columns[3].resizable);
        assert_eq!(table.columns[3].width, px(local_id_column_width(&table.rows)));
        assert_eq!(table.columns[3].width, px(10. * LOCAL_ID_GLYPH_WIDTH + LOCAL_ID_CELL_PADDING));

        table.sort_rows(3, ColumnSort::Ascending);
        assert_eq!(
            table.rows.iter().map(|row| row.selection.id).collect::<Vec<_>>(),
            [UUID::from_u128(2), UUID::from_u128(1), UUID::from_u128(3)]
        );
        assert_eq!(table.row_for_object(table.selected), Some(0));

        table.rows = Arc::from(object_table_rows(&snapshot, &ObjectFilter::default()));
        table.apply_sort();
        assert_eq!(table.rows[0].local_id.get(), 2);

        table.sort_rows(3, ColumnSort::Descending);
        assert_eq!(
            table.rows.iter().map(|row| row.selection.id).collect::<Vec<_>>(),
            [UUID::from_u128(3), UUID::from_u128(1), UUID::from_u128(2)]
        );
    }

    #[test]
    fn optional_details_omit_debug_wrappers() {
        let details = ObjectDetails {
            selection: object_row(9, Kind::Object, Region::new(3), 8).selection,
            kind: Kind::Object,
            address: LocalAddress::new(Region::new(3), NonZeroU32::new(8).unwrap()),
            parent: "Root".into(),
            world_transform: Ok(WorldTransformState::Unresolved(PoseState::Unresolved(UnresolvedReason::MissingRegionOrigin))),
            attachment_point: None,
            sit_target: None,
            position: Some(Vec3::new(1.25, -2.0, 3.5)),
            rotation: Some(Quat::from_xyzw(0.0, 0.5, 0.0, 1.0)),
            scale: None,
            velocity: Some(Vec3::ZERO),
            acceleration: None,
            angular_velocity: None,
            owner: None,
            creator: Some(UUID::from_u128(10)),
        };
        let snapshot = WorldSnapshot { details: Some(details.clone()), ..WorldSnapshot::default() };

        let groups = property_groups(&snapshot, &details);
        let value = |label: &str| {
            groups.iter().flat_map(|group| &group.properties).find(|property| property.label == label).map(|property| property.value.text())
        };

        assert_eq!(value("Position").as_deref(), Some("1.25, -2, 3.5"));
        assert_eq!(value("Rotation").as_deref(), Some("0, 0.5, 0, 1"));
        assert_eq!(value("Scale").as_deref(), Some("Unknown"));
        assert_eq!(value("Owner").as_deref(), Some("Unknown"));
        assert_eq!(value("Creator").as_deref(), Some("00000000-0000-0000-0000-00000000000a"));
        assert!(groups.iter().flat_map(|group| &group.properties).all(|property| {
            let text = property.value.text();
            !text.contains("Some(") && !text.contains("None") && !text.contains("Vec3(") && !text.contains("Quat(")
        }));
    }

    #[test]
    fn parent_states_are_classified_for_display() {
        assert_eq!(parent_value("Unknown"), PropertyValue::Unknown);
        assert_eq!(parent_value("Root"), PropertyValue::Text("Root".into()));
        assert!(matches!(parent_value("Pending 3:8 (expected unknown)"), PropertyValue::Warning(_)));
        assert!(matches!(parent_value("Invalid self-parent"), PropertyValue::Warning(_)));
        assert!(matches!(parent_value("00000000-0000-0000-0000-000000000001"), PropertyValue::Id(_)));
    }

    #[test]
    fn world_source_starts_disconnected() {
        let source = WorldSource::new();
        assert!(source.is_disconnected());
        source.set(None);
        assert!(source.is_disconnected());
    }
}
