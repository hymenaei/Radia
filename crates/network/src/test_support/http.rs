use std::{
    io::{ErrorKind, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    str::from_utf8,
    thread,
    time::{Duration, Instant},
};

use url::Url;

pub(crate) struct LocalServer {
    address: SocketAddr,
    worker: thread::JoinHandle<Vec<Vec<u8>>>,
}

impl LocalServer {
    pub(crate) fn start(responses: Vec<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local HTTP server");
        listener.set_nonblocking(true).expect("set local listener nonblocking");
        let address = listener.local_addr().expect("read local listener address");
        let worker = thread::spawn(move || {
            responses
                .into_iter()
                .map(|response| {
                    let (mut stream, _) = accept_with_timeout(&listener);
                    stream.set_nonblocking(false).expect("set local stream blocking");
                    stream.set_read_timeout(Some(Duration::from_secs(5))).expect("set local request timeout");
                    let request = read_request(&mut stream);
                    stream.write_all(&response).expect("write local response");
                    request
                })
                .collect()
        });

        Self { address, worker }
    }

    pub(crate) fn start_delayed_body(response: Vec<u8>, delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local HTTP server");
        listener.set_nonblocking(true).expect("set local listener nonblocking");
        let address = listener.local_addr().expect("read local listener address");
        let worker = thread::spawn(move || {
            let (mut stream, _) = accept_with_timeout(&listener);
            stream.set_nonblocking(false).expect("set local stream blocking");
            stream.set_read_timeout(Some(Duration::from_secs(5))).expect("set local request timeout");
            let request = read_request(&mut stream);
            let body_start = header_body_start(&response).expect("response headers are terminated") + 4;
            stream.write_all(&response[..body_start]).expect("write local response headers");
            thread::sleep(delay);
            stream.write_all(&response[body_start..]).expect("write delayed local response body");
            vec![request]
        });

        Self { address, worker }
    }

    pub(crate) fn url(&self, path: &str) -> Url {
        Url::parse(&format!("http://{}{path}", self.address)).expect("valid local HTTP URL")
    }

    pub(crate) fn finish(self) -> Vec<Vec<u8>> {
        self.worker.join().expect("local HTTP server should finish")
    }
}

pub(crate) fn response(status: u16, reason: &str, content_type: &str, retry_after: Option<&str>, body: &str) -> Vec<u8> {
    let retry_after = retry_after.map(|value| format!("Retry-After: {value}\r\n")).unwrap_or_default();
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n{retry_after}\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn accept_with_timeout(listener: &TcpListener) -> (TcpStream, SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok(connection) => return connection,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "no request reached local HTTP server");
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("accept local HTTP request: {error}"),
        }
    }
}

pub(crate) fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let size = stream.read(&mut chunk).expect("read local HTTP request");
        if size == 0 {
            return request;
        }
        request.extend_from_slice(&chunk[..size]);
        if let Some(body_start) = header_body_start(&request) {
            let content_length = request[..body_start]
                .split(|byte| *byte == b'\n')
                .filter_map(|line| from_utf8(line).ok())
                .find_map(|line| {
                    let (name, value) = line.trim().split_once(':')?;
                    name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
                })
                .unwrap_or(0);
            if request.len() >= body_start + 4 + content_length {
                return request;
            }
        }
    }
}

fn header_body_start(request: &[u8]) -> Option<usize> {
    request.windows(4).position(|window| window == b"\r\n\r\n")
}
