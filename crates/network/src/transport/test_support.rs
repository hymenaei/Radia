use std::io::ErrorKind;

use protocol::lludp::{Message, Packet};
use tokio::time::Instant;

use super::{
    UdpTransport,
    lludp::{Circuit, CircuitError, Delivery, PreparedDatagram, ReceiveError, ReceiveOutcome},
};

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
    sent_at: Instant,
) -> Result<(), CircuitError> {
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

pub(crate) fn receive_packet_at(circuit: &mut Circuit, packet: &Packet, received_at: Instant) -> Result<ReceiveOutcome, ReceiveError> {
    let admission =
        circuit.prepare_receive(packet.sequence(), packet.is_reliable(), packet.appended_acks().iter().copied(), received_at)?;
    Ok(circuit.commit_received_packet(admission, packet))
}
