use std::{
    error,
    fmt::{self, Debug, Display, Formatter},
    future::Future,
    time::{Duration, Instant, SystemTime},
};

use reqwest::{
    RequestBuilder, StatusCode, Url,
    header::{ACCEPT, CONTENT_TYPE, HeaderMap, RETRY_AFTER},
    redirect::Policy,
};
use tokio::time;

#[derive(Clone)]
pub(crate) struct Client {
    client: reqwest::Client,
    timeout: Option<Duration>,
}

impl Client {
    pub(crate) fn new(timeout: Option<Duration>, http1_only: bool, user_agent: Option<&str>) -> Result<Self, reqwest::Error> {
        let mut builder = reqwest::Client::builder().redirect(Policy::none());
        if http1_only {
            builder = builder.http1_only();
        }
        if let Some(user_agent) = user_agent {
            builder = builder.user_agent(user_agent);
        }
        builder.build().map(|client| Self { client, timeout })
    }

    pub(crate) fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }

    pub(crate) async fn get(&self, operation: &str, url: &Url, max_response_bytes: usize) -> Result<Response, Error> {
        validate_http_url(url)?;
        self.execute(operation, self.client.get(url.clone()), max_response_bytes).await
    }

    pub(crate) async fn post(
        &self,
        operation: &str,
        url: &Url,
        content_type: &str,
        accept: Option<&str>,
        body: Vec<u8>,
        max_response_bytes: usize,
    ) -> Result<Response, Error> {
        validate_http_url(url)?;
        let mut request = self.client.post(url.clone()).header(CONTENT_TYPE, content_type).body(body);
        if let Some(accept) = accept {
            request = request.header(ACCEPT, accept);
        }
        self.execute(operation, request, max_response_bytes).await
    }

    #[tracing::instrument(
        name = "http.execute", level = "debug", skip_all,
        fields(operation = %operation, response_limit = max_response_bytes)
    )]
    async fn execute(&self, operation: &str, mut request: RequestBuilder, max_response_bytes: usize) -> Result<Response, Error> {
        let started = Instant::now();
        let deadline = self.timeout.map(|timeout| time::Instant::now() + timeout);
        if let Some(deadline) = deadline {
            request = request.timeout(deadline.saturating_duration_since(time::Instant::now()));
        }
        let response = match send_with_deadline(request.send(), deadline, started).await {
            Ok(response) => response,
            Err(error) => {
                tracing::debug!(operation, error = %error, elapsed_ms = started.elapsed().as_millis(), "HTTP request failed");
                return Err(error);
            }
        };
        let status = response.status();
        let headers = response.headers().clone();
        let retry_after = response_retry_after(&headers);
        let body = match read_bounded_body(response, max_response_bytes, deadline, started).await {
            Ok(body) => body,
            Err(error) => {
                tracing::debug!(
                    operation,
                    http_status = status.as_u16(),
                    elapsed_ms = started.elapsed().as_millis(),
                    "HTTP response body rejected"
                );
                return Err(error.with_response(status, retry_after, started.elapsed()));
            }
        };
        let elapsed = started.elapsed();
        tracing::debug!(
            operation,
            http_status = status.as_u16(),
            response_bytes = body.len(),
            elapsed_ms = elapsed.as_millis(),
            "HTTP response received"
        );
        Ok(Response { status, headers, body, elapsed })
    }
}

pub(crate) struct Response {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Vec<u8>,
    pub(crate) elapsed: Duration,
}

impl Response {
    pub(crate) fn retry_after(&self) -> Option<Duration> {
        let value = self.headers.get(RETRY_AFTER)?.to_str().ok()?;
        retry_after_delay(value, SystemTime::now())
    }
}

impl Debug for Response {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Response")
            .field("status", &self.status)
            .field("header_count", &self.headers.len())
            .field("body_bytes", &self.body.len())
            .field("elapsed", &self.elapsed)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    Transport,
    Timeout,
    Cancelled,
    ResourceExhausted,
    HttpStatus,
    InvalidUrl,
}

#[non_exhaustive]
pub struct Error {
    pub kind: ErrorKind,
    pub elapsed: Duration,
    pub status: Option<StatusCode>,
    pub retry_after: Option<Duration>,
    message: String,
}

impl Error {
    fn new(kind: ErrorKind, elapsed: Duration, status: Option<StatusCode>, retry_after: Option<Duration>, message: String) -> Self {
        Self { kind, elapsed, status, retry_after, message }
    }

    fn request(error: reqwest::Error, elapsed: Duration) -> Self {
        let kind = if error.is_timeout() { ErrorKind::Timeout } else { ErrorKind::Transport };
        Self::new(kind, elapsed, None, None, error.without_url().to_string())
    }

