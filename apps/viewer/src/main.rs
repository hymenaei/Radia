mod logging;

use std::io;
use std::sync::Arc;

use gpui_component_assets::Assets;
use tokio::runtime::Builder;

fn main() -> io::Result<()> {
    let _logging = logging::init();
    let runtime = Builder::new_multi_thread().enable_all().thread_name("radia-service").build()?;
    let services = runtime.handle().clone();
    let asset_cache = match assets::cache::AssetCache::open_default() {
        Ok(cache) => Some(Arc::new(cache)),
        Err(error) => {
            tracing::warn!(%error, "asset cache unavailable; continuing without persistence");
            None
        }
    };
    let gui_cache = asset_cache.clone();
    gpui_platform::application().with_assets(Assets).run(move |cx| viewer::gui::init(cx, services, gui_cache));
    if let Some(cache) = asset_cache
        && let Ok(cache) = Arc::try_unwrap(cache)
        && let Err(error) = cache.shutdown()
    {
        tracing::warn!(%error, "asset cache shutdown failed");
    }
    runtime.shutdown_background();
    Ok(())
}
