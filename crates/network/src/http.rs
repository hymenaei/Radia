use std::{
    fmt::{Debug, Display, Formatter},
    time::{Duration, Instant, SystemTime},
};

use reqwest::{
    StatusCode, Url,
    header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, RETRY_AFTER},
    redirect::Policy,
};

use crate::{capture::CaptureRecorder, status::NetworkStatusHandle};

#[cfg(test)]
const MAX_HTTP_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Client {
    client: reqwest::Client,
    capture: Option<CaptureRecorder>,
    status: Option<NetworkStatusHandle>,
}

pub(crate) struct Response {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    pub(crate) url: Url,
    pub(crate) body: Vec<u8>,
    pub(crate) elapsed: Duration,
}

impl Response {
    pub(crate) fn retry_after(&self) -> Option<Duration> {
        let value = self.headers.get(RETRY_AFTER)?.to_str().ok()?;
        retry_after_delay(value, SystemTime::now())
    }
}

fn retry_after_delay(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        let seconds = value.parse::<u64>().unwrap_or(u64::MAX);
        return Some(Duration::from_secs(seconds));
    }

    let retry_at = httpdate::parse_http_date(value).ok()?;
    Some(retry_at.duration_since(now).unwrap_or_default())
}

pub(crate) enum Error {
    Request(reqwest::Error),
    Timeout { elapsed: Duration, message: String },
    Message(String),
}

impl Debug for Response {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Response")
            .field("status", &self.status)
            .field("header_count", &self.headers.len())
            .field("url", &"[REDACTED]")
            .field("body_bytes", &self.body.len())
            .field("elapsed", &self.elapsed)
            .finish()
    }
}

impl Debug for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(_) => formatter.debug_tuple("Request").field(&"[REDACTED]").finish(),
            Self::Timeout { elapsed, .. } => {
                formatter.debug_struct("Timeout").field("elapsed", elapsed).field("message", &"[REDACTED]").finish()
            }
            Self::Message(_) => formatter.debug_tuple("Message").field(&"[REDACTED]").finish(),
        }
    }
}

impl Error {
    fn request(error: reqwest::Error, elapsed: Duration) -> Self {
        if error.is_timeout() {
            Self::Timeout { elapsed, message: error.without_url().to_string() }
        } else {
            Self::Request(error.without_url())
        }
    }

    fn with_elapsed(self, elapsed: Duration) -> Self {
        match self {
            Self::Request(error) if error.is_timeout() => Self::request(error, elapsed),
            error => error,
        }
    }

    pub(crate) fn is_timeout_after(&self, minimum: Duration) -> bool {
        matches!(self, Self::Timeout { elapsed, .. } if *elapsed >= minimum)
    }

    #[cfg(test)]
    pub(crate) fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout { .. }) || matches!(self, Self::Request(error) if error.is_timeout())
    }
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(error) => write!(formatter, "{error}"),
            Self::Timeout { message, .. } => formatter.write_str(message),
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

impl Client {
    pub(crate) fn new(timeout: Option<Duration>, http1_only: bool, user_agent: Option<&str>) -> Result<Self, reqwest::Error> {
        let mut builder = reqwest::Client::builder().redirect(Policy::none());
        if let Some(timeout) = timeout {
            builder = builder.timeout(timeout);
        }
        if http1_only {
            builder = builder.http1_only();
        }
        if let Some(user_agent) = user_agent {
            builder = builder.user_agent(user_agent);
        }
        builder.build().map(|client| Self { client, capture: None, status: None })
    }

    pub(crate) fn with_capture(mut self, capture: Option<CaptureRecorder>) -> Self {
        self.capture = capture;
        self
    }

    pub(crate) fn with_status(mut self, status: NetworkStatusHandle) -> Self {
        self.status = Some(status);
        self
    }

