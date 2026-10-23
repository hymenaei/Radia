use super::PingId;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CompletePingCheck {
    pub ping_id: PingId,
}
