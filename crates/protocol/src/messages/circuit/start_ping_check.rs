use super::PingId;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StartPingCheck {
    pub ping_id: PingId,
    pub oldest_unacked: u32,
}
