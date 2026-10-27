use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::AGENT_SIT,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentSit {
    pub agent_id: Uuid,
    pub session_id: Uuid,
}

impl TypedMessage for AgentSit {
    const ID: MessageId = AGENT_SIT;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = false;
    const ZEROCODED: Option<bool> = Some(false);

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self { agent_id: reader.uuid()?, session_id: reader.uuid()? })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.agent_id);
        writer.uuid(self.session_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use crate::lludp::{Message, Packet};

    use super::AgentSit;

    #[test]
    fn agent_sit_round_trips() {
        let message = AgentSit { agent_id: Uuid::from_u128(1), session_id: Uuid::from_u128(2) };
        let packet = Packet::decode(&Packet::new(1, Message::AgentSit(message.clone())).encode().expect("encode sit")).expect("decode sit");
        assert_eq!(packet.message(), &Message::AgentSit(message));
    }
}
