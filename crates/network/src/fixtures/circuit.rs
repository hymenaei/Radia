use std::{io::ErrorKind, sync::Arc};

use protocol::{
    lludp::{Message, Packet, PacketPreflight},
    messages::{CompletePingCheck, PingId, StartPingCheck},
};
use tokio::{
    net::UdpSocket,
    sync::{Notify, OwnedSemaphorePermit, watch},
    time::Instant,
};

use crate::circuit::{
    UdpTransport,
    admission::{PacketMemoryError, PacketMemoryReservation, SimulatorMemoryBudget},
    lludp::{Circuit, CircuitError, Delivery, PreparedDatagram, ReceiveError, ReceiveOutcome},
};

pub(crate) use crate::circuit::{NEW_RELIABLE_ACK_CAPACITY, pacer_rate, set_send_gate, with_sequence};

pub(crate) trait SimulatorMemoryBudgetTestExt {
    fn with_test_limits(global: usize, neighbors: usize, neighbor_connection: usize) -> Self
    where
        Self: Sized;
    fn reserve_bytes_for_test(&self, bytes: usize) -> Result<PacketMemoryReservation, PacketMemoryError>;
    fn reserve_packet_for_test(&self, preflight: PacketPreflight) -> Result<PacketMemoryReservation, PacketMemoryError>;
    fn trim_packet_reservation_for_test(&self, reservation: &mut PacketMemoryReservation, packet: &Packet)
    -> Result<(), PacketMemoryError>;
    fn decode_slots_available(&self) -> usize;
    async fn hold_all_decode_slots(&self) -> OwnedSemaphorePermit;
    fn global_available_bytes(&self) -> usize;
    fn neighbor_available_bytes(&self) -> usize;
    fn connection_available_bytes(&self) -> usize;
}

impl SimulatorMemoryBudgetTestExt for SimulatorMemoryBudget {
    fn with_test_limits(global: usize, neighbors: usize, neighbor_connection: usize) -> Self {
        crate::circuit::admission::test_values::with_test_limits(global, neighbors, neighbor_connection)
    }

    fn reserve_bytes_for_test(&self, bytes: usize) -> Result<PacketMemoryReservation, PacketMemoryError> {
        crate::circuit::admission::test_values::reserve_bytes_for_test(self, bytes)
    }

    fn reserve_packet_for_test(&self, preflight: PacketPreflight) -> Result<PacketMemoryReservation, PacketMemoryError> {
        crate::circuit::admission::test_values::reserve_packet_for_test(self, preflight)
    }

    fn trim_packet_reservation_for_test(
        &self,
        reservation: &mut PacketMemoryReservation,
        packet: &Packet,
    ) -> Result<(), PacketMemoryError> {
        crate::circuit::admission::test_values::trim_packet_reservation_for_test(self, reservation, packet)
    }

    fn decode_slots_available(&self) -> usize {
        crate::circuit::admission::test_values::decode_slots_available_for_test(self)
    }

    async fn hold_all_decode_slots(&self) -> OwnedSemaphorePermit {
        crate::circuit::admission::test_values::hold_all_decode_slots_for_test(self).await
    }

    fn global_available_bytes(&self) -> usize {
        crate::circuit::admission::test_values::global_available_bytes(self)
    }

    fn neighbor_available_bytes(&self) -> usize {
        crate::circuit::admission::test_values::neighbor_available_bytes(self)
    }

    fn connection_available_bytes(&self) -> usize {
        crate::circuit::admission::test_values::connection_available_bytes(self)
    }
}

pub(crate) struct SendGate {
    pub(crate) paused: watch::Receiver<bool>,
    pub(crate) entered: Arc<Notify>,
}

pub(crate) fn new_send_gate() -> (SendGate, watch::Sender<bool>, Arc<Notify>) {
    let (paused, receiver) = watch::channel(false);
    let entered = Arc::new(Notify::new());
    (SendGate { paused: receiver, entered: Arc::clone(&entered) }, paused, entered)
}

pub(crate) async fn send_message(
    circuit: &mut Circuit,
    socket: &UdpTransport,
    message: Message,
    delivery: Delivery,
) -> Result<u32, CircuitError> {
    let prepared = circuit.prepare_message(message, delivery)?;
    let sequence = prepared.sequence();
    send_prepared_at(circuit, socket, prepared, Instant::now()).await?;
    Ok(sequence)
}

pub(crate) async fn connected_simulator() -> (UdpSocket, UdpTransport) {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    (simulator, transport)
}

pub(crate) fn complete_ping(ping_id: u8) -> Message {
    Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id } })
}

pub(crate) fn start_ping(ping_id: u8, oldest_unacked: u32) -> Message {
    Message::StartPingCheck(StartPingCheck { ping_id: PingId { ping_id }, oldest_unacked })
}

pub(crate) async fn receive_packet(socket: &UdpSocket, buffer: &mut [u8]) -> (usize, Packet) {
    let (size, _) = socket.recv_from(buffer).await.expect("receive simulator datagram");
    let packet = Packet::decode(&buffer[..size]).expect("decode simulator packet");
    (size, packet)
}

pub(crate) async fn send_ping_at(circuit: &mut Circuit, socket: &UdpTransport, sent_at: impl Into<Instant>) -> Result<(), CircuitError> {
    let sent_at = sent_at.into();
    let Some(prepared) = circuit.prepare_ping(sent_at)? else {
        return Ok(());
    };
    send_prepared_at(circuit, socket, prepared, sent_at).await
}

pub(crate) fn receive_at(circuit: &mut Circuit, packet: &Packet, received_at: impl Into<Instant>) -> bool {
    match receive_packet_at(circuit, packet, received_at) {
        Ok(outcome) => outcome.is_new_packet,
        Err(error) => panic!("valid circuit packet {} failed receive admission: {error}", packet.sequence()),
    }
}

pub(crate) fn reliable_packet(sequence: u32) -> Packet {
    Packet::new(sequence, complete_ping(1)).with_reliable()
}

pub(crate) fn resent_reliable_packet(sequence: u32) -> Packet {
    reliable_packet(sequence).with_resent()
}

pub(crate) async fn send_acknowledgements(circuit: &mut Circuit, socket: &UdpTransport) -> Result<(), CircuitError> {
    while let Some(prepared) = circuit.prepare_acknowledgements()? {
        send_prepared_at(circuit, socket, prepared, Instant::now()).await?;
    }
    Ok(())
}

pub(crate) async fn send_prepared_at(
    circuit: &mut Circuit,
    socket: &UdpTransport,
    prepared: PreparedDatagram,
    sent_at: impl Into<Instant>,
) -> Result<(), CircuitError> {
    let sent_at = sent_at.into();
    loop {
        match socket.try_send(prepared.bytes()) {
            Ok(()) => break,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                socket.writable().await.expect("local UDP socket should become writable");
            }
            Err(error) => panic!("could not send a prepared test datagram: {error}"),
        }
    }
    circuit.commit_sent(prepared, sent_at)
}

pub(crate) fn receive_packet_at(
    circuit: &mut Circuit,
    packet: &Packet,
    received_at: impl Into<Instant>,
) -> Result<ReceiveOutcome, ReceiveError> {
    let admission =
        circuit.prepare_receive(packet.sequence(), packet.is_reliable(), packet.appended_acks().iter().copied(), received_at.into())?;
    Ok(circuit.commit_received_packet(admission, packet))
}
