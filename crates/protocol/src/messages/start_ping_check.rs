/// Requests a circuit ping and reports the oldest unacknowledged packet.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StartPingCheck {
    pub ping_id: PingID,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PingID {
    pub ping_id: u8,
    pub oldest_unacked: u32,
}
