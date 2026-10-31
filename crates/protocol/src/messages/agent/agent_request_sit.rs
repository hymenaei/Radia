use math::Vec3;
use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::AGENT_REQUEST_SIT,
};

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRequestSit {
    pub agent_data: AgentData,
    pub target_object: TargetObject,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TargetObject {
    pub target_id: Uuid,
    pub offset: Vec3,
}

impl TypedMessage for AgentRequestSit {
    const ID: MessageId = AGENT_REQUEST_SIT;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = false;
    const ZEROCODED: Option<bool> = Some(true);

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self {
            agent_data: AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? },
            target_object: TargetObject { target_id: reader.uuid()?, offset: reader.vector3()? },
        })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.agent_data.agent_id);
        writer.uuid(self.agent_data.session_id);
        writer.uuid(self.target_object.target_id);
        writer.vector3(self.target_object.offset);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use math::Vec3;
    use uuid::Uuid;

    use crate::lludp::{Message, Packet};

    use super::{AgentData, AgentRequestSit, TargetObject};

    #[test]
    fn request_sit_round_trips() {
        let message = AgentRequestSit {
            agent_data: AgentData { agent_id: Uuid::from_u128(1), session_id: Uuid::from_u128(2) },
            target_object: TargetObject { target_id: Uuid::from_u128(3), offset: Vec3::new(1.0, 2.0, 3.0) },
        };
        let packet = Packet::decode(&Packet::new(1, Message::AgentRequestSit(message.clone())).encode().expect("encode sit request"))
            .expect("decode sit request");
        assert_eq!(packet.message(), &Message::AgentRequestSit(message));
    }
}
