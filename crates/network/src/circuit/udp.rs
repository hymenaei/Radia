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
pub(crate) mod test_values {
    use crate::fixtures::circuit::SendGate;

    use super::UdpTransport;

    pub(crate) fn set_send_gate(transport: &mut UdpTransport, send_gate: SendGate) {
        transport.send_gate = Some(send_gate);
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

    use tokio::net::UdpSocket;

    use super::{MAX_IPV4_UDP_PAYLOAD, MAX_IPV6_UDP_PAYLOAD, UdpTransport, local_bind_address};

    fn ipv6_is_disabled(error: &io::Error) -> bool {
        match error.raw_os_error() {
            #[cfg(target_os = "linux")]
            Some(97) => true,
            #[cfg(windows)]
            Some(10047) => true,
            #[cfg(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly",
                target_os = "solaris",
                target_os = "illumos"
            ))]
            Some(47) => true,
            _ => false,
        }
    }

    #[tokio::test]
    async fn ipv6_udp_round_trip() {
        let server = match UdpSocket::bind("[::1]:0").await {
            Ok(server) => server,
            Err(error) if ipv6_is_disabled(&error) => {
                eprintln!("Skipping IPv6 UDP test because IPv6 is disabled by the OS: {error}");
                return;
            }
            Err(error) => panic!("Could not bind IPv6 loopback UDP socket: {error}"),
        };
        let address = server.local_addr().expect("server local address");
        let transport = UdpTransport::connect(address).await.expect("connect IPv6 UDP transport");

        let local = transport.socket.local_addr().expect("transport local address");
        assert!(local.is_ipv6(), "expected IPv6 local address, got {local}");
        assert!(local_bind_address(address).ip().is_unspecified());
        assert!(local_bind_address(address).is_ipv6());

        transport.writable().await.expect("wait for IPv6 socket writability");
        transport.try_send(b"ipv6 request").expect("send IPv6 datagram");

        let mut buffer = [0; 32];
        let (received, peer) = server.recv_from(&mut buffer).await.expect("receive IPv6 request");
        assert_eq!(&buffer[..received], b"ipv6 request");
        assert!(peer.is_ipv6());

        server.send_to(b"ipv6 response", peer).await.expect("send IPv6 response");
        let received = transport.recv(&mut buffer).await.expect("receive IPv6 response");
        assert_eq!(&buffer[..received], b"ipv6 response");
    }

    #[tokio::test]
    async fn ipv4_datagram_limit() {
        let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind local IPv4 server");
        let transport = UdpTransport::connect(server.local_addr().expect("server address")).await.expect("connect IPv4 transport");

        let oversized = vec![0; MAX_IPV4_UDP_PAYLOAD + 1];
        let error = transport.try_send(&oversized).expect_err("oversized IPv4 datagram should fail before reaching the socket");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn ipv6_datagram_limit() {
        let server = match UdpSocket::bind("[::1]:0").await {
            Ok(server) => server,
            Err(error) if ipv6_is_disabled(&error) => {
                eprintln!("Skipping IPv6 UDP test because IPv6 is disabled by the OS: {error}");
                return;
            }
            Err(error) => panic!("Could not bind IPv6 loopback UDP socket: {error}"),
        };
        let transport = UdpTransport::connect(server.local_addr().expect("server address")).await.expect("connect IPv6 transport");

        let oversized = vec![0; MAX_IPV6_UDP_PAYLOAD + 1];
        let error = transport.try_send(&oversized).expect_err("oversized IPv6 datagram should fail before reaching the socket");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn bind_uses_remote_family() {
        let ipv4: SocketAddr = "127.0.0.1:9000".parse().expect("IPv4 address");
        let ipv6: SocketAddr = "[::1]:9000".parse().expect("IPv6 address");

        assert_eq!(local_bind_address(ipv4), SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0));
        assert_eq!(local_bind_address(ipv6), SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), 0));
    }
}
