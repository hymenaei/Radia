use uuid::Uuid;

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