    #[cfg(test)]
    pub(crate) async fn post_bounded(
        &self,
        url: &Url,
        content_type: &str,
        accept: Option<&str>,
        body: Vec<u8>,
        max_response_bytes: usize,
    ) -> Result<Response, String> {
        self.post_operation_bounded("capability_post", None, url, content_type, accept, body, max_response_bytes).await
    }

    pub(crate) async fn post_operation_bounded(
        &self,
        operation: &str,
        capture_endpoint: Option<&str>,
        url: &Url,
        content_type: &str,
        accept: Option<&str>,
        body: Vec<u8>,
        max_response_bytes: usize,
    ) -> Result<Response, String> {
        self.post_operation_bounded_inner(operation, capture_endpoint, url, content_type, accept, body, max_response_bytes, true)
            .await
            .map_err(|error| error.to_string())
    }

    pub(crate) async fn post_operation_bounded_classified(
        &self,
        operation: &str,
        capture_endpoint: Option<&str>,
        url: &Url,
        content_type: &str,
        accept: Option<&str>,
        body: Vec<u8>,
        max_response_bytes: usize,
    ) -> Result<Response, Error> {
        self.post_operation_bounded_inner(operation, capture_endpoint, url, content_type, accept, body, max_response_bytes, false).await
    }

    #[cfg(test)]
    pub(crate) async fn get(&self, url: &Url) -> Result<Response, String> {
        self.get_bounded("capability_get", None, url, MAX_HTTP_RESPONSE_BYTES).await
    }

