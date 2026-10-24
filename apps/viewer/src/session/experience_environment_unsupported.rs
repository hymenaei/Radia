use std::convert::Infallible;

use protocol::lludp::Message;
use uuid::Uuid;
use world::Region;

pub(crate) type EnvironmentQueueError = Infallible;

pub(crate) struct EnvironmentSender;

impl EnvironmentSender {
    pub(crate) fn clear_region(&self, _: Region) -> Result<(), EnvironmentQueueError> {
        Ok(())
    }

    pub(crate) fn publish(&self, _: &Message, _: Uuid, _: Region, _: network::SimulatorId) -> Result<(), EnvironmentQueueError> {
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct EnvironmentInbox;

impl EnvironmentInbox {
    pub(crate) fn channel() -> (EnvironmentSender, Self) {
        (EnvironmentSender, Self)
    }
}
