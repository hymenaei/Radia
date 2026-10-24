use std::time::{Duration, UNIX_EPOCH};

use reqwest::{
    StatusCode,
    header::{HeaderMap, HeaderValue},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net,
    task::JoinHandle,
};
use url::Url;

use crate::fixtures::http::{LocalServer, response};

use super::{Client, ErrorKind, RETRY_AFTER, Response, retry_after_delay, validate_http_url};

#[test]
fn retry_after_formats() {
    assert_eq!(retry_after_delay("120", UNIX_EPOCH), Some(Duration::from_secs(120)));

    let now = UNIX_EPOCH + Duration::from_secs(784_111_700);
    for value in ["Sun, 06 Nov 1994 08:49:37 GMT", "Sunday, 06-Nov-94 08:49:37 GMT", "Sun Nov  6 08:49:37 1994"] {
        assert_eq!(retry_after_delay(value, now), Some(Duration::from_secs(77)));
    }
}

#[test]
fn invalid_retry_after_is_ignored() {
    assert_eq!(retry_after_delay("soon", UNIX_EPOCH), None);
    assert_eq!(retry_after_delay("", UNIX_EPOCH), None);
}

#[test]
fn response_retry_after() {
    let mut headers = HeaderMap::new();
    headers.insert(RETRY_AFTER, HeaderValue::from_static("0"));
    let response = Response { status: StatusCode::TOO_MANY_REQUESTS, headers, body: Vec::new(), elapsed: Duration::ZERO };

    assert_eq!(response.retry_after(), Some(Duration::ZERO));
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let redirect = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:0/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let server = LocalServer::start(vec![redirect.as_bytes().to_vec()]);
    let client = Client::new(Some(Duration::from_secs(2)), false, None).expect("build HTTP client");

    let response = client.get("test", &server.url("/start"), 10).await.expect("return redirect response without following it");
    let requests = server.finish();

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(requests.len(), 1);
}

#[tokio::test]
async fn body_at_limit_is_accepted() {
    let server = LocalServer::start(vec![response(200, "OK", "text/plain", None, "12345")]);
    let client = Client::new(Some(Duration::from_secs(2)), false, None).expect("build HTTP client");

    let response = client.get("test", &server.url("/body"), 5).await.expect("body at limit");

    assert_eq!(response.body, b"12345");
    server.finish();
}

#[tokio::test]
async fn content_length_is_bounded_before_reading() {
    let server = LocalServer::start(vec![response(200, "OK", "text/plain", None, "123456")]);
    let client = Client::new(Some(Duration::from_secs(2)), false, None).expect("build HTTP client");

    let error = client.get("test", &server.url("/body"), 5).await.expect_err("declared body exceeds limit");

    assert_eq!(error.kind, ErrorKind::ResourceExhausted);
    assert_eq!(error.status, Some(StatusCode::OK));
    assert!(error.to_string().contains("5-byte limit"));
    assert_eq!(server.finish().len(), 1);
}

#[tokio::test]
async fn chunked_body_limit() {
    let wire_response = concat!(
        "HTTP/1.1 200 OK\r\n",
        "Transfer-Encoding: chunked\r\n",
        "Connection: close\r\n\r\n",
        "3\r\nabc\r\n",
        "3\r\ndef\r\n",
        "0\r\n\r\n"
    );
    let server = LocalServer::start(vec![wire_response.as_bytes().to_vec()]);
    let client = Client::new(Some(Duration::from_secs(2)), false, None).expect("build HTTP client");

    let error = client.get("test", &server.url("/body"), 5).await.expect_err("chunked body exceeds limit");

    assert_eq!(error.kind, ErrorKind::ResourceExhausted);
    assert_eq!(error.status, Some(StatusCode::OK));
    assert!(error.to_string().contains("5-byte limit"));
    server.finish();
}

#[tokio::test]
async fn gzip_content_encoding_remains_raw() {
    const GZIP_BODY: &[u8] = &[
        0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0a, 0xab, 0xa8, 0x18, 0x05, 0xa3, 0x60, 0x14, 0x54, 0x8c, 0x50, 0x00, 0x00,
        0x63, 0xf0, 0xd7, 0x48, 0x00, 0x04, 0x00, 0x00,
    ];
    let mut wire_response =
        format!("HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", GZIP_BODY.len())
            .into_bytes();
    wire_response.extend_from_slice(GZIP_BODY);
    let server = LocalServer::start(vec![wire_response]);
    let client = Client::new(Some(Duration::from_secs(2)), false, None).expect("build HTTP client");

    let response = client.get("test", &server.url("/compressed"), GZIP_BODY.len()).await.expect("read raw gzip bytes");

    assert_eq!(response.body, GZIP_BODY);
    assert_eq!(server.finish().len(), 1);
}

