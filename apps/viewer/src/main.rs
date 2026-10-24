mod logging;

use std::io;

use gpui_component_assets::Assets;
use tokio::runtime::Builder;

fn main() -> io::Result<()> {
    let _logging = logging::init();
    let runtime = Builder::new_multi_thread().enable_all().thread_name("radia-service").build()?;
    let services = runtime.handle().clone();
    gpui_platform::application().with_assets(Assets).run(move |cx| viewer::gui::init(cx, services));
    runtime.shutdown_background();
    Ok(())
}
