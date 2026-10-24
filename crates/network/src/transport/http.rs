use std::{
    error,
    fmt::{self, Debug, Display, Formatter},
    future::Future,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant, SystemTime},
};

use reqwest::{
    StatusCode, Url,
    header::{ACCEPT, CONTENT_TYPE, HeaderMap, RETRY_AFTER},
    redirect::Policy,
};
use tokio::time;

use crate::status::NetworkStatusHandle;

#[cfg(test)]
const MAX_HTTP_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

type AdmittedBody = (Vec<u8>, Option<ResponseMemoryPermit>, Option<ResponseMemoryPermit>);

#[derive(Clone)]
pub(crate) struct Client {
    client: reqwest::Client,
    timeout: Option<Duration>,
    status: Option<NetworkStatusHandle>,
    memory: Option<ResponseMemoryBudget>,
    memory_class: ResponseMemoryClass,
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
        builder.build().map(|client| Self { client, timeout, status: None, memory: None, memory_class: ResponseMemoryClass::Ordinary })
    }

    pub(crate) fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }

    pub(crate) fn with_memory_budget(mut self, memory: ResponseMemoryBudget, memory_class: ResponseMemoryClass) -> Self {
        self.memory = Some(memory);
        self.memory_class = memory_class;
        self
    }

    pub(crate) fn try_reserve_memory(&self, class: ResponseMemoryClass, bytes: usize) -> Result<ResponseMemoryPermit, Error> {
        match &self.memory {
            Some(memory) => memory.try_reserve(class, bytes),
            None => Ok(ResponseMemoryPermit { budget: None, class, bytes: 0 }),
        }
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
        self.post_operation_bounded("capability_post", url, content_type, accept, body, max_response_bytes).await
    }

    pub(crate) async fn post_operation_bounded(
        &self,
        operation: &str,
        url: &Url,
        content_type: &str,
        accept: Option<&str>,
        body: Vec<u8>,
        max_response_bytes: usize,
    ) -> Result<Response, String> {
        self.post_operation_bounded_inner(operation, url, content_type, accept, body, max_response_bytes, true, None)
            .await
            .map_err(|error| error.to_string())
    }

    pub(crate) async fn post_operation_bounded_classified(
        &self,
        operation: &str,
        url: &Url,
        content_type: &str,
        accept: Option<&str>,
        body: Vec<u8>,
        max_response_bytes: usize,
    ) -> Result<Response, Error> {
        self.post_operation_bounded_inner(operation, url, content_type, accept, body, max_response_bytes, false, None).await
    }

    pub(crate) async fn post_operation_bounded_classified_with_extra(
        &self,
        operation: &str,
        url: &Url,
        content_type: &str,
        accept: Option<&str>,
        body: Vec<u8>,
        max_response_bytes: usize,
        extra_memory: ResponseMemoryPermit,
    ) -> Result<Response, Error> {
        self.post_operation_bounded_inner(operation, url, content_type, accept, body, max_response_bytes, false, Some(extra_memory)).await
    }

    #[cfg(test)]
    pub(crate) async fn get(&self, url: &Url) -> Result<Response, String> {
        self.get_bounded("capability_get", url, MAX_HTTP_RESPONSE_BYTES).await
    }

    #[tracing::instrument(
        name = "http.post", level = "debug", skip_all,
        fields(operation = %operation, request_bytes = body.len(), response_limit = max_response_bytes)
    )]
    async fn post_operation_bounded_inner(
        &self,
        operation: &str,
        url: &Url,
        content_type: &str,
        accept: Option<&str>,
        body: Vec<u8>,
        max_response_bytes: usize,
        record_errors: bool,
        extra_memory: Option<ResponseMemoryPermit>,
    ) -> Result<Response, Error> {
        validate_http_url(url).map_err(Error::Message)?;
        let started = Instant::now();
        let deadline = self.timeout.map(|timeout| time::Instant::now() + timeout);
        let mut request = self.client.post(url.clone()).header(CONTENT_TYPE, content_type);
        if let Some(deadline) = deadline {
            request = request.timeout(deadline.saturating_duration_since(time::Instant::now()));
        }
        if let Some(accept) = accept {
            request = request.header(ACCEPT, accept);
        }
        if let Some(status) = &self.status {
            status.note_http_request();
        }
        let send = request.body(body).send();
        let response = match send_with_deadline(send, deadline, started).await {
            Ok(response) => response,
            Err(error) => {
                let elapsed = started.elapsed();
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
        let retry_after = response_retry_after(&headers);
        let body = match read_bounded_body_admitted(
            response,
            max_response_bytes,
            self.memory.as_ref(),
            self.memory_class,
            extra_memory,
            deadline,
            started,
        )
        .await
        {
            Ok(body) => body,
            Err(error) => {
                let elapsed = started.elapsed();
                if record_errors && let Some(status) = &self.status {
                    status.record_error(error.to_string());
                }
                tracing::warn!(http_status = status.as_u16(), elapsed_ms = elapsed.as_millis(), "HTTP response body rejected");
                return Err(error.with_response(status, retry_after, elapsed));
            }
        };
        let elapsed = started.elapsed();
        tracing::debug!(
            http_status = status.as_u16(),
            response_bytes = body.0.len(),
            elapsed_ms = elapsed.as_millis(),
            "HTTP response received"
        );
        Ok(Response { status, headers, body: body.0, elapsed, memory: body.1, extra_memory: body.2 })
    }

    #[cfg(test)]
    pub(crate) async fn get_bounded(&self, operation: &str, url: &Url, max_response_bytes: usize) -> Result<Response, String> {
        self.get_bounded_classified(operation, url, max_response_bytes).await.map_err(|error| error.to_string())
    }

    #[tracing::instrument(
        name = "http.get", level = "debug", skip_all,
        fields(operation = %operation, response_limit = max_response_bytes)
    )]
    pub(crate) async fn get_bounded_classified(&self, operation: &str, url: &Url, max_response_bytes: usize) -> Result<Response, Error> {
        validate_http_url(url).map_err(Error::Message)?;
        let started = Instant::now();
        let deadline = self.timeout.map(|timeout| time::Instant::now() + timeout);
        if let Some(status) = &self.status {
            status.note_http_request();
        }
        let mut request = self.client.get(url.clone());
        if let Some(deadline) = deadline {
            request = request.timeout(deadline.saturating_duration_since(time::Instant::now()));
        }
        let response = match send_with_deadline(request.send(), deadline, started).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(elapsed_ms = started.elapsed().as_millis(), "HTTP request failed");
                if let Some(status) = &self.status {
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
        let retry_after = response_retry_after(&headers);
        let body = match read_bounded_body_admitted(
            response,
            max_response_bytes,
            self.memory.as_ref(),
            self.memory_class,
            None,
            deadline,
            started,
        )
        .await
        {
            Ok(body) => body,
            Err(error) => {
                if let Some(status) = &self.status {
                    status.record_error(error.to_string());
                }
                tracing::warn!(http_status = status.as_u16(), elapsed_ms = started.elapsed().as_millis(), "HTTP response body rejected");
                return Err(error.with_response(status, retry_after, started.elapsed()));
            }
        };
        let elapsed = started.elapsed();
        tracing::debug!(
            http_status = status.as_u16(),
            response_bytes = body.0.len(),
            elapsed_ms = elapsed.as_millis(),
            "HTTP response received"
        );
        Ok(Response { status, headers, body: body.0, elapsed, memory: body.1, extra_memory: body.2 })
    }
}