    #[tracing::instrument(
        name = "http.post", level = "debug", skip_all,
        fields(operation = %operation, request_bytes = body.len(), response_limit = max_response_bytes)
    )]
    async fn post_operation_bounded_inner(
        &self,
        operation: &str,
        capture_endpoint: Option<&str>,
        url: &Url,
        content_type: &str,
        accept: Option<&str>,
        body: Vec<u8>,
        max_response_bytes: usize,
        record_errors: bool,
    ) -> Result<Response, Error> {
        validate_http_url(url).map_err(Error::Message)?;
        let mut request = self.client.post(url.clone()).header(CONTENT_TYPE, content_type);
        if let Some(accept) = accept {
            request = request.header(ACCEPT, accept);
        }
        let mut request_headers = vec![("content-type".to_owned(), content_type.to_owned())];
        if let Some(accept) = accept {
            request_headers.push(("accept".to_owned(), accept.to_owned()));
        }
        let request_id = self
            .capture
            .as_ref()
            .map(|capture| capture.http_request(capture_endpoint.unwrap_or(operation), "POST", request_headers, &body));
        let started = Instant::now();
        if let Some(status) = &self.status {
            status.note_http_request();
        }
        let response = match request.body(body).send().await {
            Ok(response) => response,
            Err(error) => {
                let elapsed = started.elapsed();
                if let (Some(capture), Some(request_id)) = (&self.capture, request_id) {
                    capture.http_response(request_id, None, Vec::new(), elapsed, "send_error", None);
                }
                let error = Error::request(error, elapsed);
                tracing::warn!(error = %error, elapsed_ms = elapsed.as_millis(), "HTTP request failed");
                if record_errors && let Some(status) = &self.status {
                    status.record_error(error.to_string());
                }
                return Err(error);
            }
        };
        if let Some(status) = &self.status {
            status.note_http_response();
        }
        let status = response.status();
        let headers = response.headers().clone();
        let captured_headers = capture_headers(&headers);
        let url = response.url().clone();
        let body = match read_bounded_body(response, max_response_bytes).await {
            Ok(body) => body,
            Err(error) => {
                let elapsed = started.elapsed();
                if let (Some(capture), Some(request_id)) = (&self.capture, request_id) {
                    capture.http_response(request_id, Some(status.as_u16()), captured_headers, elapsed, "body_error", None);
                }
                if record_errors && let Some(status) = &self.status {
                    status.record_error(error.to_string());
                }
                tracing::warn!(http_status = status.as_u16(), elapsed_ms = elapsed.as_millis(), "HTTP response body rejected");
                return Err(error.with_elapsed(elapsed));
            }
        };
        let elapsed = started.elapsed();
        tracing::trace!(
            operation = %operation,
            http_status = status.as_u16(),
            response_bytes = body.len(),
            elapsed_ms = elapsed.as_millis(),
            "HTTP response received"
        );
        if let (Some(capture), Some(request_id)) = (&self.capture, request_id) {
            capture.http_response(request_id, Some(status.as_u16()), captured_headers, elapsed, "complete", Some(&body));
        }
        Ok(Response { status, headers, url, body, elapsed })
    }

    #[tracing::instrument(
        name = "http.get", level = "debug", skip_all,
        fields(operation = %operation, response_limit = max_response_bytes)
    )]
    pub(crate) async fn get_bounded(
        &self,
        operation: &str,
        capture_endpoint: Option<&str>,
        url: &Url,
        max_response_bytes: usize,
    ) -> Result<Response, String> {
        validate_http_url(url)?;
        let request_id =
            self.capture.as_ref().map(|capture| capture.http_request(capture_endpoint.unwrap_or(operation), "GET", Vec::new(), &[]));
        let started = Instant::now();
        if let Some(status) = &self.status {
            status.note_http_request();
        }
        let response = match self.client.get(url.clone()).send().await {
            Ok(response) => response,
            Err(error) => {
                if let (Some(capture), Some(request_id)) = (&self.capture, request_id) {
                    capture.http_response(request_id, None, Vec::new(), started.elapsed(), "send_error", None);
                }
                let error = error.without_url().to_string();
                tracing::warn!(elapsed_ms = started.elapsed().as_millis(), "HTTP request failed");
                if let Some(status) = &self.status {
                    status.record_error(&error);
                }
                return Err(error);
            }
        };
        if let Some(status) = &self.status {
            status.note_http_response();
        }
        let status = response.status();
        let headers = response.headers().clone();
        let captured_headers = capture_headers(&headers);
        let url = response.url().clone();
        let body = match read_bounded_body(response, max_response_bytes).await {
            Ok(body) => body,
            Err(error) => {
                if let (Some(capture), Some(request_id)) = (&self.capture, request_id) {
                    capture.http_response(request_id, Some(status.as_u16()), captured_headers, started.elapsed(), "body_error", None);
                }
                if let Some(status) = &self.status {
                    status.record_error(error.to_string());
                }
                tracing::warn!(http_status = status.as_u16(), elapsed_ms = started.elapsed().as_millis(), "HTTP response body rejected");
                return Err(error.to_string());
            }
        };
        tracing::trace!(
            operation = %operation,
            http_status = status.as_u16(),
            response_bytes = body.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "HTTP response received"
        );
        if let (Some(capture), Some(request_id)) = (&self.capture, request_id) {
            capture.http_response(request_id, Some(status.as_u16()), captured_headers, started.elapsed(), "complete", Some(&body));
        }
        Ok(Response { status, headers, url, body, elapsed: started.elapsed() })
    }
}

fn capture_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    let mut captured = Vec::new();
    for name in [CONTENT_TYPE, CONTENT_LENGTH, RETRY_AFTER] {
        for value in headers.get_all(&name).iter().filter_map(|value| value.to_str().ok()) {
            captured.push((name.as_str().to_owned(), value.to_owned()));
        }
    }
    captured
}

