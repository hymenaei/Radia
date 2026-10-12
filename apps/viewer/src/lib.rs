//! Viewer application and its inventory and world integrations.

mod app;
mod chat;
mod devtools;
mod diagnostics;
mod login;
mod remembered_login;
mod secondary_windows;
mod session;
#[cfg(target_os = "windows")]
mod window_chrome;
mod window_geometry;

pub mod inventory;

pub use app::run;