pub(crate) struct Response {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Vec<u8>,
    pub(crate) elapsed: Duration,
    pub(crate) memory: Option<ResponseMemoryPermit>,
    pub(crate) extra_memory: Option<ResponseMemoryPermit>,
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

#[non_exhaustive]
pub enum Error {
    Transport { elapsed: Duration, status: Option<StatusCode>, retry_after: Option<Duration>, message: String },
    Timeout { elapsed: Duration, status: Option<StatusCode>, retry_after: Option<Duration>, message: String },
    Cancelled { elapsed: Duration, status: Option<StatusCode>, retry_after: Option<Duration>, message: String },
    ResourceExhausted { elapsed: Duration, status: Option<StatusCode>, retry_after: Option<Duration>, message: String },
    HttpStatus { status: StatusCode, elapsed: Duration, retry_after: Option<Duration>, message: String },
    Message(String),
}

impl Debug for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport { elapsed, status, retry_after, .. } => formatter
                .debug_struct("Transport")
                .field("elapsed", elapsed)
                .field("status", status)
                .field("retry_after", retry_after)
                .field("message", &"[REDACTED]")
                .finish(),
            Self::Timeout { elapsed, status, retry_after, .. } => formatter
                .debug_struct("Timeout")
                .field("elapsed", elapsed)
                .field("status", status)
                .field("retry_after", retry_after)
                .field("message", &"[REDACTED]")
                .finish(),
            Self::Cancelled { elapsed, status, retry_after, .. } => formatter
                .debug_struct("Cancelled")
                .field("elapsed", elapsed)
                .field("status", status)
                .field("retry_after", retry_after)
                .field("message", &"[REDACTED]")
                .finish(),
            Self::ResourceExhausted { elapsed, status, retry_after, .. } => formatter
                .debug_struct("ResourceExhausted")
                .field("elapsed", elapsed)
                .field("status", status)
                .field("retry_after", retry_after)
                .field("message", &"[REDACTED]")
                .finish(),
            Self::HttpStatus { status, elapsed, retry_after, .. } => formatter
                .debug_struct("HttpStatus")
                .field("status", status)
                .field("elapsed", elapsed)
                .field("retry_after", retry_after)
                .field("message", &"[REDACTED]")
                .finish(),
            Self::Message(_) => formatter.debug_tuple("Message").field(&"[REDACTED]").finish(),
        }
    }
}