#[tokio::test]
async fn truncated_body_keeps_response_metadata() {
    let wire_response = concat!("HTTP/1.1 200 OK\r\n", "Content-Length: 6\r\n", "Retry-After: 2\r\n", "Connection: close\r\n\r\n", "abc");
    let server = LocalServer::start(vec![wire_response.as_bytes().to_vec()]);
    let client = Client::new(Some(Duration::from_secs(2)), false, None).expect("build HTTP client");

    let error = client.get("test", &server.url("/body"), 6).await.expect_err("truncated Content-Length response must fail");

    assert_eq!(error.kind, ErrorKind::Transport);
    assert_eq!(error.status, Some(StatusCode::OK));
    assert_eq!(error.retry_after, Some(Duration::from_secs(2)));
    assert!(error.elapsed > Duration::ZERO && error.elapsed < Duration::from_secs(2));
    assert_eq!(server.finish().len(), 1);
}

#[tokio::test]
async fn operation_deadline_covers_body_reading() {
    let listener = net::TcpListener::bind("127.0.0.1:0").await.expect("bind local HTTP server");
    let address = listener.local_addr().expect("read local listener address");
    let (headers_sent, headers_received) = tokio::sync::oneshot::channel();
    let (release_body, body_released) = tokio::sync::oneshot::channel();
    let server: JoinHandle<()> = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept HTTP request");
        let mut request = Vec::new();
        let mut chunk = [0; 1024];
        loop {
            let size = stream.read(&mut chunk).await.expect("read HTTP request");
            if size == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..size]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n").await.expect("write headers");
        headers_sent.send(()).expect("request should await response headers");
        body_released.await.expect("allow body deadline to expire");
        if let Err(error) = stream.write_all(b"data").await {
            std::assert_matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted,
                "write response body after the client deadline: {error}"
            );
        }
    });
    let client = Client::new(Some(Duration::from_millis(250)), false, None).expect("build HTTP client");
    let url = Url::parse(&format!("http://{address}/body")).expect("local HTTP URL");
    let request = tokio::spawn(async move { client.get("test", &url, 16).await });
    headers_received.await.expect("server sent response headers");
    let error = request.await.expect("request task should finish").expect_err("body read must hit its deadline");

    assert_eq!(error.kind, ErrorKind::Timeout);
    assert_eq!(error.status, Some(StatusCode::OK));
    assert!(!error.is_timeout_after(Duration::from_millis(250)), "a body timeout has response metadata");
    release_body.send(()).expect("release held response body");
    server.await.expect("server should finish");
}

#[tokio::test]
async fn post_limit_is_enforced() {
    let server = LocalServer::start(vec![response(200, "OK", "text/plain", None, "123456")]);
    let client = Client::new(Some(Duration::from_secs(2)), false, None).expect("build HTTP client");

    let error = client
        .post("login", &server.url("/login"), "text/xml", None, Vec::new(), 5)
        .await
        .expect_err("login response exceeded the configured limit");

    assert!(error.to_string().contains("5-byte limit"));
    assert_eq!(server.finish().len(), 1);
}

#[tokio::test]
async fn transport_errors_hide_urls() {
    let url = "http://127.0.0.1:0/cap?token=secret-token";
    let client = Client::new(Some(Duration::from_secs(2)), false, None).expect("build HTTP client");

    let error = client.get("test", &Url::parse(url).expect("parse capability URL"), 10).await.expect_err("closed port should fail");

    assert!(!error.to_string().contains("secret-token"));
    assert!(!format!("{error:?}").contains("secret-token"));
    assert!(!format!("{error:?}").contains(url));
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

#[test]
fn response_debug_hides_private_details() {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", HeaderValue::from_static("Bearer header-secret"));
    headers.insert("cookie", HeaderValue::from_static("session=cookie-secret"));
    let response = Response { status: StatusCode::OK, headers, body: b"body-session-secret".to_vec(), elapsed: Duration::from_millis(12) };

    let debug = format!("{response:?}");

    for secret in ["authorization", "header-secret", "cookie", "cookie-secret", "body-session-secret"] {
        assert!(!debug.contains(secret));
    }
    assert!(debug.contains("body_bytes"));
    assert!(debug.contains("header_count"));
}
