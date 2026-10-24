use std::net::SocketAddr;

use tokio::sync::mpsc;

use crate::{event::EventEnvelope, event::SimulatorId, session::error::SessionError};

use super::SessionState;

pub(in crate::session) fn publish_primary_terminal_event(
    permit: mpsc::OwnedPermit<EventEnvelope>,
    simulator_id: SimulatorId,
    region_handle: Option<u64>,
    endpoint: SocketAddr,
    result: &Result<(), SessionError>,
) {
    SessionState::publish_primary_terminal_event(permit, simulator_id, region_handle, endpoint, result);
}
