use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::AGENT_THROTTLE,
};

#[derive(Clone, Debug, PartialEq)]
pub struct AgentThrottle {
    pub agent_data: AgentData,
    pub throttle: Throttle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub circuit_code: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Throttle {
    pub gen_counter: u32,
    pub throttles: ThrottleRates,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThrottleRates {
    pub resend_bits_per_second: f32,
    pub land_bits_per_second: f32,
    pub wind_bits_per_second: f32,
    pub cloud_bits_per_second: f32,
    pub task_bits_per_second: f32,
    pub texture_bits_per_second: f32,
    pub asset_bits_per_second: f32,
}

impl ThrottleRates {
    pub(crate) const fn from_wire_order(values: [f32; 7]) -> Self {
        Self {
            resend_bits_per_second: values[0],
            land_bits_per_second: values[1],
            wind_bits_per_second: values[2],
            cloud_bits_per_second: values[3],
            task_bits_per_second: values[4],
            texture_bits_per_second: values[5],
            asset_bits_per_second: values[6],
        }
    }

    pub(crate) const fn wire_order(self) -> [f32; 7] {
        [
            self.resend_bits_per_second,
            self.land_bits_per_second,
            self.wind_bits_per_second,
            self.cloud_bits_per_second,
            self.task_bits_per_second,
            self.texture_bits_per_second,
            self.asset_bits_per_second,
        ]
    }

    pub(crate) fn is_valid(self) -> bool {
        self.wire_order().into_iter().all(|rate| rate.is_finite() && rate >= 0.0)
    }
}

impl TypedMessage for AgentThrottle {
    const ID: MessageId = AGENT_THROTTLE;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = false;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let agent_data = AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()?, circuit_code: reader.u32()? };
        let gen_counter = reader.u32()?;
        let byte_count = usize::from(reader.u8()?);
        if byte_count != 7 * std::mem::size_of::<f32>() {
            return Err(LLUdpError::InvalidMessage);
        }
        let mut rates = [0.0; 7];
        for rate in &mut rates {
            *rate = reader.f32()?;
        }
        let throttles = ThrottleRates::from_wire_order(rates);
        if !throttles.is_valid() {
            return Err(LLUdpError::InvalidMessage);
        }
        Ok(Self { agent_data, throttle: Throttle { gen_counter, throttles } })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        let rates = self.throttle.throttles;
        if !rates.is_valid() {
            return Err(LLUdpError::InvalidMessage);
        }
        writer.uuid(self.agent_data.agent_id);
        writer.uuid(self.agent_data.session_id);
        writer.u32(self.agent_data.circuit_code);
        writer.u32(self.throttle.gen_counter);
        writer.u8(28);
        for rate in rates.wire_order() {
            writer.f32(rate);
        }
        Ok(())
    }
}
