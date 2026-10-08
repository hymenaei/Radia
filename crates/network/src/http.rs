use std::{
    fmt,
    time::{Duration, Instant, SystemTime},
};

use reqwest::{
    Client as ReqwestClient, StatusCode, Url,
    header::{HeaderMap, RETRY_AFTER},
};

use crate::{capture::CaptureRecorder, status::NetworkStatusHandle};

#[cfg(test)]
const MAX_HTTP_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Client {
    client: ReqwestClient,
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

impl fmt::Debug for Response {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
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

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
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

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(error) => write!(formatter, "{error}"),
            Self::Timeout { message, .. } => formatter.write_str(message),
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

impl Client {
    pub(crate) fn new(timeout: Option<Duration>, http1_only: bool, user_agent: Option<&str>) -> Result<Self, reqwest::Error> {
        let mut builder = ReqwestClient::builder().redirect(reqwest::redirect::Policy::none());
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
        let mut request = self.client.post(url.clone()).header(reqwest::header::CONTENT_TYPE, content_type);
        if let Some(accept) = accept {
            request = request.header(reqwest::header::ACCEPT, accept);
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
    for name in [reqwest::header::CONTENT_TYPE, reqwest::header::CONTENT_LENGTH, reqwest::header::RETRY_AFTER] {
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

    use super::{capture_headers, retry_after_delay};

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
        headers.insert(super::RETRY_AFTER, HeaderValue::from_static("0"));
        let response = super::Response {
            status: reqwest::StatusCode::TOO_MANY_REQUESTS,
            headers,
            url: super::Url::parse("http://127.0.0.1/").expect("valid local URL"),
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
