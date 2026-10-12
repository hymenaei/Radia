use super::common::PingId;

/// Requests a circuit ping and reports the oldest unacknowledged packet.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StartPingCheck {
    pub ping_id: PingId,
    pub oldest_unacked: u32,
}