    pub(crate) fn resource_exhausted(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::ResourceExhausted, Duration::ZERO, None, None, message.into())
    }

    pub(crate) fn timeout(elapsed: Duration, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Timeout, elapsed, None, None, message.into())
    }

    fn deadline(started: Instant, status: Option<StatusCode>, retry_after: Option<Duration>, message: &str) -> Self {
        Self::new(ErrorKind::Timeout, started.elapsed(), status, retry_after, message.into())
    }

    fn with_response(mut self, status: StatusCode, retry_after: Option<Duration>, elapsed: Duration) -> Self {
        self.status = Some(status);
        self.retry_after = retry_after;
        self.elapsed = elapsed;
        self
    }

    pub(crate) fn is_timeout_after(&self, minimum: Duration) -> bool {
        self.kind == ErrorKind::Timeout && self.status.is_none() && self.elapsed >= minimum
    }

    pub(crate) fn is_resource_exhausted(&self) -> bool {
        self.kind == ErrorKind::ResourceExhausted
    }

    pub(crate) fn http_status(status: StatusCode, elapsed: Duration, retry_after: Option<Duration>, context: &str) -> Self {
        Self::new(ErrorKind::HttpStatus, elapsed, Some(status), retry_after, format!("{context}: HTTP {status}"))
    }
}

impl Debug for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Error")
            .field("kind", &self.kind)
            .field("elapsed", &self.elapsed)
            .field("status", &self.status)
            .field("retry_after", &self.retry_after)
            .field("message", &"[REDACTED]")
            .finish()
    }
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl error::Error for Error {}

fn retry_after_delay(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        let seconds = value.parse::<u64>().unwrap_or(u64::MAX);
        return Some(Duration::from_secs(seconds));
    }

    let retry_at = httpdate::parse_http_date(value).ok()?;
    Some(retry_at.duration_since(now).unwrap_or_default())
}

fn response_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?;
    retry_after_delay(value, SystemTime::now())
}

async fn send_with_deadline<F>(send: F, deadline: Option<time::Instant>, started: Instant) -> Result<reqwest::Response, Error>
where
    F: Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    let result = if let Some(deadline) = deadline {
        time::timeout_at(deadline, send)
            .await
            .map_err(|_| Error::deadline(started, None, None, "The HTTP operation exceeded its deadline."))?
    } else {
        send.await
    };
    result.map_err(|error| Error::request(error, started.elapsed()))
}

async fn read_bounded_body(
    mut response: reqwest::Response,
    max_size: usize,
    deadline: Option<time::Instant>,
    started: Instant,
) -> Result<Vec<u8>, Error> {
    let status = response.status();
    let retry_after = response_retry_after(response.headers());
    if let Some(length) = response.content_length()
        && usize::try_from(length).map_or(true, |length| length > max_size)
    {
        return Err(Error::resource_exhausted(format!("HTTP response body exceeds the {max_size}-byte limit.")).with_response(
            status,
            retry_after,
            started.elapsed(),
        ));
    }

    let mut body = Vec::new();
    loop {
        let next = if let Some(deadline) = deadline {
            time::timeout_at(deadline, response.chunk()).await.map_err(|_| {
                Error::deadline(
                    started,
                    Some(status),
                    retry_after,
                    "The HTTP operation exceeded its deadline while reading the response body.",
                )
            })?
        } else {
            response.chunk().await
        };
        let Some(chunk) =
            next.map_err(|error| Error::request(error, started.elapsed()).with_response(status, retry_after, started.elapsed()))?
        else {
            break;
        };
        let required = body.len().checked_add(chunk.len()).filter(|size| *size <= max_size).ok_or_else(|| {
            Error::resource_exhausted(format!("HTTP response body exceeds the {max_size}-byte limit.")).with_response(
                status,
                retry_after,
                started.elapsed(),
            )
        })?;
        if required > body.capacity() {
            body.try_reserve(required - body.len()).map_err(|_| {
                Error::resource_exhausted("Could not grow the HTTP response body.").with_response(status, retry_after, started.elapsed())
            })?;
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub(crate) fn validate_http_url(url: &Url) -> Result<(), Error> {
    let has_userinfo = url.username() != ""
        || url.password().is_some()
        || url.as_str().split_once("://").is_some_and(|(_, authority_and_path)| {
            authority_and_path.split(['/', '?', '#']).next().is_some_and(|authority| authority.contains('@'))
        });
    if !matches!(url.scheme(), "http" | "https") || !url.has_host() || has_userinfo {
        return Err(Error::new(
            ErrorKind::InvalidUrl,
            Duration::ZERO,
            None,
            None,
            "URL must use HTTP or HTTPS and include a host without embedded credentials.".into(),
        ));
    }
    if url.fragment().is_some() {
        return Err(Error::new(ErrorKind::InvalidUrl, Duration::ZERO, None, None, "URL must not include a fragment.".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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

    use crate::test_support::http::{LocalServer, response};

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
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0a, 0xab, 0xa8, 0x18, 0x05, 0xa3, 0x60, 0x14, 0x54, 0x8c, 0x50, 0x00,
            0x00, 0x63, 0xf0, 0xd7, 0x48, 0x00, 0x04, 0x00, 0x00,
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
        let wire_response =
            concat!("HTTP/1.1 200 OK\r\n", "Content-Length: 6\r\n", "Retry-After: 2\r\n", "Connection: close\r\n\r\n", "abc");
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
            stream.write_all(b"data").await.expect("write body");
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
        let response =
            Response { status: StatusCode::OK, headers, body: b"body-session-secret".to_vec(), elapsed: Duration::from_millis(12) };

        let debug = format!("{response:?}");

        for secret in ["authorization", "header-secret", "cookie", "cookie-secret", "body-session-secret"] {
            assert!(!debug.contains(secret));
        }
        assert!(debug.contains("body_bytes"));
        assert!(debug.contains("header_count"));
    }
}
