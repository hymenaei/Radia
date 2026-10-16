use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU32,
    sync::{Arc, PoisonError, RwLock, TryLockError},
    time::{Duration, Instant},
};

use base::UUID;
use gpui::{
    AnyElement, App, ClipboardItem, ColorExt as _, Context, Div, Entity, FontWeight, Hsla, IntoElement, Pixels, Render, SharedString,
    Stateful, Subscription, Window, div, prelude::*, px,
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
use math::{Quat, Vec3};
use tokio::sync::watch;
use world::{
    Kind, LocalAddress, MAX_OBJECT_PAGE_ITEMS, MAX_REGION_PAGE_ITEMS, MAX_WORLD_CHANGE_PAGE_ITEMS, ObjectDetail, ObjectDetailResult,
    ObjectListItem, ObjectPageCursor, Parent, Region, RegionPageCursor, RegionSummary, WorldChange, WorldChangeCursor, WorldReadError,
    WorldRevision,
};

use crate::session::{CoverageHealth, CoverageSnapshot, SessionReadBarrier, WorldReader, WorldReaderError};
#[cfg(target_os = "linux")]
use crate::window_chrome::linux_titlebar;
#[cfg(target_os = "windows")]
use crate::window_chrome::windows_titlebar;
use crate::window_geometry::observe;

/// Cadence of a live capture. Pause stops this loop only; ingestion is untouched.
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
/// The poll loop wakes this often to notice pause, manual refresh and selection changes without waiting a full interval.
const POLL_TICK: Duration = Duration::from_millis(100);
/// Let the live view settle before plotting its first sample.
const CHART_WARMUP: Duration = Duration::from_secs(2);
/// Maximum cadence of samples added to the activity charts.
const CHART_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
/// Samples kept for the activity charts (about two minutes at the live cadence).
const HISTORY_LEN: usize = 120;
const COPIED_FEEDBACK: Duration = Duration::from_millis(1200);
const LOCAL_ID_GLYPH_WIDTH: f32 = 8.5;
const LOCAL_ID_CELL_PADDING: f32 = 16.;
const PANEL_BORDER_WIDTH: Pixels = px(1.);

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
    Live,
    ResyncFailed,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum KindFilter {
    #[default]
    All,
    Avatars,
    Objects,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum BottomTab {
    #[default]
    Activity,
    Diagnostics,
}

/// Evaluated when rows are rebuilt, never in `render`.
#[derive(Clone, Default)]
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
    snapshot: WorldSnapshot,
    selected: Option<ObjectSelection>,
    paused: bool,
    /// One-shot request for a capture, honoured even while paused (manual refresh, selection change).
    refresh_requested: bool,
    connection: Connection,
    bottom_open: bool,
    bottom_tab: BottomTab,
    filter: ObjectFilter,
    filter_input: Entity<InputState>,
    inspector: InspectorState,
    metrics: Metrics,
    copied: Option<&'static str>,
    objects: Entity<TableState<InspectorTable>>,
    _geometry_subscription: Subscription,
    _object_table_subscription: Subscription,
    _filter_subscription: Subscription,
}

impl DeveloperFloater {
    pub(crate) fn new(source: WorldSource, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let snapshot = WorldSnapshot::default();
        let filter = ObjectFilter::default();

        let objects = cx.new(|cx| TableState::new(object_table(&snapshot, &filter), window, cx).col_selectable(false).col_movable(false));
        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter by UUID, kind, region or local ID"));

        let object_table_subscription = cx.subscribe(&objects, |view, table, event: &TableEvent, cx| match event {
            TableEvent::SelectRow(row_ix) => {
                if view.paused {
                    let selected_row = table.read(cx).delegate().row_for_object(view.selected);
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
            let mut last_refresh = Instant::now().checked_sub(REFRESH_INTERVAL).unwrap_or_else(Instant::now);
            let mut mirror = WorldMirror::default();
            let mut revision_watch: Option<(WorldReader, watch::Receiver<WorldRevision>)> = None;
            let mut coverage_watch: Option<(WorldReader, watch::Receiver<u64>)> = None;
            let mut resync_failed = false;
            loop {
                let Ok((mut selected, paused, forced)) = this.read_with(cx, |view, _| (view.selected, view.paused, view.refresh_requested))
                else {
                    break;
                };

                let source_changed = refresh_source
                    .try_reader()
                    .flatten()
                    .is_some_and(|reader| mirror.reader.as_ref().is_some_and(|current| !current.same_session(&reader)));
                if source_changed {
                    let previous_selection = selected;
                    selected = None;
                    mirror.reset();
                    coverage_watch = None;
                    resync_failed = false;
                    let _ = this.update(cx, |view, cx| {
                        if view.selected == previous_selection {
                            view.clear_selection(cx);
                        }
                    });
                    last_refresh = Instant::now().checked_sub(REFRESH_INTERVAL).unwrap_or_else(Instant::now);
                }

                if forced && resync_failed {
                    mirror.reset();
                    resync_failed = false;
                }

                let coverage_changed = coverage_watch.as_mut().is_some_and(|(_, changed)| {
                    if changed.has_changed().unwrap_or(false) {
                        let _ = changed.borrow_and_update();
                        true
                    } else {
                        false
                    }
                });
                if paused
                    && coverage_changed
                    && !forced
                    && let Some(world) = refresh_source.try_reader().flatten()
                {
                    let source = refresh_source.clone();
                    let result = background
                        .spawn(async move {
                            let barrier = world.barrier().await;
                            (world, barrier)
                        })
                        .await;
                    if let (world, Ok(barrier)) = result {
                        let _ = this.update(cx, move |view, cx| {
                            if view.paused && source.is_current(&world) {
                                view.snapshot.coverage = Some(barrier.coverage);
                                cx.notify();
                            }
                        });
                    }
                }

                let due = forced || (!paused && !resync_failed && (last_refresh.elapsed() >= REFRESH_INTERVAL || coverage_changed));
                if due {
                    if forced {
                        let _ = this.update(cx, |view, _| view.refresh_requested = false);
                    }
                    last_refresh = Instant::now();
                    let source = refresh_source.clone();
                    let current_mirror = std::mem::take(&mut mirror);
                    let current_watch = revision_watch.take();
                    let current_coverage_watch = coverage_watch.take();
                    let (refresh, next_mirror, next_watch, next_coverage_watch) = background
                        .spawn(async move {
                            let started = Instant::now();
                            let Some(world) = source.try_reader() else {
                                return (Refresh::Busy, current_mirror, current_watch, current_coverage_watch);
                            };
                            let Some(world) = world else {
                                return (Refresh::Disconnected, current_mirror, current_watch, current_coverage_watch);
                            };
                            let mut revision_watch = match current_watch {
                                Some((current, revisions)) if current.same_session(&world) => revisions,
                                _ => world.subscribe_revisions(),
                            };
                            let mut coverage_watch = match current_coverage_watch {
                                Some((current, coverage)) if current.same_session(&world) => coverage,
                                _ => world.subscribe_coverage(),
                            };
                            let _ = coverage_watch.borrow_and_update();
                            let revision_hint = *revision_watch.borrow_and_update();
                            let mut mirror = current_mirror;
                            let refresh = match capture_snapshot(&world, &mut mirror, selected, revision_hint).await {
                                Ok(snapshot) => Refresh::Captured(world.clone(), snapshot, started.elapsed()),
                                Err(WorldReaderError::Busy) => Refresh::Busy,
                                Err(WorldReaderError::Gone) => Refresh::Disconnected,
                                Err(WorldReaderError::Read(
                                    WorldReadError::ResyncRequired | WorldReadError::InvalidCursor | WorldReadError::StaleSession,
                                )) => refresh_after_resync(&mut mirror),
                                Err(error) => {
                                    tracing::warn!(?error, "could not refresh Developer Tools World view");
                                    Refresh::Failed
                                }
                            };
                            (refresh, mirror, Some((world.clone(), revision_watch)), Some((world, coverage_watch)))
                        })
                        .await;
                    mirror = next_mirror;
                    revision_watch = next_watch;
                    coverage_watch = next_coverage_watch;

                    if mirror.selection_invalidated {
                        mirror.selection_invalidated = false;
                        let previous_selection = selected;
                        selected = None;
                        let _ = this.update(cx, |view, cx| {
                            if view.selected == previous_selection {
                                view.clear_selection(cx);
                            }
                        });
                    }

                    match refresh {
                        Refresh::Busy => {
                            if forced {
                                let _ = this.update(cx, |view, _| view.refresh_requested = true);
                            }
                            last_refresh = Instant::now().checked_sub(REFRESH_INTERVAL).unwrap_or_else(Instant::now);
                        }
                        Refresh::Resync => {
                            let _ = this.update(cx, |view, cx| {
                                view.update_snapshot(WorldSnapshot::default(), None, cx);
                                view.refresh_requested = true;
                            });
                            last_refresh = Instant::now().checked_sub(REFRESH_INTERVAL).unwrap_or_else(Instant::now);
                        }
                        Refresh::ResyncFailed => {
                            resync_failed = true;
                            let _ = this.update(cx, |view, cx| {
                                view.connection = Connection::ResyncFailed;
                                view.refresh_requested = false;
                                view.update_snapshot(WorldSnapshot::default(), None, cx);
                            });
                        }
                        Refresh::Failed => {}
                        Refresh::Disconnected => {
                            let source = refresh_source.clone();
                            let _ = this.update(cx, move |view, cx| {
                                if (!view.paused || forced) && source.is_disconnected() {
                                    view.connection = Connection::Offline;
                                    view.selected = None;
                                    view.update_snapshot(WorldSnapshot::default(), None, cx);
                                }
                            });
                        }
                        Refresh::Captured(world, snapshot, took) => {
                            let source = refresh_source.clone();
                            let _ = this.update(cx, move |view, cx| {
                                if (!view.paused || forced) && view.selected == selected && source.is_current(&world) {
                                    view.connection = Connection::Live;
                                    view.update_snapshot(snapshot, Some(took), cx);
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
            snapshot,
            selected: None,
            paused: false,
            refresh_requested: false,
            connection: Connection::Offline,
            bottom_open: true,
            bottom_tab: BottomTab::Activity,
            filter,
            filter_input,
            inspector: InspectorState::Empty,
            metrics: Metrics::default(),
            copied: None,
            objects,
            _geometry_subscription: geometry_subscription,
            _object_table_subscription: object_table_subscription,
            _filter_subscription: filter_subscription,
        }
    }

    fn request_refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh_requested = true;
        cx.notify();
    }

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        self.paused = !self.paused;
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
        if !self.paused && self.selected != Some(selection) {
            self.selected = Some(selection);
            self.inspector = inspector_state(&self.snapshot, self.selected);
            let selected = self.selected;
            self.objects.update(cx, |table, _| table.delegate_mut().selected = selected);
            self.refresh_requested = true;
            cx.notify();
        }
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.inspector = inspector_state(&self.snapshot, None);
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
        if self.filter.text != text {
            self.filter.text = text;
            self.refilter(cx);
        }
    }

    fn set_kind_filter(&mut self, index: usize, cx: &mut Context<Self>) {
        let kind = match index {
            1 => KindFilter::Avatars,
            2 => KindFilter::Objects,
            _ => KindFilter::All,
        };
        if self.filter.kind != kind {
            self.filter.kind = kind;
            self.refilter(cx);
        }
    }

    fn copy(&mut self, label: &'static str, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(label);
        cx.notify();

        let background = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            background.timer(COPIED_FEEDBACK).await;
            let _ = this.update(cx, |view, cx| {
                if view.copied == Some(label) {
                    view.copied = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn object_empty_message(&self) -> String {
        if self.connection == Connection::ResyncFailed {
            "World changes outpaced the inspector. Select Refresh to try again.".into()
        } else if self.connection == Connection::Offline {
            "Not connected to a session.".into()
        } else if self.filter.is_active() {
            "No objects match the current filter.".into()
        } else {
            "No objects observed.".into()
        }
    }

    /// Rebuild the object rows from the current snapshot and filter, keeping the highlighted row on the selected object.
    fn refilter(&mut self, cx: &mut Context<Self>) {
        let rows = object_table_rows(&self.snapshot, &self.filter);
        let empty_message = self.object_empty_message();
        let selected = self.selected;
        self.objects.update(cx, |table, cx| {
            let width_changed = {
                let delegate = table.delegate_mut();
                delegate.rows = rows;
                delegate.selected = selected;
                delegate.apply_sort();
                delegate.empty_message = empty_message;
                delegate.update_local_id_width()
            };
            if width_changed {
                table.refresh(cx);
            }
            let selected_row = table.delegate().row_for_object(selected);
            sync_selection(table, selected_row, cx);
            cx.notify();
        });
        cx.notify();
    }

    fn update_snapshot(&mut self, snapshot: WorldSnapshot, capture: Option<Duration>, cx: &mut Context<Self>) {
        let table_changed =
            !Arc::ptr_eq(&self.snapshot.regions, &snapshot.regions) || !Arc::ptr_eq(&self.snapshot.objects, &snapshot.objects);
        self.snapshot = snapshot;
        if let (Some(took), false) = (capture, self.paused) {
            self.metrics.record(&self.snapshot, took, Instant::now());
        }

        self.inspector = inspector_state(&self.snapshot, self.selected);
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
        let sample_elapsed = now.saturating_duration_since(self.last_sample_at.expect("chart sampling starts on first capture"));
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
    /// Row hover.
    hover: Hsla,
    hairline: Hsla,
    /// Active segment (the theme's `primary` is a neutral raised gray).
    selected: Hsla,
    panel_radius: Pixels,
    input: Hsla,
    warning: Hsla,
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
            hover: colors.accent,
            hairline: colors.border,
            selected: colors.primary,
            panel_radius: theme.radius_tokens().xl,
            input: colors.input,
            warning: colors.warning,
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
            let theme = Theme::global(cx).colors;
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
        let kind_tabs = segmented(
            "developer-kind-filter",
            vec!["All".into(), "Avatars".into(), "Objects".into()],
            match self.filter.kind {
                KindFilter::All => 0,
                KindFilter::Avatars => 1,
                KindFilter::Objects => 2,
            },
            p,
            cx,
            |view, index, cx| view.set_kind_filter(index, cx),
        );
        let objects = panel(p).flex_1().child(panel_header("Objects", p).child(kind_tabs)).child(table_body(
            "developer-objects",
            DataTable::new(&self.objects).small().bordered(false).scrollbar_visible(true, true),
            p,
        ));

        let inspector = self.render_inspector(p, cx);
        div().flex_1().min_h_0().flex().gap_2().px_3().pb_2().child(objects).child(inspector)
    }

    fn render_inspector(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let mut header = panel_header("Inspector", p);
        if let Some(label) = self.copied {
            header = header.child(neutral_chip(format!("Copied {label}"), p));
        }

        let body = match &self.inspector {
            InspectorState::Empty => {
                empty_state("Select an object", "Pick a row to inspect its identity, hierarchy, transform and ownership.", p)
            }
            InspectorState::Updating => empty_state("Updating details…", "Fetching the latest state for this object.", p),
            InspectorState::Removed => empty_state("Object is no longer available", "This selection belongs to a removed World entity.", p),
            InspectorState::Ready(groups) => {
                let mut next_id = 0usize;
                let mut sections = Vec::with_capacity(groups.len());
                for group in groups {
                    let mut rows = Vec::with_capacity(group.properties.len());
                    for property in &group.properties {
                        next_id += 1;
                        rows.push(property_row(next_id, property, p, cx));
                    }
                    sections.push(
                        v_flex()
                            .gap_0p5()
                            .child(div().px_2().pt_3().pb_1().text_xs().font_semibold().text_color(p.muted).child(group.title))
                            .children(rows),
                    );
                }
                div()
                    .id("developer-inspector")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_1()
                    .pb_2()
                    .child(v_flex().children(sections))
                    .into_any_element()
            }
        };

        panel(p).w(px(340.)).flex_shrink_0().bg(p.surface).child(header).child(body)
    }

    fn render_bottom(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
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

        let mut bottom = panel(p).h(px(if self.bottom_open { 214. } else { 38. })).flex_shrink_0().mx_3().mb_3().child(header);
        if self.bottom_open {
            let body = match self.bottom_tab {
                BottomTab::Activity => self.render_activity(p),
                BottomTab::Diagnostics => self.render_diagnostics(p),
            };
            bottom = bottom.child(body);
        }
        bottom
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
                format!(
                    "{health} at revision {}; {} pending; {} degraded",
                    coverage.world_revision.get(),
                    coverage.recovery.pending + coverage.recovery.in_flight,
                    coverage.recovery.degraded
                )
            },
        );
        let selected = self.selected.map_or_else(|| "None".to_owned(), |selection| selection.id.to_string());
        let capture = self.metrics.latest().map_or_else(|| "—".to_owned(), |point| format!("{:.2} ms", point.capture_ms));
        let peak_capture = self.metrics.peak_capture_ms().map_or_else(|| "—".to_owned(), |peak| format!("{peak:.2} ms"));
        let status = match self.connection {
            Connection::Live => "Connected",
            Connection::Offline => "Disconnected",
            Connection::ResyncFailed => "Refresh required",
        };
        let capture_state = if self.paused { "Paused" } else { "Updating" };
        let buffered = format!("{} / {} captures", self.metrics.chart.len(), HISTORY_LEN);
        let diagnostics = [
            ("Connection", status.to_owned()),
            ("Capture", capture_state.to_owned()),
            ("Revision", revision),
            ("Coverage", coverage),
            ("Regions", self.snapshot.regions.len().to_string()),
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

fn chip(text: impl Into<SharedString>, tone: Hsla) -> Div {
    div()
        .h(px(18.))
        .flex()
        .items_center()
        .px(px(6.))
        .rounded_md()
        .bg(tone.opacity(0.12))
        .text_xs()
        .whitespace_nowrap()
        .text_color(tone)
        .child(text.into())
}

fn neutral_chip(text: impl Into<SharedString>, p: &Palette) -> Div {
    div()
        .h(px(18.))
        .flex()
        .items_center()
        .px(px(6.))
        .rounded_md()
        .bg(p.background)
        .text_xs()
        .whitespace_nowrap()
        .text_color(p.foreground)
        .child(text.into())
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
    let mut control =
        div().id(id).flex().items_center().gap(px(2.)).p(px(2.)).rounded_lg().bg(p.raised).border_1().border_color(p.hairline);
    for (index, label) in labels.into_iter().enumerate() {
        let active = index == selected;
        control = control.child(
            div()
                .id((id, index))
                .h(px(22.))
                .flex()
                .items_center()
                .px_2()
                .rounded(px(6.))
                .text_xs()
                .whitespace_nowrap()
                .cursor_pointer()
                .when(active, |item| item.bg(selected_bg).text_color(foreground).shadow_sm())
                .when(!active, |item| item.text_color(muted).hover(move |style| style.text_color(foreground)))
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

fn empty_state(title: &'static str, hint: &'static str, p: &Palette) -> AnyElement {
    v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_1()
        .px_6()
        .child(div().text_sm().font_semibold().child(title))
        .child(div().text_xs().text_color(p.muted).text_center().child(hint))
        .into_any_element()
}

/// Linear-style property row. Identifier values copy on click.
fn property_row(index: usize, property: &Property, p: &Palette, cx: &mut Context<DeveloperFloater>) -> Stateful<Div> {
    let copyable = matches!(property.value, PropertyValue::Id(_));
    let value = match &property.value {
        PropertyValue::Text(text) => div().min_w_0().truncate().child(text.clone()),
        PropertyValue::Id(text) => div().min_w_0().truncate().font_family(p.mono.clone()).text_sm().child(text.clone()),
        PropertyValue::Vector(components) => vector_chips(components, p),
        PropertyValue::Warning(text) => div().min_w_0().child(chip(text.clone(), p.warning)),
        PropertyValue::Unknown => div().italic().text_color(p.muted.opacity(0.8)).child("Unknown"),
    };
    let text = property.value.text();
    let label = property.label;
    let (hover, foreground) = (p.hover, p.foreground);
    h_flex()
        .id(("property", index))
        .min_h(px(28.))
        .items_start()
        .gap_3()
        .px_2()
        .py(px(4.))
        .rounded_md()
        .child(div().w(px(92.)).flex_shrink_0().text_xs().text_color(p.muted).pt(px(2.)).child(label))
        .child(div().flex_1().min_w_0().child(value))
        .when(copyable, move |row| {
            row.cursor_pointer()
                .hover(move |style| style.bg(hover).text_color(foreground))
                .on_click(cx.listener(move |view, _, _, cx| view.copy(label, text.clone(), cx)))
        })
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
    rows: Vec<InspectorRow>,
    selected: Option<ObjectSelection>,
    sort: Option<(usize, ColumnSort)>,
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

    fn sort_rows(&mut self, column: usize, sort: ColumnSort) {
        self.sort = Some((column, sort));
        self.apply_sort();
    }

    fn apply_sort(&mut self) {
        let Some((column, sort)) = self.sort else {
            return;
        };
        self.rows.sort_by(|left, right| {
            let ordering = match column {
                0 => left.selection.id.cmp(&right.selection.id),
                1 | 2 => left.cells[column].cmp(&right.cells[column]),
                3 => left.local_id.cmp(&right.local_id),
                _ => Ordering::Equal,
            };
            let ordering = if sort == ColumnSort::Descending { ordering.reverse() } else { ordering };
            ordering.then_with(|| left.selection.id.cmp(&right.selection.id))
        });
    }

    fn update_local_id_width(&mut self) -> bool {
        let width = px(local_id_column_width(&self.rows));
        let column = &mut self.columns[3];
        if column.width == width {
            return false;
        }
        column.width = width;
        true
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

    fn perform_sort(&mut self, col_ix: usize, sort: ColumnSort, window: &mut Window, cx: &mut Context<TableState<Self>>) {
        self.sort_rows(col_ix, sort);
        let selected = self.selected;
        cx.defer_in(window, move |table, _, cx| {
            let row = table.delegate().row_for_object(selected);
            sync_selection(table, row, cx);
        });
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
        rows,
        selected: None,
        sort: None,
        empty_message: "No objects observed.".into(),
    }
}

fn local_id_column_width(rows: &[InspectorRow]) -> f32 {
    let longest = rows.iter().map(|row| row.cells[3].len()).max().unwrap_or(0).max("Local ID".len());
    longest as f32 * LOCAL_ID_GLYPH_WIDTH + LOCAL_ID_CELL_PADDING
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

enum InspectorState {
    Empty,
    Updating,
    Removed,
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
    let Some(selection) = selected else { return InspectorState::Empty };
    let Some(details) = snapshot.details.as_ref().filter(|details| details.selection == selection) else {
        return if snapshot.objects.iter().any(|object| object.selection == selection) {
            InspectorState::Updating
        } else {
            InspectorState::Removed
        };
    };
    InspectorState::Ready(property_groups(snapshot, details))
}

fn property_groups(snapshot: &WorldSnapshot, details: &ObjectDetails) -> Vec<PropertyGroup> {
    let property = |label, value| Property { label, value };
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
        PropertyGroup { title: "Hierarchy", properties: vec![property("Parent", parent_value(&details.parent))] },
        PropertyGroup {
            title: "Transform",
            properties: vec![
                property("Position", vector3(details.position)),
                property("Rotation", quaternion(details.rotation)),
                property("Scale", vector3(details.scale)),
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

#[derive(Clone, Default, PartialEq)]
struct WorldSnapshot {
    revision: Option<WorldRevision>,
    coverage: Option<CoverageSnapshot>,
    regions: Arc<[RegionRow]>,
    object_count: usize,
    objects: Arc<[ObjectRow]>,
    details: Option<ObjectDetails>,
}

impl WorldSnapshot {
    fn region_label(&self, region: Region) -> String {
        match self.regions.binary_search_by_key(&region, |row| row.region) {
            Ok(index) => self.regions[index].label.clone(),
            Err(_) => format!("Region {}", region.get()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct RegionRow {
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

const MAX_BOOTSTRAP_PAGES_PER_SYNC: usize = 16;
const MAX_BOOTSTRAP_RESTARTS: usize = 3;
const MAX_CHANGE_PAGES_PER_SYNC: usize = 4;
const MAX_SUMMARY_HYDRATIONS_PER_SYNC: usize = 64;
const MAX_REGION_HYDRATIONS_PER_SYNC: usize = 32;

enum Refresh {
    Busy,
    Disconnected,
    Resync,
    ResyncFailed,
    Failed,
    Captured(WorldReader, WorldSnapshot, Duration),
}

#[derive(Default)]
struct WorldMirror {
    reader: Option<WorldReader>,
    selection_invalidated: bool,
    resync_restarts: usize,
    change_cursor: Option<WorldChangeCursor>,
    object_cursor: Option<ObjectPageCursor>,
    region_cursor: Option<RegionPageCursor>,
    objects_done: bool,
    regions_done: bool,
    bootstrapping: bool,
    has_more_changes: bool,
    revision: Option<WorldRevision>,
    journal_revision: Option<WorldRevision>,
    coverage: Option<CoverageSnapshot>,
    object_count: usize,
    objects: BTreeMap<UUID, ObjectRow>,
    regions: BTreeMap<Region, RegionSummary>,
    pending_objects: BTreeMap<UUID, world::Entity>,
    pending_regions: BTreeSet<Region>,
    selection: Option<ObjectSelection>,
    details: Option<ObjectDetails>,
    details_dirty: bool,
    snapshot: WorldSnapshot,
    snapshot_dirty: bool,
}

impl WorldMirror {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn restart_after_history_loss(&mut self) -> bool {
        if self.resync_restarts == MAX_BOOTSTRAP_RESTARTS {
            return false;
        }
        let reader = self.reader.clone();
        let selection_invalidated = self.selection_invalidated;
        let resync_restarts = self.resync_restarts + 1;
        *self = Self::default();
        self.reader = reader;
        self.selection_invalidated = selection_invalidated;
        self.resync_restarts = resync_restarts;
        true
    }

    fn begin(&mut self, reader: &WorldReader, barrier: SessionReadBarrier) {
        self.reader = Some(reader.clone());
        self.change_cursor = Some(barrier.changes);
        self.object_count = barrier.object_count;
        self.revision = Some(barrier.revision);
        self.journal_revision = Some(barrier.revision);
        self.coverage = Some(barrier.coverage);
        self.bootstrapping = true;
        self.details_dirty = true;
        self.snapshot_dirty = true;
    }

    fn is_current(&self, revision_hint: WorldRevision) -> bool {
        !self.bootstrapping
            && !self.has_more_changes
            && self.pending_objects.is_empty()
            && self.pending_regions.is_empty()
            && !self.details_dirty
            && self.journal_revision == Some(revision_hint)
    }

    fn apply_change(&mut self, change: WorldChange) {
        match change {
            WorldChange::EntityAdded { id, entity, .. } => {
                self.object_count += 1;
                self.pending_objects.insert(id, entity);
                if self.selection.is_some_and(|selection| selection.id == id && selection.entity == entity) {
                    self.details_dirty = true;
                }
            }
            WorldChange::EntityRebound { id, entity, .. } | WorldChange::EntityTransferred { id, entity, .. } => {
                self.pending_objects.insert(id, entity);
                if self.selection.is_some_and(|selection| selection.id == id && selection.entity == entity) {
                    self.details_dirty = true;
                }
            }
            WorldChange::EntityUpdated { id, entity, .. } => {
                if self.selection.is_some_and(|selection| selection.id == id && selection.entity == entity) {
                    self.details_dirty = true;
                }
            }
            WorldChange::EntityRemoved { id, entity, .. } => {
                self.object_count -= 1;
                if self.objects.get(&id).is_some_and(|row| row.selection.entity == entity) {
                    self.objects.remove(&id);
                    self.snapshot_dirty = true;
                }
                if self.pending_objects.get(&id).is_some_and(|pending| *pending == entity) {
                    self.pending_objects.remove(&id);
                }
                if self.selection.is_some_and(|selection| selection.id == id && selection.entity == entity) {
                    self.details_dirty = true;
                }
            }
            WorldChange::SubtreeInvalidated { .. } => {
                self.details_dirty |= self.selection.is_some();
            }
            WorldChange::RegionAdopted(region) => {
                self.pending_regions.insert(region);
            }
            WorldChange::RegionUpdated(region) => {
                self.pending_regions.insert(region);
                self.details_dirty |=
                    self.selection.and_then(|selection| self.objects.get(&selection.id)).is_some_and(|row| row.region == region);
            }
            WorldChange::RegionRetired { region, .. } => {
                self.snapshot_dirty |= self.regions.remove(&region).is_some();
                self.pending_regions.remove(&region);
            }
        }
    }

    fn snapshot(&mut self) -> WorldSnapshot {
        if self.snapshot_dirty {
            self.snapshot.regions = self
                .regions
                .values()
                .map(|region| RegionRow {
                    region: region.region,
                    label: region.info.name.as_deref().map_or_else(|| format!("Region {}", region.region.get()), ToOwned::to_owned),
                })
                .collect::<Vec<_>>()
                .into();
            self.snapshot.objects = self.objects.values().cloned().collect::<Vec<_>>().into();
            self.snapshot_dirty = false;
        }
        self.snapshot.revision = self.revision;
        self.snapshot.coverage = self.coverage.clone();
        self.snapshot.object_count = self.object_count;
        self.snapshot.details = self.details.clone();
        self.snapshot.clone()
    }
}

fn refresh_after_resync(mirror: &mut WorldMirror) -> Refresh {
    if mirror.restart_after_history_loss() { Refresh::Resync } else { Refresh::ResyncFailed }
}

async fn capture_snapshot(
    reader: &WorldReader,
    mirror: &mut WorldMirror,
    selected: Option<ObjectSelection>,
    _revision_hint: WorldRevision,
) -> Result<WorldSnapshot, WorldReaderError> {
    let reader_changed = mirror.reader.as_ref().is_some_and(|current| !current.same_session(reader));
    if reader_changed {
        mirror.reset();
        mirror.selection_invalidated = selected.is_some();
    }
    let selected = if reader_changed { None } else { selected };
    let barrier = reader.barrier().await?;
    let target_revision = barrier.revision;
    if mirror.reader.is_none() || mirror.change_cursor.is_none() {
        mirror.begin(reader, barrier);
    } else {
        mirror.coverage = Some(barrier.coverage);
    }
    if mirror.selection != selected {
        mirror.selection = selected;
        mirror.details = None;
        mirror.details_dirty = true;
    }

    if mirror.is_current(target_revision) {
        return Ok(mirror.snapshot());
    }

    for _ in 0..MAX_BOOTSTRAP_PAGES_PER_SYNC {
        if !mirror.objects_done {
            let page = reader.object_list_page(mirror.object_cursor, MAX_OBJECT_PAGE_ITEMS).await?;
            mirror.objects.extend(page.objects.into_iter().map(|item| (item.id, object_row(item))));
            mirror.snapshot_dirty = true;
            mirror.object_cursor = page.next;
            mirror.objects_done = mirror.object_cursor.is_none();
        } else if !mirror.regions_done {
            let page = reader.region_page(mirror.region_cursor, MAX_REGION_PAGE_ITEMS).await?;
            mirror.regions.extend(page.regions.into_iter().map(|summary| (summary.region, summary)));
            mirror.snapshot_dirty = true;
            mirror.region_cursor = page.next;
            mirror.regions_done = mirror.region_cursor.is_none();
        }
        if mirror.objects_done && mirror.regions_done {
            mirror.bootstrapping = false;
            break;
        }
    }

    if !mirror.bootstrapping {
        for _ in 0..MAX_CHANGE_PAGES_PER_SYNC {
            let cursor = mirror.change_cursor.expect("bootstrap establishes a journal cursor");
            let page = reader.changes(cursor, MAX_WORLD_CHANGE_PAGE_ITEMS).await?;
            for record in page.records {
                mirror.apply_change(record.change);
            }
            mirror.change_cursor = Some(page.cursor);
            mirror.has_more_changes = page.has_more;
            mirror.revision = Some(page.observed_revision);
            if !page.has_more {
                mirror.journal_revision = Some(page.observed_revision);
            }
            if !page.has_more {
                break;
            }
        }
    }

    for _ in 0..MAX_SUMMARY_HYDRATIONS_PER_SYNC {
        let Some((id, entity)) = mirror.pending_objects.first_key_value().map(|(id, entity)| (*id, *entity)) else { break };
        match reader.object_list_item(entity).await {
            Ok(read) if read.item.id == id => {
                mirror.pending_objects.remove(&id);
                if mirror.objects.insert(id, object_row(read.item)) != Some(object_row(read.item)) {
                    mirror.snapshot_dirty = true;
                }
                mirror.revision = Some(read.hydration_revision);
            }
            Err(WorldReaderError::Read(WorldReadError::StaleIdentity)) => {
                mirror.pending_objects.remove(&id);
                if mirror.objects.get(&id).is_some_and(|row| row.selection.entity == entity) {
                    mirror.objects.remove(&id);
                    mirror.snapshot_dirty = true;
                }
            }
            Err(error) => return Err(error),
            Ok(_) => {
                mirror.pending_objects.remove(&id);
            }
        }
    }

    for _ in 0..MAX_REGION_HYDRATIONS_PER_SYNC {
        let Some(region) = mirror.pending_regions.first().copied() else { break };
        let summary = reader.region_summary(region).await?;
        mirror.pending_regions.remove(&region);
        match summary {
            Some(read) => {
                if mirror.regions.insert(region, read.summary.clone()) != Some(read.summary) {
                    mirror.snapshot_dirty = true;
                }
                mirror.revision = Some(read.hydration_revision);
            }
            None => {
                mirror.snapshot_dirty |= mirror.regions.remove(&region).is_some();
            }
        }
    }

    if mirror.details_dirty {
        mirror.details = match selected {
            Some(selection) => {
                let batch = reader.object_details(&[selection.entity]).await?;
                mirror.revision = Some(batch.hydration_revision);
                match batch.details.into_iter().next() {
                    Some(ObjectDetailResult::Found(detail)) if detail.summary.id == selection.id => Some(object_details(selection, detail)),
                    Some(ObjectDetailResult::Gone(_) | ObjectDetailResult::TooLarge(_)) | Some(ObjectDetailResult::Found(_)) | None => None,
                }
            }
            None => None,
        };
        mirror.details_dirty = false;
    }

    let snapshot = mirror.snapshot();
    if mirror.is_current(target_revision) {
        mirror.resync_restarts = 0;
    }
    Ok(snapshot)
}

#[cfg(test)]
pub(crate) struct WorldSnapshotForTest {
    pub(crate) objects: Vec<(UUID, world::Entity, Region, NonZeroU32)>,
    pub(crate) regions: Vec<RegionSummary>,
    pub(crate) object_count: usize,
    pub(crate) revision: Option<WorldRevision>,
    pub(crate) selected_position: Option<Vec3>,
    pub(crate) selected_parent: Option<String>,
    pub(crate) selected_id: Option<UUID>,
}

#[cfg(test)]
pub(crate) async fn capture_snapshot_for_test(
    reader: &WorldReader,
    selected: Option<(UUID, world::Entity)>,
    revision_hint: WorldRevision,
) -> Result<WorldSnapshotForTest, WorldReaderError> {
    let mut mirror = WorldMirror { reader: Some(reader.clone()), ..WorldMirror::default() };
    assert!(mirror.restart_after_history_loss());
    let snapshot =
        capture_snapshot(reader, &mut mirror, selected.map(|(id, entity)| ObjectSelection { id, entity }), revision_hint).await?;
    let objects = mirror.objects.iter().map(|(id, row)| (*id, row.selection.entity, row.region, row.local_id)).collect();
    let regions = mirror.regions.values().cloned().collect();
    Ok(WorldSnapshotForTest {
        objects,
        regions,
        object_count: snapshot.object_count,
        revision: snapshot.revision,
        selected_position: snapshot.details.as_ref().and_then(|details| details.position),
        selected_parent: snapshot.details.as_ref().map(|details| details.parent.clone()),
        selected_id: mirror.selection.map(|selection| selection.id),
    })
}

#[cfg(test)]
pub(crate) async fn capture_snapshot_after_session_change_for_test(
    previous_reader: &WorldReader,
    reader: &WorldReader,
    selected: (UUID, world::Entity),
    revision_hint: WorldRevision,
) -> (Result<WorldSnapshotForTest, WorldReaderError>, bool) {
    let (id, entity) = selected;
    let selection = ObjectSelection { id, entity };
    let mut mirror = WorldMirror { reader: Some(previous_reader.clone()), selection: Some(selection), ..WorldMirror::default() };
    let capture = capture_snapshot(reader, &mut mirror, Some(selection), revision_hint).await;
    if matches!(
        &capture,
        Err(WorldReaderError::Read(WorldReadError::ResyncRequired | WorldReadError::InvalidCursor | WorldReadError::StaleSession))
    ) {
        let _ = refresh_after_resync(&mut mirror);
    }
    let result = capture.map(|snapshot| {
        let objects = mirror.objects.iter().map(|(id, row)| (*id, row.selection.entity, row.region, row.local_id)).collect();
        let regions = mirror.regions.values().cloned().collect();
        WorldSnapshotForTest {
            objects,
            regions,
            object_count: snapshot.object_count,
            revision: snapshot.revision,
            selected_position: snapshot.details.as_ref().and_then(|details| details.position),
            selected_parent: snapshot.details.as_ref().map(|details| details.parent.clone()),
            selected_id: mirror.selection.map(|selection| selection.id),
        }
    });
    (result, mirror.selection_invalidated)
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
        Kind, LocalAddress, MAX_OBJECT_PAGE_ITEMS, MAX_REGION_PAGE_ITEMS, ObjectObservation,
        OrderEvidence::Ordered,
        ParentUpdate::Root,
        Region, UpdateSource, World, WorldChange, WorldEntityRemovalReason, WorldLimits,
    };

    use math::{Quat, Vec3};

    use super::{
        CHART_WARMUP, HISTORY_LEN, InspectorState, InspectorTable, KindFilter, LOCAL_ID_CELL_PADDING, LOCAL_ID_GLYPH_WIDTH,
        MAX_BOOTSTRAP_RESTARTS, Metrics, ObjectDetails, ObjectFilter, ObjectRow, ObjectSelection, Palette, PropertyValue, Refresh,
        RegionRow, WorldMirror, WorldSnapshot, WorldSource, inspector_state, local_id_column_width, object_details, object_table,
        object_table_rows, parent_value, property_groups, revision_rate, table_body, table_inner_corner_radius,
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

    #[test]
    fn mirror_builds_a_sorted_snapshot() {
        let region = Region::new(1);
        let mut world = World::new(limits());
        assert_eq!(world.adopt_region(region).expect("adopt region").outcome, RegionAdopted);
        let selected = UUID::from_u128(1);
        for id in 1..=SNAPSHOT_OBJECTS as u128 {
            assert!(matches!(
                world
                    .observe_object(ObjectObservation {
                        source: UpdateSource::new(region, id as u32, Ordered(id as u64), 0),
                        id: UUID::from_u128(id),
                        local_id: NonZeroU32::new(id as u32).expect("nonzero local ID"),
                        kind: Kind::Object,
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
                    .expect("introduce object")
                    .outcome,
                Introduced(_)
            ));
        }

        let barrier = world.read_barrier();
        let selection = ObjectSelection { entity: world.entity(selected).expect("selected object"), id: selected };
        let detail = world.object_detail(selection.entity).expect("selected detail");
        let mut mirror = WorldMirror {
            object_count: barrier.object_count,
            revision: Some(barrier.revision),
            selection: Some(selection),
            details: Some(object_details(selection, detail)),
            ..WorldMirror::default()
        };

        let mut object_cursor = None;
        loop {
            let page = world.object_list_page(object_cursor, MAX_OBJECT_PAGE_ITEMS).expect("object list page");
            mirror.objects.extend(page.objects.into_iter().map(|item| (item.id, super::object_row(item))));
            object_cursor = page.next;
            if object_cursor.is_none() {
                break;
            }
        }
        let mut region_cursor = None;
        loop {
            let page = world.region_page(region_cursor, MAX_REGION_PAGE_ITEMS).expect("region summary page");
            mirror.regions.extend(page.regions.into_iter().map(|summary| (summary.region, summary)));
            region_cursor = page.next;
            if region_cursor.is_none() {
                break;
            }
        }
        mirror.snapshot_dirty = true;
        let snapshot = mirror.snapshot();
        assert_eq!(snapshot.objects.len(), SNAPSHOT_OBJECTS);
        assert_eq!(snapshot.object_count, SNAPSHOT_OBJECTS);
        assert_eq!(snapshot.details.as_ref().map(|details| details.selection.id), Some(selected));
        assert!(snapshot.objects.iter().all(|object| object.region == region));
    }

    #[test]
    fn old_removal_keeps_reused_object() {
        let region = Region::new(1);
        let id = UUID::from_u128(42);
        let local_id = NonZeroU32::new(9).unwrap();
        let mut world = World::new(limits());
        assert_eq!(world.adopt_region(region).expect("adopt region").outcome, RegionAdopted);
        let mut observation = ObjectObservation {
            source: UpdateSource::new(region, 1, Ordered(1), 0),
            id,
            local_id,
            kind: Kind::Object,
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
        };
        assert!(matches!(world.observe_object(observation.clone()).unwrap().outcome, Introduced(_)));
        let old_entity = world.entity(id).unwrap();
        let old_item = world.object_list_item(old_entity).unwrap().item;
        let selection = ObjectSelection { entity: old_entity, id };

        let _ = world.remove_object(UpdateSource::new(region, 2, Ordered(2), 0), local_id).expect("remove selected entity");
        observation.source = UpdateSource::new(region, 3, Ordered(3), 0);
        assert!(matches!(world.observe_object(observation).unwrap().outcome, Introduced(_)));
        let replacement = ObjectSelection { entity: world.entity(id).unwrap(), id };
        assert_ne!(selection.entity, replacement.entity);

        let item = world.object_list_item(replacement.entity).unwrap().item;
        let mut mirror = WorldMirror {
            object_count: 1,
            objects: std::collections::BTreeMap::from([(id, super::object_row(old_item))]),
            selection: Some(selection),
            ..WorldMirror::default()
        };
        mirror.apply_change(WorldChange::EntityRemoved {
            id,
            entity: old_entity,
            address: LocalAddress::new(region, local_id),
            reason: WorldEntityRemovalReason::Killed,
        });
        assert_eq!(mirror.object_count, 0);
        mirror.apply_change(WorldChange::EntityAdded { id, entity: replacement.entity, address: item.address });
        mirror.objects.insert(id, super::object_row(item));
        mirror.snapshot_dirty = true;
        let snapshot = mirror.snapshot();
        let table = object_table(&snapshot, &ObjectFilter::default());

        assert_eq!(replacement.id, selection.id);
        assert_eq!(snapshot.object_count, 1);
        assert!(snapshot.details.is_none());
        assert!(matches!(inspector_state(&snapshot, Some(selection)), InspectorState::Removed));
        assert_eq!(table.row_for_object(Some(selection)), None);
        assert_eq!(table.row_for_object(Some(replacement)), Some(0));
    }

    #[test]
    fn updates_refresh_selected_details() {
        let selection = sample_snapshot().objects[0].selection;
        let mut mirror = WorldMirror { selection: Some(selection), ..WorldMirror::default() };

        mirror.apply_change(WorldChange::EntityUpdated { id: selection.id, entity: selection.entity, classes: Default::default() });

        assert!(mirror.pending_objects.is_empty());
        assert!(mirror.details_dirty);
    }

    #[test]
    fn repeated_resync_requires_refresh() {
        let mut mirror = WorldMirror { object_count: 5, ..WorldMirror::default() };

        for attempt in 1..=MAX_BOOTSTRAP_RESTARTS {
            assert!(matches!(super::refresh_after_resync(&mut mirror), Refresh::Resync));
            assert_eq!(mirror.resync_restarts, attempt);
            assert_eq!(mirror.object_count, 0);
        }

        mirror.object_count = 7;
        assert!(matches!(super::refresh_after_resync(&mut mirror), Refresh::ResyncFailed));
        assert_eq!(mirror.resync_restarts, MAX_BOOTSTRAP_RESTARTS);
        assert_eq!(mirror.object_count, 7);
    }

    #[test]
    fn hydration_does_not_advance_journal() {
        let region = Region::new(1);
        let mut world = World::new(limits());
        let start = world.read_barrier().revision;
        let mut mirror = WorldMirror { journal_revision: Some(start), revision: Some(start), ..WorldMirror::default() };
        assert!(mirror.is_current(start));

        assert_eq!(world.adopt_region(region).expect("adopt region").outcome, RegionAdopted);
        let hydrated = world.region_summary(region).unwrap().expect("region summary").hydration_revision;
        mirror.revision = Some(hydrated);

        assert!(!mirror.is_current(hydrated));
        mirror.journal_revision = Some(hydrated);
        assert!(mirror.is_current(hydrated));
    }

    fn sample_snapshot() -> WorldSnapshot {
        WorldSnapshot {
            regions: Arc::from(vec![
                RegionRow { region: Region::new(1), label: "Mauve".into() },
                RegionRow { region: Region::new(2), label: "Ochre".into() },
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
            regions: Arc::from(vec![RegionRow { region, label: "Mauve".into() }]),
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

        table.rows = object_table_rows(&snapshot, &ObjectFilter::default());
        table.apply_sort();
        assert_eq!(table.rows[0].local_id.get(), 2);

        table.sort_rows(3, ColumnSort::Descending);
        assert_eq!(
            table.rows.iter().map(|row| row.selection.id).collect::<Vec<_>>(),
            [UUID::from_u128(3), UUID::from_u128(1), UUID::from_u128(2)]
        );
    }

    #[test]
    fn details_hide_debug_wrappers_on_optional_values() {
        let details = ObjectDetails {
            selection: object_row(9, Kind::Object, Region::new(3), 8).selection,
            kind: Kind::Object,
            address: LocalAddress::new(Region::new(3), NonZeroU32::new(8).unwrap()),
            parent: "Root".into(),
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
    fn chart_samples_use_one_second_windows() {
        let start = Instant::now();
        let snapshot = sample_snapshot();
        let mut metrics = Metrics::default();

        metrics.record(&snapshot, Duration::from_millis(1), start);
        metrics.record(&snapshot, Duration::from_millis(3), start + Duration::from_secs(2));
        assert_eq!(metrics.chart.len(), 1);
        assert_eq!(metrics.chart[0].capture_ms, 2.0);
        assert_eq!(metrics.chart[0].capture_peak_ms, 3.0);

        metrics.record(&snapshot, Duration::from_millis(5), start + Duration::from_millis(2100));
        metrics.record(&snapshot, Duration::from_millis(7), start + Duration::from_millis(2500));
        assert_eq!(metrics.chart.len(), 1);

        metrics.record(&snapshot, Duration::from_millis(9), start + Duration::from_secs(3));
        assert_eq!(metrics.chart.len(), 2);
        assert_eq!(metrics.chart[0].elapsed.as_ref(), "0:02");
        assert_eq!(metrics.chart[1].elapsed.as_ref(), "0:03");
        assert_eq!(metrics.chart[1].capture_ms, 7.0);
        assert_eq!(metrics.chart[1].capture_peak_ms, 9.0);
    }

    #[test]
    fn metrics_history_is_bounded_and_rate_is_per_second() {
        assert_eq!(revision_rate(10, 40, Duration::from_secs(2)), 15.0);
        assert_eq!(revision_rate(40, 10, Duration::from_secs(1)), 0.0);

        let start = Instant::now();
        let mut warming = Metrics::default();
        warming.record(&sample_snapshot(), Duration::ZERO, start);
        warming.record(&sample_snapshot(), Duration::ZERO, start + Duration::from_secs(1));
        assert!(warming.chart.is_empty());
        warming.record(&sample_snapshot(), Duration::ZERO, start + CHART_WARMUP);
        assert_eq!(warming.chart[0].elapsed.as_ref(), "0:02");

        let mut metrics = Metrics::default();
        let now = Instant::now();
        for step in 0..(HISTORY_LEN + 2) {
            metrics.record(&sample_snapshot(), Duration::from_micros(250), now + Duration::from_secs(step as u64));
        }
        assert_eq!(metrics.chart.len(), HISTORY_LEN);
        assert_eq!(metrics.chart[0].elapsed.as_ref(), "0:02");
        assert_eq!(metrics.chart[HISTORY_LEN - 1].elapsed.as_ref(), "2:01");
        assert!(metrics.chart.iter().all(|point| !point.elapsed.starts_with('#')));
        assert_eq!(metrics.chart.last().map(|point| point.objects), Some(3.0));
        assert_eq!(metrics.latest().map(|point| point.object_count), Some(3));
        assert_eq!(metrics.peak_capture_ms(), Some(0.25));
    }

    #[test]
    fn object_counts_use_readable_grouping() {
        assert_eq!(super::format_count(0), "0");
        assert_eq!(super::format_count(999), "999");
        assert_eq!(super::format_count(1_234_567), "1,234,567");
    }

    #[test]
    fn world_source_starts_disconnected() {
        let source = WorldSource::new();
        assert!(source.is_disconnected());
        source.set(None);
        assert!(source.is_disconnected());
    }
}
