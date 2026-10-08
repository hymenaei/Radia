/// Replies to a circuit ping request.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CompletePingCheck {
    pub ping_id: PingID,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PingID {
    pub ping_id: u8,
}
