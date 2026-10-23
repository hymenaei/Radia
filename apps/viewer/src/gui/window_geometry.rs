use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use gpui::{App, Bounds, Context, Pixels, Point, Size, Subscription, Window, point, px, size};
use serde::{Deserialize, Serialize};
use serde_json::{from_slice, to_vec};
use utils::write_atomic;

const SAVE_DELAY: Duration = Duration::from_millis(400);
const MAX_WINDOW_DIMENSION: u32 = 8192;
const MIN_VISIBLE_TITLEBAR_WIDTH: f32 = 64.0;
const MIN_VISIBLE_TITLEBAR_HEIGHT: f32 = 24.0;
const TITLEBAR_HEIGHT: f32 = 48.0;

static SAVE_LOCK: Mutex<()> = Mutex::new(());

type StoredWindowGeometries = BTreeMap<String, StoredWindowGeometry>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StoredWindowGeometry {
    width: u32,
    height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    y: Option<i32>,
}

impl StoredWindowGeometry {
    fn size(self) -> StoredWindowSize {
        StoredWindowSize { width: self.width, height: self.height }
    }

    fn origin(self) -> Option<Point<Pixels>> {
        Some(point(px(self.x? as f32), px(self.y? as f32)))
    }
}

impl From<Bounds<Pixels>> for StoredWindowGeometry {
    fn from(bounds: Bounds<Pixels>) -> Self {
        Self {
            width: u32::from(bounds.size.width.round()),
            height: u32::from(bounds.size.height.round()),
            x: Some(f32::from(bounds.origin.x).round() as i32),
            y: Some(f32::from(bounds.origin.y).round() as i32),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StoredWindowSize {
    width: u32,
    height: u32,
}

impl From<Size<Pixels>> for StoredWindowSize {
    fn from(size: Size<Pixels>) -> Self {
        Self { width: u32::from(size.width.round()), height: u32::from(size.height.round()) }
    }
}

impl From<StoredWindowSize> for Size<Pixels> {
    fn from(stored_size: StoredWindowSize) -> Self {
        size(px(stored_size.width as f32), px(stored_size.height as f32))
    }
}

pub(crate) fn restore_bounds(
    key: &str,
    display_id: Option<gpui::DisplayId>,
    default: Size<Pixels>,
    minimum: Size<Pixels>,
    cx: &App,
) -> Bounds<Pixels> {
    let default = StoredWindowSize::from(default);
    let minimum = StoredWindowSize::from(minimum);
    let saved = storage_path().and_then(|path| read_geometries(&path).ok()?.remove(key));
    let restored_size = clamp_size(saved.map_or(default, StoredWindowGeometry::size), minimum).into();
    let default_bounds = Bounds::centered(display_id, restored_size, cx);

    let Some(origin) = saved.and_then(StoredWindowGeometry::origin) else {
        return default_bounds;
    };

    let restored = Bounds { origin, size: restored_size };
    let displays = cx.displays();
    if displays.iter().any(|display| has_usable_titlebar(restored, display.visible_bounds())) {
        return restored;
    }

    let display = display_id.and_then(|id| cx.find_display(id)).or_else(|| cx.primary_display());
    display.map_or(default_bounds, |display| center_on_display(restored_size, display.visible_bounds()))
}

pub(crate) fn observe<T: 'static>(key: &'static str, window: &mut Window, cx: &mut Context<T>) -> Subscription {
    let Some(path) = storage_path() else {
        return cx.observe_window_bounds(window, |_, _, _| {});
    };
    let generation = Arc::new(AtomicU64::new(0));

    cx.observe_window_bounds(window, move |_, window, cx| {
        let current_geometry = StoredWindowGeometry::from(window.bounds());
        let current_generation = generation.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let generation = Arc::clone(&generation);
        let path = path.clone();
        let executor = cx.background_executor().clone();

        executor
            .clone()
            .spawn(async move {
                executor.timer(SAVE_DELAY).await;
                if generation.load(Ordering::Relaxed) == current_generation
                    && let Err(error) = save_geometry(&path, key, current_geometry)
                {
                    tracing::debug!(%error, "could not save window geometry");
                }
            })
            .detach();
    })
}

fn storage_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|path| path.join("Radia").join("window-sizes.json"))
}

fn read_geometries(path: &Path) -> std::io::Result<StoredWindowGeometries> {
    match fs::read(path) {
        Ok(contents) => from_slice(&contents).map_err(std::io::Error::other),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(StoredWindowGeometries::new()),
        Err(error) => Err(error),
    }
}

fn save_geometry(path: &Path, key: &str, geometry: StoredWindowGeometry) -> std::io::Result<()> {
    let _guard = SAVE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let mut geometries = read_geometries(path)?;
    geometries.insert(key.to_owned(), geometry);
    let contents = to_vec(&geometries).map_err(std::io::Error::other)?;
    fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))?;
    write_atomic(path, &contents)
}

