use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use tokio::net::UdpSocket;

use crate::{capture::CaptureRecorder, status::NetworkStatusHandle};

const MAX_IPV4_UDP_PAYLOAD: usize = u16::MAX as usize - 20 - 8;
const MAX_IPV6_UDP_PAYLOAD: usize = u16::MAX as usize - 8;

pub(crate) struct UdpTransport {
    socket: UdpSocket,
    max_payload_size: usize,
    local_address: SocketAddr,
    remote_address: SocketAddr,
    circuit_id: u32,
    capture: Option<CaptureRecorder>,
    status: NetworkStatusHandle,
}

impl UdpTransport {
    #[cfg(test)]
    pub(crate) async fn connect(address: SocketAddr) -> Result<Self, String> {
        Self::connect_with_status(address, 0, None, NetworkStatusHandle::default()).await
    }

    pub(crate) async fn connect_with_status(
        address: SocketAddr,
        circuit_id: u32,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
    ) -> Result<Self, String> {
        let socket = UdpSocket::bind(local_bind_address(address))
            .await
            .map_err(|error| format!("Could not open the simulator connection: {error}"))?;
        socket.connect(address).await.map_err(|error| format!("Could not reach the simulator: {error}"))?;
        let max_payload_size = if address.is_ipv4() { MAX_IPV4_UDP_PAYLOAD } else { MAX_IPV6_UDP_PAYLOAD };
        let local_address = socket.local_addr().map_err(|error| format!("Could not inspect the simulator connection: {error}"))?;
        Ok(Self { socket, max_payload_size, local_address, remote_address: address, circuit_id, capture, status })
    }

    #[tracing::instrument(
        name = "network.udp.socket_send", level = "debug", skip_all,
        fields(datagram_bytes = bytes.len())
    )]
    pub(crate) async fn send(&self, bytes: &[u8]) -> Result<(), std::io::Error> {
        if bytes.len() > self.max_payload_size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "UDP datagram exceeds the address family's maximum payload size.",
            ));
        }
        let sent = self.socket.send(bytes).await?;
        if sent != bytes.len() {
            return Err(std::io::Error::new(std::io::ErrorKind::WriteZero, "UDP socket sent an incomplete datagram."));
        }
        self.status.note_udp_sent();
        if let Some(capture) = &self.capture {
            capture.udp(self.circuit_id, "out", self.local_address, self.remote_address, bytes);
        }
        Ok(())
    }

    pub(crate) async fn recv(&self, buffer: &mut [u8]) -> Result<usize, std::io::Error> {
        let received = self.socket.recv(buffer).await?;
        self.status.note_udp_received();
        if let Some(capture) = &self.capture {
            capture.udp(self.circuit_id, "in", self.local_address, self.remote_address, &buffer[..received]);
        }
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
mod tests {
    use super::{MAX_IPV4_UDP_PAYLOAD, MAX_IPV6_UDP_PAYLOAD, UdpTransport, local_bind_address};
    use std::io;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
    use tokio::net::UdpSocket;
    use tokio::time::{Duration, timeout};

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

        transport.send(b"ipv6 request").await.expect("send IPv6 datagram");

        let mut buffer = [0; 32];
        let (received, peer) = timeout(Duration::from_secs(5), server.recv_from(&mut buffer))
            .await
            .expect("timed out receiving IPv6 request")
            .expect("receive IPv6 request");
        assert_eq!(&buffer[..received], b"ipv6 request");
        assert!(peer.is_ipv6());

        server.send_to(b"ipv6 response", peer).await.expect("send IPv6 response");
        let received = timeout(Duration::from_secs(5), transport.recv(&mut buffer))
            .await
            .expect("timed out receiving IPv6 response")
            .expect("receive IPv6 response");
        assert_eq!(&buffer[..received], b"ipv6 response");
    }

    #[tokio::test]
    async fn ipv4_datagram_limit() {
        let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind local IPv4 server");
        let transport = UdpTransport::connect(server.local_addr().expect("server address")).await.expect("connect IPv4 transport");

        let oversized = vec![0; MAX_IPV4_UDP_PAYLOAD + 1];
        let error = transport.send(&oversized).await.expect_err("oversized IPv4 datagram should fail before reaching the socket");

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
        let error = transport.send(&oversized).await.expect_err("oversized IPv6 datagram should fail before reaching the socket");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn bind_uses_remote_family() {
        let ipv4: SocketAddr = "127.0.0.1:9000".parse().expect("IPv4 address");
        let ipv6: SocketAddr = "[::1]:9000".parse().expect("IPv6 address");

        assert_eq!(local_bind_address(ipv4), SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0));
        assert_eq!(local_bind_address(ipv6), SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), 0));
    }

    fn ipv6_is_disabled(error: &io::Error) -> bool {
        match error.raw_os_error() {
            #[cfg(target_os = "linux")]
            Some(97) => true, // EAFNOSUPPORT
            #[cfg(windows)]
            Some(10047) => true, // WSAEAFNOSUPPORT
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
            Some(47) => true, // EAFNOSUPPORT
            _ => false,
        }
    }
}
