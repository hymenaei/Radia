/// Acknowledges packets received reliably.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PacketAck {
    /// Acknowledged packet IDs in wire order; the packet carries a one-byte count.
    pub packets: Vec<Packets>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Packets {
    pub id: u32,
}
