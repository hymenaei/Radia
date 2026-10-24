#[cfg(test)]
pub(crate) mod test_values {
    use crate::fixtures::circuit::SendGate;

    use super::UdpTransport;

    pub(crate) fn set_send_gate(transport: &mut UdpTransport, send_gate: SendGate) {
        transport.send_gate = Some(send_gate);
    }
}

use std::{
    error::Error as StdError,
    fmt,
    io::{self, ErrorKind},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
};

#[cfg(test)]
use crate::fixtures::circuit::SendGate;
use tokio::net::UdpSocket;

const MAX_IPV4_UDP_PAYLOAD: usize = u16::MAX as usize - 20 - 8;
const MAX_IPV6_UDP_PAYLOAD: usize = u16::MAX as usize - 8;

#[derive(Clone, Debug)]
pub(crate) enum UdpError {
    Bind(Arc<io::Error>),
    Connect(Arc<io::Error>),
}

impl fmt::Display for UdpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bind(error) => write!(formatter, "Could not open the simulator connection: {error}"),
            Self::Connect(error) => write!(formatter, "Could not reach the simulator: {error}"),
        }
    }
}

impl StdError for UdpError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Bind(error) | Self::Connect(error) => Some(error.as_ref()),
        }
    }
}

pub(crate) struct UdpTransport {
    socket: UdpSocket,
    max_payload_size: usize,
    #[cfg(test)]
    send_gate: Option<SendGate>,
}

impl UdpTransport {
    pub(crate) async fn connect(address: SocketAddr) -> Result<Self, UdpError> {
        let socket = UdpSocket::bind(local_bind_address(address)).await.map_err(|error| UdpError::Bind(Arc::new(error)))?;
        socket.connect(address).await.map_err(|error| UdpError::Connect(Arc::new(error)))?;
        let max_payload_size = if address.is_ipv4() { MAX_IPV4_UDP_PAYLOAD } else { MAX_IPV6_UDP_PAYLOAD };
        Ok(Self {
            socket,
            max_payload_size,
            #[cfg(test)]
            send_gate: None,
        })
    }

    pub(crate) async fn writable(&self) -> Result<(), io::Error> {
        #[cfg(test)]
        if let Some(send_gate) = &self.send_gate {
            let mut paused = send_gate.paused.clone();
            while *paused.borrow() {
                send_gate.entered.notify_one();
                paused.changed().await.map_err(|_| io::Error::new(ErrorKind::BrokenPipe, "UDP test send gate closed"))?;
            }
        }
        self.socket.writable().await
    }

    pub(crate) fn try_send(&self, bytes: &[u8]) -> Result<(), io::Error> {
        if bytes.len() > self.max_payload_size {
            return Err(io::Error::new(ErrorKind::InvalidInput, "UDP datagram exceeds the address family's maximum payload size."));
        }
        #[cfg(test)]
        if let Some(send_gate) = &self.send_gate
            && *send_gate.paused.borrow()
        {
            send_gate.entered.notify_one();
            return Err(io::Error::from(ErrorKind::WouldBlock));
        }
        let sent = self.socket.try_send(bytes)?;
        if sent != bytes.len() {
            return Err(io::Error::new(ErrorKind::WriteZero, "UDP socket sent an incomplete datagram."));
        }
        Ok(())
    }

    pub(crate) async fn recv(&self, buffer: &mut [u8]) -> Result<usize, io::Error> {
        let received = self.socket.recv(buffer).await?;
        Ok(received)
    }
}

fn local_bind_address(remote: SocketAddr) -> SocketAddr {
    match remote {
        SocketAddr::V4(_) => SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0),
        SocketAddr::V6(_) => SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), 0),
    }
}

#[cfg(test)]
#[path = "tests/udp.rs"]
mod tests;
