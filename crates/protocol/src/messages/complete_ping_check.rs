use super::common::PingId;

/// Replies to a circuit ping request.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CompletePingCheck {
    pub ping_id: PingId,
}
