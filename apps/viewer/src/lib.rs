mod agent_movement;
pub mod asset_coordinator;
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) mod camera;
pub mod gui;
pub mod inventory;
mod remembered_login;
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod scene_feed;
mod session;
#[cfg(test)]
mod test_helpers;
pub mod world_reader;
pub mod teleport {
    pub use crate::session::{
        CrossingState, TeleportAlertInfo, TeleportFailure, TeleportFailureReason, TeleportHandle, TeleportLandmarkRequest,
        TeleportLocationRequest, TeleportProgressKind, TeleportRequestError, TeleportSnapshot, TeleportStatus,
    };
}

pub(crate) const DEFAULT_DRAW_DISTANCE: f32 = 512.0;
