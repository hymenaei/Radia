use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    thread,
    time::{Duration, Instant},
};

use crate::http::{Client, read_bounded_body, validate_http_url};
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
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "no request reached local HTTP server");
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("accept local HTTP request: {error}"),
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
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
                .filter_map(|line| std::str::from_utf8(line).ok())
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

#[tokio::test]
async fn redirects_are_not_followed() {
    let redirect = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:0/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let server = LocalServer::start(vec![redirect.as_bytes().to_vec()]);
    let client = Client::new(Some(Duration::from_secs(2)), false, None).expect("build HTTP client");

    let response = client.get(&server.url("/start")).await.expect("return redirect response without following it");
    let requests = server.finish();

    assert_eq!(response.status, reqwest::StatusCode::FOUND);
    assert_eq!(requests.len(), 1);
}

#[tokio::test]
async fn body_at_limit_is_accepted() {
    let server = LocalServer::start(vec![response(200, "OK", "text/plain", None, "12345")]);
    let response = reqwest::Client::new().get(server.url("/body")).send().await.expect("receive local HTTP response");

    assert_eq!(read_bounded_body(response, 5).await.expect("body at limit"), b"12345");
    server.finish();
}

#[tokio::test]
async fn content_length_is_bounded() {
    let server = LocalServer::start(vec![response(200, "OK", "text/plain", None, "123456")]);
    let response = reqwest::Client::new().get(server.url("/body")).send().await.expect("receive local HTTP response");

    let error = read_bounded_body(response, 5).await.expect_err("declared body exceeds limit");
    assert!(error.to_string().contains("5-byte limit"));
    server.finish();
}

#[tokio::test]
async fn chunked_body_limit() {
    let response = concat!(
        "HTTP/1.1 200 OK\r\n",
        "Transfer-Encoding: chunked\r\n",
        "Connection: close\r\n\r\n",
        "3\r\nabc\r\n",
        "3\r\ndef\r\n",
        "0\r\n\r\n"
    );
    let server = LocalServer::start(vec![response.as_bytes().to_vec()]);
    let response = reqwest::Client::new().get(server.url("/body")).send().await.expect("receive local HTTP response");

    let error = read_bounded_body(response, 5).await.expect_err("chunked body exceeds limit");
    assert!(error.to_string().contains("5-byte limit"));
    server.finish();
}

#[tokio::test]
async fn post_limit_is_enforced() {
    let server = LocalServer::start(vec![response(200, "OK", "text/plain", None, "123456")]);
    let client = Client::new(Some(Duration::from_secs(2)), false, None).expect("build HTTP client");

    let error = match client.post_bounded(&server.url("/login"), "text/xml", None, Vec::new(), 5).await {
        Err(error) => error,
        Ok(_) => panic!("login response exceeded the configured limit"),
    };

    assert!(error.contains("5-byte limit"));
    assert_eq!(server.finish().len(), 1);
}

#[tokio::test]
async fn transport_errors_hide_urls() {
    let url = "http://127.0.0.1:0/cap?token=secret-token";
    let client = Client::new(Some(Duration::from_secs(2)), false, None).expect("build HTTP client");

    let error = match client.get(&Url::parse(url).expect("parse capability URL")).await {
        Err(error) => error,
        Ok(_) => panic!("closed port should fail"),
    };

    assert!(!error.contains("secret-token"));
    assert!(!error.contains(url));
}

#[test]
fn unsafe_urls_are_rejected() {
    for url in [
        Url::parse("ftp://example.com/cap").expect("parse FTP URL"),
        Url::parse("http://user:password@example.com/cap").expect("parse credential URL"),
        Url::parse("https://example.com/cap#fragment").expect("parse fragment URL"),
        Url::parse("file:///tmp/cap").expect("parse file URL"),
    ] {
        assert!(validate_http_url(&url).is_err(), "accepted {url}");
    }

    for url in [
        Url::parse("http://127.0.0.1:9000/cap?token=opaque").expect("parse local HTTP URL"),
        Url::parse("https://caps.example.net:8443/arbitrary/path").expect("parse HTTPS URL"),
    ] {
        assert!(validate_http_url(&url).is_ok(), "rejected {url}");
    }
}