impl Error {
    fn request(error: reqwest::Error, elapsed: Duration) -> Self {
        if error.is_timeout() {
            Self::Timeout { elapsed, status: None, retry_after: None, message: error.without_url().to_string() }
        } else {
            Self::Transport { elapsed, status: None, retry_after: None, message: error.without_url().to_string() }
        }
    }

    pub(crate) fn resource_exhausted(message: impl Into<String>) -> Self {
        Self::ResourceExhausted { elapsed: Duration::ZERO, status: None, retry_after: None, message: message.into() }
    }

    fn with_response(self, status: StatusCode, retry_after: Option<Duration>, elapsed: Duration) -> Self {
        match self {
            Self::Timeout { message, .. } => Self::Timeout { elapsed, status: Some(status), retry_after, message },
            Self::Transport { message, .. } => Self::Transport { elapsed, status: Some(status), retry_after, message },
            Self::Cancelled { message, .. } => Self::Cancelled { elapsed, status: Some(status), retry_after, message },
            Self::ResourceExhausted { message, .. } => Self::ResourceExhausted { elapsed, status: Some(status), retry_after, message },
            error => error,
        }
    }

    pub(crate) fn is_timeout_after(&self, minimum: Duration) -> bool {
        matches!(self, Self::Timeout { elapsed, status: None, .. } if *elapsed >= minimum)
    }

    pub(crate) fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Transport { retry_after, .. }
            | Self::Timeout { retry_after, .. }
            | Self::Cancelled { retry_after, .. }
            | Self::ResourceExhausted { retry_after, .. }
            | Self::HttpStatus { retry_after, .. } => *retry_after,
            Self::Message(_) => None,
        }
    }

    pub(crate) fn is_resource_exhausted(&self) -> bool {
        matches!(self, Self::ResourceExhausted { .. })
    }

    pub(crate) fn http_status(status: StatusCode, elapsed: Duration, retry_after: Option<Duration>, context: &str) -> Self {
        Self::HttpStatus { status, elapsed, retry_after, message: format!("{context}: HTTP {status}") }
    }

    #[cfg(test)]
    pub(crate) fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout { .. })
    }
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport { message, .. }
            | Self::Timeout { message, .. }
            | Self::Cancelled { message, .. }
            | Self::ResourceExhausted { message, .. }
            | Self::HttpStatus { message, .. } => formatter.write_str(message),
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

impl error::Error for Error {}

#[derive(Clone, Copy)]
pub(crate) enum ResponseMemoryClass {
    Ordinary,
    EventQueue,
}

#[derive(Clone)]
pub(crate) struct ResponseMemoryBudget(Arc<Mutex<ResponseMemoryState>>);

impl ResponseMemoryBudget {
    pub(crate) fn new(ordinary_limit: usize, event_queue_limit: usize, shared_limit: usize) -> Self {
        Self(Arc::new(Mutex::new(ResponseMemoryState {
            ordinary_limit,
            event_queue_limit,
            shared_limit,
            ordinary_used: 0,
            event_queue_used: 0,
            shared_used: 0,
        })))
    }

    pub(crate) fn try_reserve(&self, class: ResponseMemoryClass, bytes: usize) -> Result<ResponseMemoryPermit, Error> {
        self.reserve(class, bytes)?;
        Ok(ResponseMemoryPermit { budget: Some(self.clone()), class, bytes })
    }

    fn reserve(&self, class: ResponseMemoryClass, bytes: usize) -> Result<(), Error> {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let (used, limit) = match class {
            ResponseMemoryClass::Ordinary => (state.ordinary_used, state.ordinary_limit),
            ResponseMemoryClass::EventQueue => (state.event_queue_used, state.event_queue_limit),
        };
        let next_used =
            used.checked_add(bytes).ok_or_else(|| Error::resource_exhausted("Capability memory budget arithmetic overflowed."))?;
        let old_shared = used.saturating_sub(limit);
        let new_shared = next_used.saturating_sub(limit);
        let Some(next_shared) = state
            .shared_used
            .checked_sub(old_shared)
            .and_then(|current| current.checked_add(new_shared))
            .filter(|shared| *shared <= state.shared_limit)
        else {
            return Err(Error::resource_exhausted("Capability response memory is temporarily unavailable."));
        };
        match class {
            ResponseMemoryClass::Ordinary => state.ordinary_used = next_used,
            ResponseMemoryClass::EventQueue => state.event_queue_used = next_used,
        }
        state.shared_used = next_shared;
        Ok(())
    }

    fn release(&self, class: ResponseMemoryClass, bytes: usize) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let (used, limit) = match class {
            ResponseMemoryClass::Ordinary => (state.ordinary_used, state.ordinary_limit),
            ResponseMemoryClass::EventQueue => (state.event_queue_used, state.event_queue_limit),
        };
        let next_used = used.saturating_sub(bytes);
        let released_shared = used.saturating_sub(limit).saturating_sub(next_used.saturating_sub(limit));
        match class {
            ResponseMemoryClass::Ordinary => state.ordinary_used = next_used,
            ResponseMemoryClass::EventQueue => state.event_queue_used = next_used,
        }
        state.shared_used = state.shared_used.saturating_sub(released_shared);
    }
}

