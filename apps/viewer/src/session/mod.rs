mod agent_control;
mod agent_motion;
mod coordinator;
mod environment;
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod experience_environment;
mod inbound;
mod object_cache;
mod object_media;
mod outbound;
mod recovery;
mod region_lookup;
mod terrain;
mod world;

#[cfg(any(target_os = "windows", target_os = "linux", test))]
pub(crate) use agent_control::SitCameraOverride;
pub use agent_control::{AgentControlError, AgentControlHandle, SitOutcome};
pub(crate) use agent_control::{AgentControlEvent, AgentControlState, SitResponseState};
pub(crate) use agent_motion::{AgentMotionPublisher, AgentMotionSample};
pub(crate) use coordinator::{
    AuthenticationUpdate, CredentialPersistence, FavoritesUpdate, PresentationMailbox, PresentationSnapshot, SessionEnd,
    SessionReadyUpdate, ViewerUpdate, WriteLock, drain_session, save_favorite_store, write_lock,
};
#[cfg(test)]
pub(crate) use coordinator::{apply_inventory_udp, resolve_pending_inventory_moves};
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) use experience_environment::EnvironmentInbox;
pub(crate) use inbound::{ChatMessage, display_message, format_avatar_name};
pub(crate) use network::{Session, SessionHandle};
pub(crate) use outbound::CameraUplink;
pub(crate) use region_lookup::{RegionGridCell, RegionLookup, RegionLookupError, map_request_coordinates};

#[cfg(all(test, any(target_os = "windows", target_os = "linux")))]
pub(crate) use experience_environment::EnvironmentCommand;
