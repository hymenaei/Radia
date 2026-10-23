use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::GENERIC_MESSAGE,
};

#[derive(Clone, Debug, PartialEq)]
pub struct GenericMessage {
    pub agent_data: AgentData,
    pub method_data: MethodData,
    pub param_list: Option<Vec<Parameter>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub transaction_id: Uuid,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MethodData {
    pub method: Vec<u8>,
    pub invoice: Option<Uuid>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Parameter {
    pub parameter: Vec<u8>,
}

impl TypedMessage for GenericMessage {
    const ID: MessageId = GENERIC_MESSAGE;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = false;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let agent_data = AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()?, transaction_id: reader.uuid()? };
        let method = reader.variable1()?.to_vec();
        let invoice = reader.optional_fixed(16, |reader| reader.uuid())?;
        let param_list = if reader.is_empty() {
            None
        } else {
            let count = usize::from(reader.u8()?);
            let mut parameters = Vec::with_capacity(count);
            for _ in 0..count {
                parameters.push(Parameter { parameter: reader.variable1()?.to_vec() });
            }
            Some(parameters)
        };
        Ok(Self { agent_data, method_data: MethodData { method, invoice }, param_list })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.agent_data.agent_id);
        writer.uuid(self.agent_data.session_id);
        writer.uuid(self.agent_data.transaction_id);
        writer.variable1(&self.method_data.method)?;
        let Some(invoice) = self.method_data.invoice else { return Ok(()) };
        writer.uuid(invoice);
        if let Some(parameters) = &self.param_list {
            writer.u8(u8::try_from(parameters.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
            for parameter in parameters {
                writer.variable1(&parameter.parameter)?;
            }
        }
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        let mut bytes = crate::lludp::message::allocation::capacity_bytes(self.method_data.method.capacity(), std::mem::size_of::<u8>());
        if let Some(parameters) = &self.param_list {
            crate::lludp::message::allocation::add_capacity(&mut bytes, parameters.capacity(), std::mem::size_of::<Parameter>());
            for parameter in parameters {
                crate::lludp::message::allocation::add_capacity(&mut bytes, parameter.parameter.capacity(), std::mem::size_of::<u8>());
            }
        }
        bytes
    }
}
