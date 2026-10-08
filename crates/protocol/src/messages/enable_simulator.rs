/// Describes a simulator endpoint made available to the viewer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnableSimulator {
    pub simulator_info: SimulatorInfo,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SimulatorInfo {
    /// Global handle of the simulator's region.
    pub handle: u64,
    /// Four IPv4 address octets in wire order.
    pub ip: [u8; 4],
    /// Two port bytes in wire order.
    pub port: [u8; 2],
}
