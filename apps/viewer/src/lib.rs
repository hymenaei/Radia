pub mod asset_coordinator;
pub(crate) mod camera;
pub mod gui;
pub mod inventory;
mod remembered_login;
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod scene_feed;
mod session;
pub mod world_reader;

pub(crate) const DEFAULT_DRAW_DISTANCE: f32 = 512.0;