struct ResponseMemoryState {
    ordinary_limit: usize,
    event_queue_limit: usize,
    shared_limit: usize,
    ordinary_used: usize,
    event_queue_used: usize,
    shared_used: usize,
}

pub(crate) struct ResponseMemoryPermit {
    budget: Option<ResponseMemoryBudget>,
    class: ResponseMemoryClass,
    bytes: usize,
}

impl ResponseMemoryPermit {
    pub(crate) fn try_grow_to(&mut self, bytes: usize) -> Result<(), Error> {
        self.try_grow(bytes.saturating_sub(self.bytes))
    }

    pub(crate) fn try_grow(&mut self, bytes: usize) -> Result<(), Error> {
        let Some(budget) = &self.budget else {
            return Ok(());
        };
        let next_bytes =
            self.bytes.checked_add(bytes).ok_or_else(|| Error::resource_exhausted("Capability memory budget arithmetic overflowed."))?;
        budget.reserve(self.class, bytes)?;
        self.bytes = next_bytes;
        Ok(())
    }

    pub(crate) fn shrink_to(&mut self, bytes: usize) {
        let released = self.bytes.saturating_sub(bytes);
        if released > 0 {
            if let Some(budget) = &self.budget {
                budget.release(self.class, released);
            }
            self.bytes -= released;
        }
    }
}

impl Drop for ResponseMemoryPermit {
    fn drop(&mut self) {
        if let Some(budget) = &self.budget {
            budget.release(self.class, self.bytes);
        }
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

fn response_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?;
    retry_after_delay(value, SystemTime::now())
}

async fn send_with_deadline<F>(send: F, deadline: Option<time::Instant>, started: Instant) -> Result<reqwest::Response, Error>
where
    F: Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    let result = if let Some(deadline) = deadline {
        time::timeout_at(deadline, send).await.map_err(|_| Error::Timeout {
            elapsed: started.elapsed(),
            status: None,
            retry_after: None,
            message: "The HTTP operation exceeded its deadline.".into(),
        })?
    } else {
        send.await
    };
    result.map_err(|error| Error::request(error, started.elapsed()))
}

fn admit_body_capacity(
    memory: &mut Option<ResponseMemoryPermit>,
    capacity: usize,
    status: StatusCode,
    retry_after: Option<Duration>,
    started: Instant,
) -> Result<(), Error> {
    let Some(memory) = memory else { return Ok(()) };
    if capacity > memory.bytes {
        memory.try_grow(capacity - memory.bytes).map_err(|error| error.with_response(status, retry_after, started.elapsed()))?;
    }
    memory.shrink_to(capacity);
    Ok(())
}

fn grow_admitted_body(
    body: &mut Vec<u8>,
    additional_bytes: usize,
    required: usize,
    admitted_bytes: &mut usize,
    memory: &mut Option<ResponseMemoryPermit>,
    status: StatusCode,
    retry_after: Option<Duration>,
    started: Instant,
) -> Result<(), Error> {
    if required > body.capacity()
        && let Some(memory) = memory
    {
        memory.try_grow(required).map_err(|error| error.with_response(status, retry_after, started.elapsed()))?;
    }
    body.try_reserve_exact(additional_bytes).map_err(|_| {
        Error::resource_exhausted("Could not grow the admitted HTTP response body.").with_response(status, retry_after, started.elapsed())
    })?;
    if body.capacity() > *admitted_bytes {
        admit_body_capacity(memory, body.capacity(), status, retry_after, started)?;
        *admitted_bytes = body.capacity();
    }
    Ok(())
}

#[cfg(test)]
pub(crate) async fn read_bounded_body(response: reqwest::Response, max_size: usize) -> Result<Vec<u8>, Error> {
    read_bounded_body_admitted(response, max_size, None, ResponseMemoryClass::Ordinary, None, None, Instant::now())
        .await
        .map(|(body, _, _)| body)
}