fn clamp_size(size: StoredWindowSize, minimum: StoredWindowSize) -> StoredWindowSize {
    StoredWindowSize {
        width: size.width.clamp(minimum.width.min(MAX_WINDOW_DIMENSION), MAX_WINDOW_DIMENSION),
        height: size.height.clamp(minimum.height.min(MAX_WINDOW_DIMENSION), MAX_WINDOW_DIMENSION),
    }
}

fn has_usable_titlebar(window: Bounds<Pixels>, display: Bounds<Pixels>) -> bool {
    let titlebar = Bounds { origin: window.origin, size: size(window.size.width, px(f32::from(window.size.height).min(TITLEBAR_HEIGHT))) };
    let visible = titlebar.intersect(&display);
    let required_width = f32::from(window.size.width).min(MIN_VISIBLE_TITLEBAR_WIDTH).min(f32::from(display.size.width));
    let required_height = f32::from(titlebar.size.height).min(MIN_VISIBLE_TITLEBAR_HEIGHT).min(f32::from(display.size.height));

    f32::from(visible.size.width) >= required_width && f32::from(visible.size.height) >= required_height
}

fn center_on_display(size: Size<Pixels>, display: Bounds<Pixels>) -> Bounds<Pixels> {
    let centered = Bounds::centered_at(display.center(), size);
    fit_inside_display(centered, display)
}

fn fit_inside_display(bounds: Bounds<Pixels>, display: Bounds<Pixels>) -> Bounds<Pixels> {
    let left = f32::from(display.origin.x);
    let top = f32::from(display.origin.y);
    let right = f32::from(display.bottom_right().x);
    let bottom = f32::from(display.bottom_right().y);
    let max_x = (right - f32::from(bounds.size.width)).max(left);
    let max_y = (bottom - f32::from(bounds.size.height)).max(top);

    Bounds {
        origin: point(px(f32::from(bounds.origin.x).clamp(left, max_x)), px(f32::from(bounds.origin.y).clamp(top, max_y))),
        size: bounds.size,
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, Point, px, size};
    use tempfile::tempdir;

    use super::{
        MAX_WINDOW_DIMENSION, StoredWindowGeometry, StoredWindowSize, center_on_display, clamp_size, has_usable_titlebar, read_geometries,
        save_geometry,
    };

    #[test]
    fn restored_size_respects_minimum_and_maximum_dimensions() {
        assert_eq!(
            clamp_size(StoredWindowSize { width: 200, height: MAX_WINDOW_DIMENSION + 1 }, StoredWindowSize { width: 640, height: 400 }),
            StoredWindowSize { width: 640, height: MAX_WINDOW_DIMENSION }
        );
    }

    #[test]
    fn named_geometry_shares_file() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("window-sizes.json");
        let main = StoredWindowGeometry { x: Some(-120), y: Some(60), width: 720, height: 480 };
        let resized_main = StoredWindowGeometry { x: Some(340), y: Some(90), width: 900, height: 640 };
        let developer_tools = StoredWindowGeometry { x: Some(920), y: Some(140), width: 1080, height: 760 };

        save_geometry(&path, "main", main).unwrap();
        save_geometry(&path, "developer-tools", developer_tools).unwrap();
        save_geometry(&path, "main", resized_main).unwrap();

        let saved = read_geometries(&path).unwrap();
        assert_eq!(saved.get("main"), Some(&resized_main));
        assert_eq!(saved.get("developer-tools"), Some(&developer_tools));
    }

    #[test]
    fn old_size_entries_without_positions_still_deserialize() {
        let geometry: StoredWindowGeometry = serde_json::from_str(r#"{"width":720,"height":480}"#).unwrap();
        assert_eq!(geometry.size(), StoredWindowSize { width: 720, height: 480 });
        assert_eq!(geometry.origin(), None);
    }

    #[test]
    fn saved_position_is_usable_when_its_titlebar_is_visible() {
        let display = Bounds { origin: Point { x: px(0.), y: px(0.) }, size: size(px(1920.), px(1080.)) };
        let window = Bounds { origin: Point { x: px(1820.), y: px(10.) }, size: size(px(900.), px(640.)) };

        assert!(has_usable_titlebar(window, display));
    }

    #[test]
    fn off_screen_saved_position_can_be_centered_back_on_a_display() {
        let display = Bounds { origin: Point { x: px(0.), y: px(0.) }, size: size(px(1920.), px(1080.)) };
        let restored = center_on_display(size(px(900.), px(640.)), display);

        assert_eq!(restored.origin, Point { x: px(510.), y: px(220.) });
        assert!(has_usable_titlebar(restored, display));
    }
}
