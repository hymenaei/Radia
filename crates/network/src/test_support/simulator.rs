use std::{net::SocketAddr, time::Duration};

use protocol::{
    lludp::{Message, Packet},
    messages::{PacketAck, circuit::packet_ack::Packets},
};
use tokio::{net::UdpSocket, time::timeout};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn test_source() -> SocketAddr {
    "127.0.0.1:13000".parse().expect("fixture simulator endpoint")
}

pub(crate) async fn receive_udp(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
    let mut buffer = [0; 65_535];
    let (size, endpoint) = timeout(TEST_TIMEOUT, socket.recv_from(&mut buffer))
        .await
        .expect("receive local simulator packet")
        .expect("read local simulator packet");
    (buffer[..size].to_vec(), endpoint)
}

pub(crate) async fn receive_message(socket: &UdpSocket, expected: impl Fn(&Message) -> bool) -> Packet {
    timeout(TEST_TIMEOUT, async {
        loop {
            let (bytes, _) = receive_udp(socket).await;
            let packet = Packet::decode(&bytes).expect("decode local simulator packet");
            if expected(packet.message()) {
                return packet;
            }
        }
    })
    .await
    .expect("receive expected local simulator message")
}

pub(crate) fn acknowledgement(sequence: u32, received_sequence: u32) -> Vec<u8> {
    Packet::new(sequence, Message::PacketAck(PacketAck { packets: vec![Packets { id: received_sequence }] }))
        .encode()
        .expect("encode simulator circuit acknowledgement")
}