async fn read_bounded_body_admitted(
    mut response: reqwest::Response,
    max_size: usize,
    memory_budget: Option<&ResponseMemoryBudget>,
    memory_class: ResponseMemoryClass,
    extra_memory: Option<ResponseMemoryPermit>,
    deadline: Option<time::Instant>,
    started: Instant,
) -> Result<AdmittedBody, Error> {
    let status = response.status();
    let retry_after = response_retry_after(response.headers());
    let declared = response.content_length();
    let hinted_bytes = match declared {
        Some(length) => match usize::try_from(length) {
            Ok(length) if length <= max_size => length,
            Ok(_) | Err(_) => {
                return Err(Error::resource_exhausted(format!("HTTP response body exceeds the {max_size}-byte limit.")).with_response(
                    status,
                    retry_after,
                    started.elapsed(),
                ));
            }
        },
        None => 0,
    };
    let mut memory = memory_budget
        .map(|budget| budget.try_reserve(memory_class, hinted_bytes))
        .transpose()
        .map_err(|error| error.with_response(status, retry_after, started.elapsed()))?;
    let mut admitted_bytes = hinted_bytes;
    let mut body = Vec::new();
    if hinted_bytes > 0 {
        body.try_reserve_exact(hinted_bytes).map_err(|_| {
            Error::resource_exhausted("Could not allocate the admitted HTTP response body.").with_response(
                status,
                retry_after,
                started.elapsed(),
            )
        })?;
        if body.capacity() > admitted_bytes {
            admit_body_capacity(&mut memory, body.capacity(), status, retry_after, started)?;
            admitted_bytes = body.capacity();
        }
    }

    loop {
        let next = if let Some(deadline) = deadline {
            time::timeout_at(deadline, response.chunk()).await.map_err(|_| Error::Timeout {
                elapsed: started.elapsed(),
                status: Some(status),
                retry_after,
                message: "The HTTP operation exceeded its deadline while reading the response body.".into(),
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
        let chunk_memory = memory_budget
            .map(|budget| budget.try_reserve(memory_class, chunk.len()))
            .transpose()
            .map_err(|error| error.with_response(status, retry_after, started.elapsed()))?;
        grow_admitted_body(&mut body, chunk.len(), required, &mut admitted_bytes, &mut memory, status, retry_after, started)?;
        body.extend_from_slice(&chunk);
        drop(chunk);
        drop(chunk_memory);
    }

    if let Some(memory) = &mut memory {
        memory.shrink_to(body.capacity());
    }
    Ok((body, memory, extra_memory))
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

    use reqwest::header::{HeaderMap, HeaderValue};

    use super::{Error, RETRY_AFTER, Response, ResponseMemoryBudget, ResponseMemoryClass, StatusCode, retry_after_delay};

    #[test]
    fn shared_memory_preserves_class_reserves() {
        let budget = ResponseMemoryBudget::new(8, 6, 4);
        let mut ordinary = budget.try_reserve(ResponseMemoryClass::Ordinary, 12).expect("ordinary borrows shared bytes");
        let mut events = budget.try_reserve(ResponseMemoryClass::EventQueue, 6).expect("protected EventQueue bytes remain available");
        assert!(matches!(events.try_grow(1), Err(Error::ResourceExhausted { .. })));
        ordinary.shrink_to(8);
        events.try_grow(4).expect("released shared bytes can be borrowed by EventQueue");
        assert!(matches!(ordinary.try_grow(1), Err(Error::ResourceExhausted { .. })));
        drop(ordinary);
        assert!(
            matches!(events.try_grow(1), Err(Error::ResourceExhausted { .. })),
            "EventQueue cannot exceed its reserved and shared capacity"
        );
        drop(events);
        for (class, bytes) in [(ResponseMemoryClass::Ordinary, 12), (ResponseMemoryClass::EventQueue, 10)] {
            drop(budget.try_reserve(class, bytes).expect("all reservations return to baseline"));
        }
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
            body: Vec::new(),
            elapsed: Duration::ZERO,
            memory: None,
            extra_memory: None,
        };

        assert_eq!(response.retry_after(), Some(Duration::ZERO));
    }

    #[test]
    fn invalid_retry_after_is_ignored() {
        assert_eq!(retry_after_delay("soon", UNIX_EPOCH), None);
        assert_eq!(retry_after_delay("", UNIX_EPOCH), None);
    }
    #[cfg(test)]
    mod body_tests {
        use std::{
            future::pending,
            io::{Read, Write},
            net::TcpListener,
            thread,
            time::Instant,
        };

        use crate::capabilities::ProcessMeasurement;
        use crate::http::{
            Client, ResponseMemoryClass, admit_body_capacity, grow_admitted_body, read_bounded_body, read_bounded_body_admitted,
            validate_http_url,
        };
        use crate::test_support::http::{LocalServer, response};
        use reqwest::StatusCode;
        use url::Url;

        use super::*;

        struct HeldBodyServer {
            address: std::net::SocketAddr,
            worker: thread::JoinHandle<Vec<Vec<u8>>>,
        }

        impl HeldBodyServer {
            fn url(&self, path: &str) -> Url {
                Url::parse(&format!("http://{}{path}", self.address)).expect("valid local HTTP URL")
            }

            fn finish(self) -> Vec<Vec<u8>> {
                self.worker.join().expect("held-body HTTP server should finish")
            }
        }

        fn start_held_body(response: Vec<u8>) -> (HeldBodyServer, tokio::sync::oneshot::Sender<()>) {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind local HTTP server");
            let address = listener.local_addr().expect("read local listener address");
            let (release_body, body_released) = tokio::sync::oneshot::channel();
            let worker = thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("accept local HTTP request");
                stream.set_read_timeout(Some(Duration::from_secs(5))).expect("set local request timeout");
                let mut request = Vec::new();
                let mut chunk = [0; 4096];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let size = stream.read(&mut chunk).expect("read local HTTP request");
                    assert_ne!(size, 0, "local request ended before its headers");
                    request.extend_from_slice(&chunk[..size]);
                }
                let body_start = response.windows(4).position(|window| window == b"\r\n\r\n").expect("response headers are terminated") + 4;
                stream.write_all(&response[..body_start]).expect("write local response headers");
                body_released.blocking_recv().expect("test should release the held response body");
                stream.write_all(&response[body_start..]).expect("write local response body");
                vec![request]
            });

            (HeldBodyServer { address, worker }, release_body)
        }

        #[tokio::test]
        async fn redirects_are_not_followed() {
            let redirect =
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:0/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
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

        #[tokio::test(flavor = "current_thread")]
        #[ignore = "manual allocation measurement; run cargo test -p network --release http_client_heap_peak -- --ignored --nocapture"]
        async fn http_client_heap_peak() {
            const BODY_SIZES: [usize; 2] = [128 * 1024, crate::http::MAX_HTTP_RESPONSE_BYTES];
            let responses = BODY_SIZES
                .iter()
                .flat_map(|body_bytes| {
                    let body = "x".repeat(*body_bytes);
                    let wire_response = response(200, "OK", "application/octet-stream", None, &body);
                    [wire_response.clone(), wire_response]
                })
                .collect();
            let server = LocalServer::start(responses);
            let raw_client = reqwest::Client::new();
            for body_bytes in BODY_SIZES {
                let raw_measurement: ProcessMeasurement = crate::capabilities::begin_process_measurement();
                let raw = raw_client
                    .get(server.url("/raw"))
                    .send()
                    .await
                    .expect("receive raw Reqwest response")
                    .bytes()
                    .await
                    .expect("read raw body");
                let (raw_baseline, raw_live, raw_peak, raw_elapsed) = raw_measurement.finish();
                assert_eq!(raw.len(), body_bytes);
                assert!(raw.iter().all(|byte| *byte == b'x'));
                let raw_peak_delta = raw_peak.saturating_sub(raw_baseline);
                drop(raw);

                let budget = ResponseMemoryBudget::new(body_bytes * 4, 0, 0);
                let client = crate::http::Client::new(Some(Duration::from_secs(5)), false, None)
                    .expect("build bounded Network HTTP client")
                    .with_memory_budget(budget.clone(), ResponseMemoryClass::Ordinary);
                let network_measurement: ProcessMeasurement = crate::capabilities::begin_process_measurement();
                let result = client.get_bounded_classified("test", &server.url("/bounded"), body_bytes).await;
                let (network_baseline, network_live, network_peak, network_elapsed) = network_measurement.finish();
                let response = result.expect("admit bounded Network response");
                assert_eq!(response.body.len(), body_bytes);
                assert!(response.body.iter().all(|byte| *byte == b'x'));
                assert_eq!(response.memory.as_ref().map(|permit| permit.bytes), Some(response.body.capacity()));
                let network_peak_delta = network_peak.saturating_sub(network_baseline);
                eprintln!(
                    "HTTP/1 loopback process allocation sample: body={body_bytes} B, raw Reqwest live/peak={raw_live}/{raw_peak_delta} B in {raw_elapsed} ns, Network live/peak={network_live}/{network_peak_delta} B in {network_elapsed} ns, Network body capacity={} B",
                    response.body.capacity(),
                );

                drop(response);
                let permit =
                    budget.try_reserve(ResponseMemoryClass::Ordinary, body_bytes * 4).expect("response drop returns all body admission");
                drop(permit);
            }
            assert_eq!(server.finish().len(), BODY_SIZES.len() * 2);
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
        async fn oversized_declared_length_is_rejected_before_body_read_or_admission() {
            let wire_response = b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\n".to_vec();
            let server = LocalServer::start(vec![wire_response]);
            let budget = ResponseMemoryBudget::new(8, 0, 0);
            let client = Client::new(Some(Duration::from_secs(2)), false, None)
                .expect("build HTTP client")
                .with_memory_budget(budget.clone(), ResponseMemoryClass::Ordinary);

            let error = client
                .get_bounded_classified("test", &server.url("/body"), 5)
                .await
                .expect_err("declared body size exceeds limit before the body arrives");

            assert!(matches!(error, Error::ResourceExhausted { status: Some(StatusCode::OK), .. }));
            assert_eq!(server.finish().len(), 1);
            drop(budget.try_reserve(ResponseMemoryClass::Ordinary, 8).expect("early rejection reserves no response memory"));
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
        async fn gzip_content_encoding_remains_raw_and_memory_bounded() {
            const GZIP_BODY: &[u8] = &[
                0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0a, 0xab, 0xa8, 0x18, 0x05, 0xa3, 0x60, 0x14, 0x54, 0x8c, 0x50,
                0x00, 0x00, 0x63, 0xf0, 0xd7, 0x48, 0x00, 0x04, 0x00, 0x00,
            ];
            let mut response =
                format!("HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", GZIP_BODY.len())
                    .into_bytes();
            response.extend_from_slice(GZIP_BODY);
            let server = LocalServer::start(vec![response]);
            let budget = ResponseMemoryBudget::new(GZIP_BODY.len() * 3, 0, 0);
            let client = Client::new(Some(Duration::from_secs(2)), false, None)
                .expect("build HTTP client")
                .with_memory_budget(budget.clone(), ResponseMemoryClass::Ordinary);

            let response = client
                .get_bounded_classified("test", &server.url("/compressed"), GZIP_BODY.len())
                .await
                .expect("the HTTP client does not automatically decompress response bodies");

            assert_eq!(response.body, GZIP_BODY, "compressed content remains caller-owned raw bytes");
            assert_eq!(
                response.memory.as_ref().map(|permit| permit.bytes),
                Some(response.body.capacity()),
                "raw response allocation is charged at its actual capacity"
            );
            drop(response);
            let permit =
                budget.try_reserve(ResponseMemoryClass::Ordinary, GZIP_BODY.len() * 3).expect("drop releases compressed-body admission");
            drop(permit);
            assert_eq!(server.finish().len(), 1);
        }

        #[tokio::test]
        async fn content_length_mismatch_releases_admitted_memory() {
            let response =
                concat!("HTTP/1.1 200 OK\r\n", "Content-Length: 6\r\n", "Retry-After: 2\r\n", "Connection: close\r\n\r\n", "abc");
            let server = LocalServer::start(vec![response.as_bytes().to_vec()]);
            let budget = ResponseMemoryBudget::new(12, 0, 0);
            let client = Client::new(Some(Duration::from_secs(2)), false, None)
                .expect("build HTTP client")
                .with_memory_budget(budget.clone(), ResponseMemoryClass::Ordinary);

            let error = client
                .get_bounded_classified("test", &server.url("/body"), 6)
                .await
                .expect_err("truncated Content-Length response must fail");

            assert!(matches!(
                error,
                Error::Transport { status: Some(StatusCode::OK), elapsed, retry_after: Some(retry_after), .. }
                    if elapsed > Duration::ZERO && elapsed < Duration::from_secs(2) && retry_after == Duration::from_secs(2)
            ));
            assert_eq!(server.finish().len(), 1);
            let permit = budget.try_reserve(ResponseMemoryClass::Ordinary, 12).expect("truncated body releases its admission");
            drop(permit);
        }

        #[tokio::test]
        async fn body_growth_fails_fast_and_releases_budget() {
            let response = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Transfer-Encoding: chunked\r\n",
                "Connection: close\r\n\r\n",
                "3\r\nabc\r\n",
                "3\r\ndef\r\n",
                "0\r\n\r\n"
            );
            let server = LocalServer::start(vec![response.as_bytes().to_vec()]);
            let budget = ResponseMemoryBudget::new(5, 0, 0);
            let client = Client::new(Some(Duration::from_secs(2)), false, None)
                .expect("build HTTP client")
                .with_memory_budget(budget.clone(), ResponseMemoryClass::Ordinary);

            let error = client
                .get_bounded_classified("test", &server.url("/body"), 10)
                .await
                .expect_err("actual response growth exceeds the admitted memory");

            assert!(matches!(error, Error::ResourceExhausted { status: Some(StatusCode::OK), .. }));
            assert_eq!(server.finish().len(), 1);
            let permit = budget.try_reserve(ResponseMemoryClass::Ordinary, 5).expect("failed response releases partial admission");
            drop(permit);
        }

        #[tokio::test]
        async fn chunked_body_growth_releases_budget() {
            let response = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Transfer-Encoding: chunked\r\n",
                "Connection: close\r\n\r\n",
                "3\r\nabc\r\n",
                "3\r\ndef\r\n",
                "0\r\n\r\n"
            );
            let server = LocalServer::start(vec![response.as_bytes().to_vec()]);
            let budget = ResponseMemoryBudget::new(12, 0, 0);
            let client = Client::new(Some(Duration::from_secs(2)), false, None)
                .expect("build HTTP client")
                .with_memory_budget(budget.clone(), ResponseMemoryClass::Ordinary);

            let response = client.get_bounded_classified("test", &server.url("/body"), 6).await.expect("admit exact body capacity");
            assert_eq!(response.body, b"abcdef");
            assert!(budget.try_reserve(ResponseMemoryClass::Ordinary, 7).is_err(), "response memory stays charged while its body lives");

            drop(response);
            let permit = budget.try_reserve(ResponseMemoryClass::Ordinary, 12).expect("body drop returns the full admission");
            drop(permit);
            server.finish();
        }

        #[test]
        fn body_growth_reserves_replacement() {
            let budget = ResponseMemoryBudget::new(8, 0, 0);
            let mut body = Vec::with_capacity(4);
            body.extend_from_slice(b"data");
            let mut memory = Some(budget.try_reserve(ResponseMemoryClass::Ordinary, body.capacity()).expect("admit old body"));
            let mut admitted_bytes = body.capacity();

            let error = grow_admitted_body(&mut body, 1, 5, &mut admitted_bytes, &mut memory, StatusCode::OK, None, Instant::now())
                .expect_err("the old body and full replacement exceed the eight-byte budget");

            assert!(matches!(error, Error::ResourceExhausted { status: Some(StatusCode::OK), .. }));
            assert_eq!(body.as_slice(), b"data");
            assert_eq!(body.capacity(), 4, "reallocation must not start before admission succeeds");
            assert_eq!(admitted_bytes, 4);
            assert!(budget.try_reserve(ResponseMemoryClass::Ordinary, 4).is_ok(), "only the old body remains charged");
        }

        #[test]
        fn body_overallocation_is_charged() {
            let budget = ResponseMemoryBudget::new(12, 0, 0);
            let mut memory = Some(budget.try_reserve(ResponseMemoryClass::Ordinary, 9).expect("admit old and requested body bytes"));

            admit_body_capacity(&mut memory, 12, StatusCode::OK, None, Instant::now())
                .expect("charge allocator capacity beyond the requested growth");
            assert!(budget.try_reserve(ResponseMemoryClass::Ordinary, 1).is_err(), "actual Vec capacity remains charged");

            drop(memory);
            let budget = ResponseMemoryBudget::new(11, 0, 0);
            let mut memory = Some(budget.try_reserve(ResponseMemoryClass::Ordinary, 9).expect("admit initial reservation"));
            let error = admit_body_capacity(&mut memory, 12, StatusCode::OK, None, Instant::now())
                .expect_err("reject actual capacity beyond the remaining budget");
            assert!(matches!(error, Error::ResourceExhausted { status: Some(StatusCode::OK), .. }));
            assert!(budget.try_reserve(ResponseMemoryClass::Ordinary, 2).is_ok(), "failed growth keeps only the old reservation");
        }

        #[tokio::test]
        async fn body_copy_accounts_for_chunk_memory() {
            let response = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Transfer-Encoding: chunked\r\n",
                "Connection: close\r\n\r\n",
                "6\r\nabcdef\r\n",
                "0\r\n\r\n"
            );
            let server = LocalServer::start(vec![response.as_bytes().to_vec()]);
            let budget = ResponseMemoryBudget::new(6, 0, 0);
            let client = Client::new(Some(Duration::from_secs(2)), false, None)
                .expect("build HTTP client")
                .with_memory_budget(budget.clone(), ResponseMemoryClass::Ordinary);

            let error = client
                .get_bounded_classified("test", &server.url("/body"), 6)
                .await
                .expect_err("body storage and the incoming chunk coexist during the copy");

            assert!(matches!(error, Error::ResourceExhausted { status: Some(StatusCode::OK), .. }));
            let permit = budget.try_reserve(ResponseMemoryClass::Ordinary, 6).expect("failed read releases body and chunk admission");
            drop(permit);
            server.finish();
        }

        #[tokio::test(start_paused = true)]
        async fn operation_deadline_covers_body_reading() {
            let response = response(200, "OK", "text/plain", None, "data");
            let (server, release_body) = start_held_body(response);
            let timeout = Duration::from_millis(250);
            let started = Instant::now();
            let deadline = tokio::time::Instant::now() + timeout;
            let response = reqwest::Client::new().get(server.url("/body")).send().await.expect("receive response headers");
            let (body_started, body_started_receiver) = tokio::sync::oneshot::channel();
            let body_read = tokio::spawn(async move {
                tokio::select! {
                    biased;
                    result = read_bounded_body_admitted(
                        response,
                        16,
                        None,
                        ResponseMemoryClass::Ordinary,
                        None,
                        Some(deadline),
                        started,
                    ) => result,
                    _ = async {
                        body_started.send(()).expect("body read is being observed");
                        pending::<()>().await;
                    } => unreachable!(),
                }
            });
            body_started_receiver.await.expect("body read reached the response stream");
            tokio::time::advance(timeout).await;
            let error = match body_read.await.expect("body read task should not panic") {
                Err(error) => error,
                Ok(_) => panic!("held response body should exceed the operation deadline"),
            };

            assert!(matches!(&error, Error::Timeout { status: Some(StatusCode::OK), .. }), "body read exceeded its deadline: {error:?}");
            release_body.send(()).expect("local server should still be waiting for the response body release");
            assert_eq!(server.finish().len(), 1);
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
            StatusCode,
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
                body: b"body-session-secret".to_vec(),
                elapsed: Duration::from_millis(12),
                memory: None,
                extra_memory: None,
            };

            let debug = format!("{response:?}");

            for secret in ["authorization", "header-secret", "cookie", "cookie-secret", "body-session-secret"] {
                assert!(!debug.contains(secret));
            }
            assert!(debug.contains("body_bytes"));
            assert!(debug.contains("header_count"));
        }
    }
}