pub(crate) async fn read_bounded_body(mut response: reqwest::Response, max_size: usize) -> Result<Vec<u8>, Error> {
    if response.content_length().is_some_and(|length| length > max_size as u64) {
        return Err(Error::Message(format!("HTTP response body exceeds the {max_size}-byte limit.")));
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| Error::Request(error.without_url()))? {
        if body.len().checked_add(chunk.len()).filter(|size| *size <= max_size).is_none() {
            return Err(Error::Message(format!("HTTP response body exceeds the {max_size}-byte limit.")));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub(crate) fn validate_http_url(url: &Url) -> Result<(), String> {
    let has_userinfo = url.username() != ""
        || url.password().is_some()
        || url.as_str().split_once("://").is_some_and(|(_, authority_and_path)| {
            authority_and_path.split(['/', '?', '#']).next().is_some_and(|authority| authority.contains('@'))
        });
    if !matches!(url.scheme(), "http" | "https") || !url.has_host() || has_userinfo {
        return Err("URL must use HTTP or HTTPS and include a host without embedded credentials.".into());
    }
    if url.fragment().is_some() {
        return Err("URL must not include a fragment.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};

    use super::{RETRY_AFTER, Response, StatusCode, Url, capture_headers, retry_after_delay};

    #[test]
    fn duplicate_headers() {
        let mut headers = HeaderMap::new();
        headers.append(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        headers.append(CONTENT_TYPE, HeaderValue::from_static("application/xml"));

        assert_eq!(
            capture_headers(&headers),
            [("content-type".into(), "text/plain".into()), ("content-type".into(), "application/xml".into())]
        );
    }

    #[test]
    fn retry_after_formats() {
        assert_eq!(retry_after_delay("120", UNIX_EPOCH), Some(Duration::from_secs(120)));

        let now = UNIX_EPOCH + Duration::from_secs(784_111_700);
        for value in ["Sun, 06 Nov 1994 08:49:37 GMT", "Sunday, 06-Nov-94 08:49:37 GMT", "Sun Nov  6 08:49:37 1994"] {
            assert_eq!(retry_after_delay(value, now), Some(Duration::from_secs(77)));
        }
    }

    #[test]
    fn response_retry_after() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("0"));
        let response = Response {
            status: StatusCode::TOO_MANY_REQUESTS,
            headers,
            url: Url::parse("http://127.0.0.1/").expect("valid local URL"),
            body: Vec::new(),
            elapsed: Duration::ZERO,
        };

        assert_eq!(response.retry_after(), Some(Duration::ZERO));
    }

    #[test]
    fn invalid_retry_after_is_ignored() {
        assert_eq!(retry_after_delay("soon", UNIX_EPOCH), None);
        assert_eq!(retry_after_delay("", UNIX_EPOCH), None);
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::{
        io::{ErrorKind, Read, Write},
        net::{SocketAddr, TcpListener, TcpStream},
        str::from_utf8,
        thread,
        time::{Duration, Instant},
    };

    use crate::http::{Client, read_bounded_body, validate_http_url};
    use reqwest::StatusCode;
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
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
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

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let redirect = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:0/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let server = LocalServer::start(vec![redirect.as_bytes().to_vec()]);
        let client = Client::new(Some(Duration::from_secs(2)), false, None).expect("build HTTP client");

        let response = client.get(&server.url("/start")).await.expect("return redirect response without following it");
        let requests = server.finish();

        assert_eq!(response.status, StatusCode::FOUND);
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
}

#[cfg(test)]
mod redaction_tests {
    use std::time::Duration;

    use reqwest::{
        StatusCode, Url,
        header::{HeaderMap, HeaderValue},
    };

    use super::Response;

    #[test]
    fn response_debug_hides_private_details() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer header-secret"));
        headers.insert("cookie", HeaderValue::from_static("session=cookie-secret"));
        let response = Response {
            status: StatusCode::OK,
            headers,
            url: Url::parse("https://private-grid.example.test/private-capability?token=url-secret").expect("valid URL"),
            body: b"body-session-secret".to_vec(),
            elapsed: Duration::from_millis(12),
        };

        let debug = format!("{response:?}");

        for secret in [
            "private-grid.example.test",
            "private-capability",
            "url-secret",
            "authorization",
            "header-secret",
            "cookie",
            "cookie-secret",
            "body-session-secret",
        ] {
            assert!(!debug.contains(secret));
        }
        assert!(debug.contains("body_bytes"));
        assert!(debug.contains("header_count"));
    }
}
