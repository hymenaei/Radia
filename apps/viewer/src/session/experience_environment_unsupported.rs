use protocol::lludp::Message;
use uuid::Uuid;
use world::Region;

pub(crate) struct EnvironmentSender;

impl EnvironmentSender {
    pub(crate) fn clear_region(&self, _: Region) -> Result<(), String> {
        Ok(())
    }

    pub(crate) fn publish(&self, _: &Message, _: Uuid, _: Region, _: network::SimulatorId) -> Result<(), String> {
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
