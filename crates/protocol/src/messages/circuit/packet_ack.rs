#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PacketAck {
    pub packets: Vec<Packets>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Packets {
    pub id: u32,
}
