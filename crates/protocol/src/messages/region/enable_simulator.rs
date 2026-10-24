use std::net::Ipv4Addr;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnableSimulator {
    pub simulator_info: SimulatorInfo,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatorInfo {
    pub handle: u64,
    pub ip: Ipv4Addr,
    pub port: u16,
}
