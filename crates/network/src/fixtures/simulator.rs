use std::{collections::HashSet, io, net::SocketAddr, time::Duration};

use protocol::{
    lludp::{Message, Packet},
    messages::{PacketAck, circuit::packet_ack::Packets},
};
use tokio::{net::UdpSocket, time::timeout};

use crate::event::SimulatorId;

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) const fn test_simulator_id(generation: u64) -> SimulatorId {
    SimulatorId::new(generation)
}

pub(crate) fn test_source() -> SocketAddr {
    "127.0.0.1:13000".parse().expect("fixture simulator endpoint")
}

pub(crate) fn clone_udp_socket(socket: &UdpSocket) -> io::Result<std::net::UdpSocket> {
    #[cfg(unix)]
    let owned = {
        use std::os::fd::AsFd;
        socket.as_fd().try_clone_to_owned()?
    };
    #[cfg(windows)]
    let owned = {
        use std::os::windows::io::AsSocket;
        socket.as_socket().try_clone_to_owned()?
    };
    let socket = std::net::UdpSocket::from(owned);
    socket.set_nonblocking(true)?;
    Ok(socket)
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

pub(crate) async fn receive_message_with_acknowledgements(
    socket: &UdpSocket,
    expected: impl Fn(&Message) -> bool,
) -> (Packet, HashSet<u32>) {
    timeout(TEST_TIMEOUT, async {
        let mut acknowledgements = HashSet::new();
        loop {
            let (bytes, _) = receive_udp(socket).await;
            let packet = Packet::decode(&bytes).expect("decode local simulator packet");
            acknowledgements.extend(packet.appended_acks().iter().copied());
            if let Message::PacketAck(message) = packet.message() {
                acknowledgements.extend(message.packets.iter().map(|ack| ack.id));
            }
            if expected(packet.message()) {
                return (packet, acknowledgements);
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
