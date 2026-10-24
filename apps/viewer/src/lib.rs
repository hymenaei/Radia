mod app;
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod camera;
mod chat;
mod logging;
mod login;
mod remembered_login;
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod scene_feed;
mod session;
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod viewport;
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod window_chrome;
mod window_geometry;

pub(crate) const DEFAULT_DRAW_DISTANCE: f32 = 512.0;

pub mod asset_coordinator;
pub mod inventory;
pub mod world_reader;

pub use app::run;
