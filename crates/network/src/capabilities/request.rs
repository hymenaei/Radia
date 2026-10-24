use std::{
    collections::HashMap,
    fmt,
    mem::size_of,
    str::from_utf8,
    time::{Duration, Instant},
};

use llsd::{Limits, Value, xml};
use reqwest::{StatusCode, Url};
use tokio::{
    sync::{OwnedSemaphorePermit, mpsc, oneshot, watch},
    task::JoinSet,
    time,
};

use crate::{
    capture::CaptureRecorder,
    error::Error,
    http::{self, Response, validate_http_url},
    status::NetworkStatusHandle,
};

use super::{
    budget::{
        CLOSED_ERROR, LLSD_SCAN_WORKING_BYTES, MAX_CAPABILITY_NAME_BYTES, MAX_CAPABILITY_NAMES, MAX_CAPABILITY_NAMES_BYTES,
        MAX_CAPABILITY_REQUEST_BODY_BYTES, MAX_CAPABILITY_RESPONSE_BYTES, MAX_CONCURRENT_CAPABILITY_REQUESTS, MAX_LLSD_NESTING_DEPTH,
        MAX_LLSD_XML_ELEMENTS, MAX_SEED_CAPABILITY_RESPONSE_BYTES, spawn_blocking_with_permit,
    },
    client::CapabilityUrl,
};

pub(super) struct AdmittedResponse {
    pub(super) body: Vec<u8>,
    pub(super) max_response_bytes: usize,
    pub(super) _memory: Option<http::ResponseMemoryPermit>,
    pub(super) _request: OwnedSemaphorePermit,
}

pub(super) struct AdmittedCapabilities {
    pub(super) capabilities: HashMap<String, CapabilityUrl>,
    pub(super) _request: OwnedSemaphorePermit,
    pub(super) _memory: Option<http::ResponseMemoryPermit>,
}

pub(super) struct SeedCapability {
    pub(super) url: Url,
    pub(super) input_bytes: usize,
}

pub(super) fn capability_error(error: CapabilityFailure) -> Error {
    match error {
        CapabilityFailure::Http(error) => Error::CapabilityHttp(error),
        CapabilityFailure::Message(message) => Error::Capability(message),
        CapabilityFailure::SessionClosed => Error::CapabilitySessionClosed,
    }
}

pub(super) fn validate_capability_names(names: &[&str]) -> Result<(), Error> {
    if names.len() > MAX_CAPABILITY_NAMES {
        return Err(Error::Capability(format!("Capability discovery supports at most {MAX_CAPABILITY_NAMES} names.")));
    }

    let mut total_bytes = 0_usize;
    for name in names {
        if name.len() > MAX_CAPABILITY_NAME_BYTES {
            return Err(Error::Capability(format!("A capability discovery name exceeds the {MAX_CAPABILITY_NAME_BYTES}-byte limit.")));
        }
        total_bytes = total_bytes.checked_add(name.len()).filter(|size| *size <= MAX_CAPABILITY_NAMES_BYTES).ok_or_else(|| {
            Error::Capability(format!("Capability discovery names exceed the {MAX_CAPABILITY_NAMES_BYTES}-byte total limit."))
        })?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) struct RequestDeadline {
    started: Instant,
    pub(super) at: time::Instant,
}

impl RequestDeadline {
    pub(super) fn new(timeout: Duration) -> Self {
        Self { started: Instant::now(), at: time::Instant::now() + timeout }
    }

    pub(super) fn expired(self) -> bool {
        time::Instant::now() >= self.at
    }

    fn http_error(self) -> http::Error {
        let elapsed = self.started.elapsed();
        http::Error::Timeout {
            elapsed,
            status: None,
            retry_after: None,
            message: format!("Capability request exceeded its deadline after {} ms.", elapsed.as_millis()),
        }
    }

    pub(super) fn error(self) -> Error {
        Error::CapabilityHttp(self.http_error())
    }
}

pub(super) enum Command {
    RequestCapabilities {
        names: Vec<String>,
        reply: oneshot::Sender<Result<AdmittedCapabilities, CapabilityFailure>>,
        deadline: RequestDeadline,
        _permit: OwnedSemaphorePermit,
    },
    Post {
        url: CapabilityUrl,
        body: Vec<u8>,
        max_response_bytes: usize,
        reply: oneshot::Sender<Result<AdmittedResponse, CapabilityFailure>>,
        deadline: RequestDeadline,
        _permit: OwnedSemaphorePermit,
    },
    GetBytes {
        url: CapabilityUrl,
        max_response_bytes: usize,
        reply: oneshot::Sender<Result<AdmittedResponse, CapabilityFailure>>,
        deadline: RequestDeadline,
        _permit: OwnedSemaphorePermit,
    },
}

pub(super) enum CapabilityFailure {
    Http(http::Error),
    Message(String),
    SessionClosed,
}

impl CapabilityFailure {
    fn is_session_closed(&self) -> bool {
        matches!(self, Self::SessionClosed)
    }
}

impl fmt::Display for CapabilityFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(error) => error.fmt(formatter),
            Self::Message(message) => formatter.write_str(message),
            Self::SessionClosed => formatter.write_str(CLOSED_ERROR),
        }
    }
}

#[tracing::instrument(name = "capabilities.worker", level = "debug", skip_all)]
pub(super) async fn run(
    client: http::Client,
    seed: Url,
    mut commands: mpsc::Receiver<Command>,
    session_closed: watch::Receiver<bool>,
    capture: Option<CaptureRecorder>,
    status: NetworkStatusHandle,
) {
    let mut requests = JoinSet::new();
    loop {
        let command = tokio::select! {
            biased;
            _ = wait_for_session_close(session_closed.clone()) => break,
            command = commands.recv() => command,
        };
        let Some(command) = command else {
            break;
        };

        if requests.len() >= MAX_CONCURRENT_CAPABILITY_REQUESTS {
            tokio::select! {
                biased;
                _ = wait_for_session_close(session_closed.clone()) => break,
                _ = requests.join_next() => {}
            }
        }

        let client = client.clone();
        let seed = seed.clone();
        let session_closed = session_closed.clone();
        let capture = capture.clone();
        let status = status.clone();
        requests.spawn(async move {
            match command {
                Command::RequestCapabilities { names, mut reply, deadline, _permit } => {
                    let result = tokio::select! {
                        biased;
                        _ = wait_for_session_close(session_closed) => Err(CapabilityFailure::SessionClosed),
                        _ = reply.closed() => return,
                        _ = time::sleep_until(deadline.at) => Err(CapabilityFailure::Http(deadline.http_error())),
                        result = request_capabilities(&client, &seed, &names, _permit) => result,
                    };
                    match &result {
                        Ok((granted, _, _)) => status.note_capabilities(granted.keys().map(String::as_str)),
                        Err(error) if !error.is_session_closed() => status.record_error(error.to_string()),
                        Err(_) => {}
                    }
                    if result.is_ok()
                        && let Some(capture) = &capture
                    {
                        capture.lifecycle("capabilities_discovered");
                    }
                    let result = result.map(|(capabilities, memory, request)| AdmittedCapabilities {
                        capabilities,
                        _request: request,
                        _memory: memory,
                    });
                    let _ = reply.send(result);
                }
                Command::Post { url, body, max_response_bytes, mut reply, deadline, _permit } => {
                    let result = tokio::select! {
                        biased;
                        _ = wait_for_session_close(session_closed) => Err(CapabilityFailure::SessionClosed),
                        _ = reply.closed() => return,
                        _ = time::sleep_until(deadline.at) => Err(CapabilityFailure::Http(deadline.http_error())),
                        result = post(&client, &url.url, url.capture_endpoint.as_deref(), body, "Capability request", max_response_bytes)
                            => result.map_err(CapabilityFailure::Http),
                    };
                    if let Err(error) = &result
                        && !error.is_session_closed()
                    {
                        status.record_error(error.to_string());
                    }
                    let result = result.map(|response| into_admitted_response(response, max_response_bytes, _permit));
                    let _ = reply.send(result);
                }
                Command::GetBytes { url, max_response_bytes, mut reply, deadline, _permit } => {
                    let result = tokio::select! {
                        biased;
                        _ = wait_for_session_close(session_closed) => Err(CapabilityFailure::SessionClosed),
                        _ = reply.closed() => return,
                        _ = time::sleep_until(deadline.at) => Err(CapabilityFailure::Http(deadline.http_error())),
                        result = get_bytes(&client, &url.url, url.capture_endpoint.as_deref(), max_response_bytes) => result.map_err(CapabilityFailure::Http),
                    };
                    if let Err(error) = &result
                        && !error.is_session_closed()
                    {
                        status.record_error(error.to_string());
                    }
                    let result = result.map(|response| into_admitted_response(response, max_response_bytes, _permit));
                    let _ = reply.send(result);
                }
            }
        });
    }

    drop(commands);
    while requests.join_next().await.is_some() {}
}

pub(super) async fn wait_for_session_close(mut session_closed: watch::Receiver<bool>) {
    while !*session_closed.borrow() {
        if session_closed.changed().await.is_err() {
            break;
        }
    }
}

async fn request_capabilities(
    client: &http::Client,
    seed: &Url,
    names: &[String],
    request_permit: OwnedSemaphorePermit,
) -> Result<(HashMap<String, CapabilityUrl>, Option<http::ResponseMemoryPermit>, OwnedSemaphorePermit), CapabilityFailure> {
    let response =
        request_capabilities_response(client, seed, names, MAX_SEED_CAPABILITY_RESPONSE_BYTES).await.map_err(LookupError::into_failure)?;
    let names = names.to_vec();
    let (decoded, request_permit) = spawn_blocking_with_permit(request_permit, move || decode_requested_capabilities(response, &names))
        .await
        .map_err(|error| CapabilityFailure::Message(format!("Capability response decoder failed: {error}")))?;
    let (capabilities, memory) = decoded?;
    Ok((capabilities, memory, request_permit))
}

fn decode_requested_capabilities(
    response: Response,
    names: &[String],
) -> Result<(HashMap<String, CapabilityUrl>, Option<http::ResponseMemoryPermit>), CapabilityFailure> {
    let mut response = admit_seed_capability_decode(response).map_err(LookupError::into_failure)?;
    let capabilities = decode_capabilities_with_memory(&response.body, names, response.memory.as_mut(), |name, url, _| {
        CapabilityUrl::from_discovery(name, url)
    })
    .map_err(CapabilityDecodeError::into_failure)?;
    Ok((capabilities, response.memory.take()))
}

pub(super) fn decode_seed_capabilities(
    mut response: Response,
    names: &[String],
) -> Result<(HashMap<String, SeedCapability>, Option<http::ResponseMemoryPermit>), LookupError> {
    let capabilities = decode_capabilities_with_memory(&response.body, names, response.memory.as_mut(), |_, url, input_bytes| {
        SeedCapability { url, input_bytes }
    })
    .map_err(CapabilityDecodeError::into_lookup_error)?;
    let retained_bytes = capability_map_retained_memory_estimate(&capabilities);
    if let Some(memory) = &mut response.memory {
        memory.try_grow_to(retained_bytes).map_err(LookupError::permanent_http)?;
    }
    drop(response.body);
    if let Some(memory) = &mut response.memory {
        memory.shrink_to(retained_bytes);
    }
    Ok((capabilities, response.memory))
}

pub(super) fn admit_and_decode_seed_capabilities(
    response: Response,
    names: &[String],
) -> Result<(HashMap<String, SeedCapability>, Option<http::ResponseMemoryPermit>), LookupError> {
    decode_seed_capabilities(admit_seed_capability_decode(response)?, names)
}

pub(super) async fn request_capabilities_response(
    client: &http::Client,
    seed: &Url,
    names: &[String],
    max_response_bytes: usize,
) -> Result<Response, LookupError> {
    let payload = Value::Array(names.iter().cloned().map(Value::String).collect());
    let body = xml::to_vec_with_max_bytes(&payload, MAX_CAPABILITY_REQUEST_BODY_BYTES)
        .map_err(|error| LookupError::Permanent(format!("Could not encode an LLSD request: {error}")))?;
    let response = client
        .post_operation_bounded_classified(
            "seed_capability",
            None,
            seed,
            "application/llsd+xml",
            Some("application/llsd+xml"),
            body,
            max_response_bytes,
        )
        .await
        .map_err(|error| if error.is_resource_exhausted() { LookupError::permanent_http(error) } else { LookupError::transient(error) })?;
    if response.status.is_server_error() || response.status == StatusCode::TOO_MANY_REQUESTS {
        let error = http::Error::http_status(response.status, response.elapsed, response.retry_after(), "Could not request capabilities");
        return Err(LookupError::transient(error));
    }
    if !response.status.is_success() {
        return Err(LookupError::permanent_http(http::Error::http_status(
            response.status,
            response.elapsed,
            response.retry_after(),
            "Could not request capabilities",
        )));
    }
    Ok(response)
}

fn admit_seed_capability_decode(mut response: Response) -> Result<Response, LookupError> {
    let body = from_utf8(&response.body)
        .map_err(|error| LookupError::Permanent(format!("Could not request capabilities: response was not UTF-8: {error}")))?;
    if let Some(memory) = &mut response.memory {
        memory.try_grow(LLSD_SCAN_WORKING_BYTES).map_err(LookupError::permanent_http)?;
    }
    let bound = xml::value_allocation_bound(body.as_bytes(), seed_capability_llsd_limits()).map_err(|error| {
        LookupError::Permanent(format!("Could not request capabilities: response allocation preflight failed: {error}"))
    })?;
    if let Some(memory) = &mut response.memory {
        memory.shrink_to(response.body.capacity());
        memory.try_grow(bound).map_err(LookupError::permanent_http)?;
    }
    Ok(response)
}

fn decode_capabilities_with_memory<T>(
    body: &[u8],
    names: &[String],
    mut memory: Option<&mut http::ResponseMemoryPermit>,
    make_grant: impl FnMut(&str, Url, usize) -> T,
) -> Result<HashMap<String, T>, CapabilityDecodeError> {
    let response = decode_capability_values(body).map_err(CapabilityDecodeError::Message)?;
    if let Some(memory) = &mut memory {
        let estimate = capability_url_parse_memory_estimate(&response, names)?;
        (**memory).try_grow(estimate).map_err(CapabilityDecodeError::Admission)?;
    }
    decode_capabilities_from_value(&response, names, make_grant).map_err(CapabilityDecodeError::Message)
}

fn decode_capability_values(body: &[u8]) -> Result<Value, String> {
    let body = from_utf8(body).map_err(|error| format!("Could not request capabilities: response was not UTF-8: {error}"))?;
    decode_llsd_xml(body, "Could not request capabilities")
}

fn decode_capabilities_from_value<T>(
    response: &Value,
    names: &[String],
    mut make_grant: impl FnMut(&str, Url, usize) -> T,
) -> Result<HashMap<String, T>, String> {
    let fields = response.as_map().ok_or_else(|| "Second Life returned an invalid capability response.".to_string())?;
    let grant_count = names.iter().filter(|name| fields.contains_key(name.as_str())).count();
    let mut granted = HashMap::with_capacity(grant_count);
    for name in names {
        let Some(value) = fields.get(name) else {
            continue;
        };
        let value = string_value(value).ok_or_else(|| format!("Second Life returned an invalid {name} capability."))?;
        let input_bytes = value.len();
        let url = Url::parse(value).map_err(|_| format!("Second Life returned an invalid {name} capability."))?;
        validate_http_url(&url).map_err(|_| format!("Second Life returned an invalid {name} capability."))?;
        granted.insert(name.clone(), make_grant(name, url, input_bytes));
    }
    Ok(granted)
}

const CAPABILITY_URL_PARSE_BYTES_PER_INPUT_BYTE: usize = 4;
const CAPABILITY_URL_PARSE_BYTES_PER_HOST_COMPONENT: usize = 80;
const CAPABILITY_URL_PARSE_BYTES_PER_GRANT: usize = 512;
const CAPABILITY_URL_PARSE_BASE_BYTES: usize = 2048;
const CAPABILITY_URL_RETAINED_BYTES_PER_SERIALIZED_BYTE: usize = 2;
const CAPABILITY_URL_RETAINED_BASE_BYTES: usize = 128;

fn capability_url_parse_memory_estimate(response: &Value, names: &[String]) -> Result<usize, CapabilityDecodeError> {
    let fields = response
        .as_map()
        .ok_or_else(|| CapabilityDecodeError::Message("Second Life returned an invalid capability response.".to_string()))?;
    let mut input_bytes = 0_usize;
    let mut host_components = 0_usize;
    let mut grant_count = 0_usize;
    for name in names {
        let Some(value) = fields.get(name) else {
            continue;
        };
        let value = string_value(value)
            .ok_or_else(|| CapabilityDecodeError::Message(format!("Second Life returned an invalid {name} capability.")))?;
        input_bytes = input_bytes
            .checked_add(value.len())
            .ok_or_else(|| CapabilityDecodeError::Admission(http::Error::resource_exhausted("Capability URL size estimate overflowed.")))?;
        host_components = host_components
            .checked_add(url_host_component_count(value))
            .ok_or_else(|| CapabilityDecodeError::Admission(http::Error::resource_exhausted("Capability host estimate overflowed.")))?;
        grant_count = grant_count
            .checked_add(1)
            .ok_or_else(|| CapabilityDecodeError::Admission(http::Error::resource_exhausted("Capability grant count overflowed.")))?;
    }
    input_bytes
        .checked_mul(CAPABILITY_URL_PARSE_BYTES_PER_INPUT_BYTE)
        .and_then(|bytes| bytes.checked_add(host_components.checked_mul(CAPABILITY_URL_PARSE_BYTES_PER_HOST_COMPONENT)?))
        .and_then(|bytes| bytes.checked_add(grant_count.checked_mul(CAPABILITY_URL_PARSE_BYTES_PER_GRANT)?))
        .and_then(|bytes| bytes.checked_add(if grant_count == 0 { 0 } else { CAPABILITY_URL_PARSE_BASE_BYTES }))
        .ok_or_else(|| CapabilityDecodeError::Admission(http::Error::resource_exhausted("Capability URL memory estimate overflowed.")))
}

fn url_host_component_count(url: &str) -> usize {
    let authority = url.split_once("://").map(|(_, authority)| authority).unwrap_or(url);
    let authority = authority.split(['/', '?', '#']).next().unwrap_or(authority);
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = if host.starts_with('[') { host.split(']').next().unwrap_or(host) } else { host.split(':').next().unwrap_or(host) };
    let bytes = host.as_bytes();
    let mut count = 1_usize;
    let mut index = 0;
    while index < bytes.len() {
        let (component_separator, width) = if bytes[index] == b'.' || bytes[index] == b':' {
            (true, 1)
        } else if bytes[index..].starts_with("。".as_bytes())
            || bytes[index..].starts_with("．".as_bytes())
            || bytes[index..].starts_with("｡".as_bytes())
        {
            (true, 3)
        } else if starts_ascii_case_insensitive(&bytes[index..], b"%2e")
            || starts_ascii_case_insensitive(&bytes[index..], b"%e3%80%82")
            || starts_ascii_case_insensitive(&bytes[index..], b"%ef%bc%8e")
            || starts_ascii_case_insensitive(&bytes[index..], b"%ef%bd%a1")
        {
            let width = if starts_ascii_case_insensitive(&bytes[index..], b"%2e") { 3 } else { 9 };
            (true, width)
        } else {
            (false, 1)
        };
        if component_separator {
            count = count.saturating_add(1);
        }
        index += width;
    }
    count
}

fn starts_ascii_case_insensitive(bytes: &[u8], prefix: &[u8]) -> bool {
    bytes.get(..prefix.len()).is_some_and(|bytes| bytes.eq_ignore_ascii_case(prefix))
}

fn capability_map_retained_memory_estimate(capabilities: &HashMap<String, SeedCapability>) -> usize {
    if capabilities.is_empty() {
        return 0;
    }
    let bucket_count = capabilities.capacity().saturating_mul(2).max(4);
    let mut bytes = bucket_count
        .saturating_mul(size_of::<(String, SeedCapability)>().saturating_add(1))
        .saturating_add(CAPABILITY_URL_RETAINED_BASE_BYTES);
    for (name, capability) in capabilities {
        bytes = bytes
            .saturating_add(name.capacity())
            .saturating_add(CAPABILITY_URL_RETAINED_BASE_BYTES)
            .saturating_add(url_retained_memory_estimate(&capability.url, capability.input_bytes));
    }
    bytes
}

pub(super) fn url_retained_memory_estimate(url: &Url, input_bytes: usize) -> usize {
    url.as_str()
        .len()
        .saturating_mul(CAPABILITY_URL_RETAINED_BYTES_PER_SERIALIZED_BYTE)
        .max(input_bytes)
        .saturating_add(CAPABILITY_URL_RETAINED_BASE_BYTES)
}

#[derive(Debug)]
enum CapabilityDecodeError {
    Message(String),
    Admission(http::Error),
}

impl CapabilityDecodeError {
    fn into_failure(self) -> CapabilityFailure {
        match self {
            Self::Message(message) => CapabilityFailure::Message(message),
            Self::Admission(error) => CapabilityFailure::Http(error),
        }
    }

    fn into_lookup_error(self) -> LookupError {
        match self {
            Self::Message(message) => LookupError::Permanent(message),
            Self::Admission(error) => LookupError::permanent_http(error),
        }
    }
}

fn seed_capability_llsd_limits() -> Limits {
    Limits { max_bytes: MAX_SEED_CAPABILITY_RESPONSE_BYTES, ..capability_llsd_limits() }
}

pub(super) enum LookupError {
    Http { error: http::Error, transient: bool },
    Permanent(String),
}

impl LookupError {
    fn transient(error: http::Error) -> Self {
        Self::Http { error, transient: true }
    }

    fn permanent_http(error: http::Error) -> Self {
        Self::Http { error, transient: false }
    }

    pub(super) fn is_transient(&self) -> bool {
        matches!(self, Self::Http { transient: true, .. })
    }

    pub(super) fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Http { error, .. } => error.retry_after(),
            Self::Permanent(_) => None,
        }
    }

    fn into_failure(self) -> CapabilityFailure {
        match self {
            Self::Http { error, .. } => CapabilityFailure::Http(error),
            Self::Permanent(message) => CapabilityFailure::Message(message),
        }
    }
}

impl fmt::Display for LookupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http { error, .. } => error.fmt(formatter),
            Self::Permanent(message) => formatter.write_str(message),
        }
    }
}

async fn post(
    client: &http::Client,
    url: &Url,
    capture_endpoint: Option<&str>,
    body: Vec<u8>,
    context: &str,
    max_response_bytes: usize,
) -> Result<Response, http::Error> {
    let response = client
        .post_operation_bounded_classified(
            "capability_post",
            capture_endpoint,
            url,
            "application/llsd+xml",
            Some("application/llsd+xml"),
            body,
            max_response_bytes,
        )
        .await?;
    if !response.status.is_success() {
        return Err(http::Error::http_status(response.status, response.elapsed, response.retry_after(), context));
    }
    Ok(response)
}

async fn get_bytes(
    client: &http::Client,
    url: &Url,
    capture_endpoint: Option<&str>,
    max_response_bytes: usize,
) -> Result<Response, http::Error> {
    let response = client.get_bounded_classified("capability_get", capture_endpoint, url, max_response_bytes).await?;
    if !response.status.is_success() {
        return Err(http::Error::http_status(
            response.status,
            response.elapsed,
            response.retry_after(),
            "Could not fetch capability resource",
        ));
    }
    Ok(response)
}

fn into_admitted_response(response: Response, max_response_bytes: usize, request: OwnedSemaphorePermit) -> AdmittedResponse {
    AdmittedResponse { body: response.body, max_response_bytes, _memory: response.memory, _request: request }
}

pub(super) fn logical_endpoint_label(name: &str) -> Option<String> {
    const MAX_LABEL_BYTES: usize = 128;

    if name.is_empty()
        || name.len() > MAX_LABEL_BYTES
        || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    Some(name.to_owned())
}

pub(super) fn decode_llsd_xml(body: &str, context: &str) -> Result<Value, String> {
    xml::from_slice_with_limits(body.as_bytes(), capability_llsd_limits()).map_err(|error| {
        let reason = llsd_error(error);
        format!("{context}: {reason}")
    })
}

#[cfg(test)]
pub(super) fn decode_capability_value(body: &[u8]) -> Result<Value, Error> {
    decode_capability_value_with_response_limit(body, MAX_CAPABILITY_RESPONSE_BYTES)
}

pub(super) fn decode_capability_value_with_response_limit(body: &[u8], max_response_bytes: usize) -> Result<Value, Error> {
    xml::from_slice_with_limits(body, capability_llsd_limits_with_response_limit(max_response_bytes))
        .map_err(|error| Error::Capability(format!("Could not decode an LLSD response: {}", llsd_error(error))))
}

pub(super) fn capability_llsd_limits() -> Limits {
    capability_llsd_limits_with_response_limit(MAX_CAPABILITY_RESPONSE_BYTES)
}

pub(super) fn capability_llsd_limits_with_response_limit(max_response_bytes: usize) -> Limits {
    Limits {
        max_bytes: max_response_bytes,
        max_depth: MAX_LLSD_NESTING_DEPTH,
        max_values: MAX_LLSD_XML_ELEMENTS,
        max_container_items: MAX_LLSD_XML_ELEMENTS,
        max_string_bytes: max_response_bytes,
        max_map_key_bytes: max_response_bytes,
        max_binary_bytes: max_response_bytes,
    }
}

fn llsd_error(error: llsd::Error) -> String {
    match error {
        llsd::Error::LimitExceeded("nesting depth") => format!("LLSD nesting exceeded the {MAX_LLSD_NESTING_DEPTH}-level limit."),
        llsd::Error::LimitExceeded("value count" | "container items") => {
            format!("LLSD exceeded the {MAX_LLSD_XML_ELEMENTS}-element limit.")
        }
        error => error.to_string(),
    }
}
fn string_value(value: &Value) -> Option<&str> {
    match value {
        Value::String(value) => Some(value),
        Value::Uri(value) => Some(value.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use reqwest::header::HeaderMap;
    use std::future::poll_fn;
    use std::{
        sync::{self, Arc},
        task::Poll,
    };
    use tokio::{
        spawn,
        sync::{Semaphore, oneshot},
        task::yield_now,
        time::timeout,
    };

    use super::super::{
        budget::{COMMAND_CAPACITY, CapabilityBudget, LLSD_SCAN_WORKING_BYTES},
        client::Client,
    };

    fn response(body: &[u8], memory: http::ResponseMemoryPermit) -> Response {
        Response {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: body.to_vec(),
            elapsed: Duration::ZERO,
            memory: Some(memory),
            extra_memory: None,
        }
    }

    fn capability_response_budget(limit: usize) -> http::ResponseMemoryBudget {
        http::ResponseMemoryBudget::new(limit, 0, 0, 0)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancellation_holds_seed_decode_slot_until_decoder_finishes() {
        let body = b"<llsd><map><key>Texture</key><uri>https://cap.example/texture</uri></map></llsd>";
        let names = vec!["Texture".to_owned()];
        let body_capacity = body.to_vec().capacity();
        let value = decode_capability_values(body).expect("decode seed response for URL estimate");
        let parse_estimate = capability_url_parse_memory_estimate(&value, &names).expect("URL parsing estimate");
        let bound = xml::value_allocation_bound(body, seed_capability_llsd_limits()).expect("seed response bound");
        let limit = body_capacity + LLSD_SCAN_WORKING_BYTES.max(bound) + parse_estimate;
        let budget = capability_response_budget(limit);
        let memory = budget.try_reserve(http::ResponseMemoryClass::Ordinary, body_capacity).expect("admit raw response");
        let response = response(body, memory);
        let slots = Arc::new(Semaphore::new(1));
        let request_permit = Arc::clone(&slots).acquire_owned().await.expect("ordinary request slot");
        let (entered, entered_rx) = oneshot::channel();
        let (release, release_rx) = sync::mpsc::channel();
        let task = spawn(spawn_blocking_with_permit(request_permit, move || {
            let _ = entered.send(());
            release_rx.recv().expect("release seed decoder");
            decode_requested_capabilities(response, &names)
        }));

        entered_rx.await.expect("seed decoder started");
        task.abort();
        let error = match task.await {
            Ok(_) => panic!("async waiter should be canceled"),
            Err(error) => error,
        };
        assert!(error.is_cancelled());
        assert_eq!(slots.available_permits(), 0, "the ordinary request slot stays with the decoder");
        assert!(budget.try_reserve(http::ResponseMemoryClass::Ordinary, limit).is_err(), "response memory stays with the decoder");

        release.send(()).expect("unblock seed decoder");
        timeout(Duration::from_secs(1), async {
            loop {
                let slot_released = match Arc::clone(&slots).try_acquire_owned() {
                    Ok(permit) => {
                        drop(permit);
                        true
                    }
                    Err(_) => false,
                };
                let memory_released = match budget.try_reserve(http::ResponseMemoryClass::Ordinary, limit) {
                    Ok(permit) => {
                        drop(permit);
                        true
                    }
                    Err(_) => false,
                };
                if slot_released && memory_released {
                    break;
                }
                yield_now().await;
            }
        })
        .await
        .expect("decoder completion should release the request slot and response memory");
    }

    #[test]
    fn seed_endpoint_keeps_admission() {
        let body = b"<llsd><map><key>Texture</key><uri>https://cap.example/texture</uri></map></llsd>";
        let bound = xml::value_allocation_bound(body, seed_capability_llsd_limits()).expect("valid seed response bound");
        let names = vec!["Texture".to_owned()];
        let value = decode_capability_values(body).expect("decode seed response for URL admission estimate");
        let parse_estimate = capability_url_parse_memory_estimate(&value, &names).expect("estimate URL parse admission");
        drop(value);
        let body_capacity = body.to_vec().capacity();
        let decode_charge = body_capacity + bound;
        let limit = body_capacity + LLSD_SCAN_WORKING_BYTES.max(bound) + parse_estimate;
        let budget = capability_response_budget(limit);
        let raw_memory = budget.try_reserve(http::ResponseMemoryClass::Ordinary, body_capacity).expect("admit raw response");
        let response = match admit_seed_capability_decode(response(body, raw_memory)) {
            Ok(response) => response,
            Err(error) => panic!("valid seed response should be admitted: {error}"),
        };
        let available_before_url = limit - decode_charge;
        assert!(
            budget.try_reserve(http::ResponseMemoryClass::Ordinary, available_before_url).is_ok(),
            "the scanner workspace is returned before decoding"
        );
        assert!(
            budget.try_reserve(http::ResponseMemoryClass::Ordinary, available_before_url + 1).is_err(),
            "the response permit holds raw input and the LLSD allocation bound before URL parsing"
        );

        let (mut capabilities, mut memory) =
            decode_seed_capabilities(response, &names).unwrap_or_else(|_| panic!("admitted seed response decodes"));
        let map_retained = capability_map_retained_memory_estimate(&capabilities);
        drop(budget.try_reserve(http::ResponseMemoryClass::Ordinary, limit - map_retained).expect("decoded map is charged"));
        assert!(
            budget.try_reserve(http::ResponseMemoryClass::Ordinary, limit - map_retained + 1).is_err(),
            "the response permit is reduced to the retained capability map"
        );

        let endpoint = capabilities.remove("Texture").expect("seed response includes Texture");
        assert_eq!(endpoint.url.as_str(), "https://cap.example/texture");
        drop(capabilities);
        let endpoint_retained = url_retained_memory_estimate(&endpoint.url, endpoint.input_bytes);
        memory.as_mut().expect("admitted response memory").shrink_to(endpoint_retained);
        assert!(
            budget.try_reserve(http::ResponseMemoryClass::Ordinary, limit - endpoint_retained).is_ok(),
            "only the decoded endpoint remains charged after its response map is dropped"
        );
        assert!(
            budget.try_reserve(http::ResponseMemoryClass::Ordinary, limit - endpoint_retained + 1).is_err(),
            "the endpoint charge remains held until ownership is released"
        );
        drop(endpoint);
        drop(memory);
        drop(budget.try_reserve(http::ResponseMemoryClass::Ordinary, limit).expect("seed endpoint admission returns after drop"));
    }

    #[test]
    fn seed_url_parse_admission_exhaustion_returns_memory() {
        let body = b"<llsd><map><key>Texture</key><uri>https://cap.example/texture</uri></map></llsd>";
        let names = vec!["Texture".to_owned()];
        let value = decode_capability_values(body).expect("decode seed response for URL admission estimate");
        let parse_estimate = capability_url_parse_memory_estimate(&value, &names).expect("estimate URL parse admission");
        drop(value);
        let bound = xml::value_allocation_bound(body, seed_capability_llsd_limits()).expect("valid seed response bound");
        let body_capacity = body.to_vec().capacity();
        let limit = body_capacity + LLSD_SCAN_WORKING_BYTES.max(bound) + parse_estimate - 1;
        let budget = capability_response_budget(limit);
        let raw_memory = budget.try_reserve(http::ResponseMemoryClass::Ordinary, body_capacity).expect("admit raw response");
        let response = match admit_seed_capability_decode(response(body, raw_memory)) {
            Ok(response) => response,
            Err(error) => panic!("admit LLSD decoding before URL parsing: {error}"),
        };
        let scanner_excess = LLSD_SCAN_WORKING_BYTES.saturating_sub(bound);
        let scanner_charge = budget
            .try_reserve(http::ResponseMemoryClass::Ordinary, scanner_excess)
            .expect("keep scanner headroom unavailable to URL parsing");

        let error = match decode_seed_capabilities(response, &names) {
            Err(error) => error,
            Ok(_) => panic!("URL parsing should fail when its admission estimate is unavailable"),
        };

        assert!(matches!(error, LookupError::Http { transient: false, .. }));
        drop(scanner_charge);
        drop(budget.try_reserve(http::ResponseMemoryClass::Ordinary, limit).expect("URL admission failure returns all response memory"));
    }

    #[test]
    fn malformed_seed_body_releases_admission() {
        let body = b"<llsd><map></llsd>";
        let limit = body.len() + LLSD_SCAN_WORKING_BYTES + 1024;
        let budget = capability_response_budget(limit);
        let raw_memory = budget.try_reserve(http::ResponseMemoryClass::Ordinary, body.len()).expect("admit malformed response");

        let error = match admit_seed_capability_decode(response(body, raw_memory)) {
            Err(error) => error,
            Ok(_) => panic!("malformed XML should fail allocation preflight"),
        };

        assert!(error.to_string().contains("response allocation preflight failed"));
        drop(budget.try_reserve(http::ResponseMemoryClass::Ordinary, limit).expect("malformed response admission is returned"));
    }

    #[test]
    fn seed_capability_preflight_enforces_seed_body_limit() {
        let oversized_body = vec![b' '; MAX_SEED_CAPABILITY_RESPONSE_BYTES + 1];
        let limit = oversized_body.len() + LLSD_SCAN_WORKING_BYTES;
        let budget = capability_response_budget(limit);
        let raw_memory =
            budget.try_reserve(http::ResponseMemoryClass::Ordinary, oversized_body.len()).expect("admit oversized response fixture");

        let error = match admit_seed_capability_decode(response(&oversized_body, raw_memory)) {
            Err(error) => error,
            Ok(_) => panic!("an oversized seed response should fail allocation preflight"),
        };

        assert!(error.to_string().contains("input bytes limit exceeded"), "{error}");
        drop(budget.try_reserve(http::ResponseMemoryClass::Ordinary, limit).expect("oversized response admission is returned"));
    }

    #[test]
    fn seed_decode_exhaustion_releases_admission() {
        let body = b"<llsd><map><key>Texture</key><uri>https://cap.example/texture</uri></map></llsd>";
        let bound = xml::value_allocation_bound(body, seed_capability_llsd_limits()).expect("valid seed response bound");
        let limit = body.len() + LLSD_SCAN_WORKING_BYTES.max(bound) - 1;
        let budget = capability_response_budget(limit);
        let raw_memory = budget.try_reserve(http::ResponseMemoryClass::Ordinary, body.len()).expect("admit raw response");

        let error = match admit_seed_capability_decode(response(body, raw_memory)) {
            Err(error) => error,
            Ok(_) => panic!("decode should be rejected when allocation admission is unavailable"),
        };

        assert!(matches!(error, LookupError::Http { transient: false, .. }));
        drop(budget.try_reserve(http::ResponseMemoryClass::Ordinary, limit).expect("failed decode admission is returned"));
    }

    #[tokio::test]
    async fn cancel_before_admission_uses_no_slot() {
        let budget = CapabilityBudget::new();
        let clients = (0..4)
            .map(|_| {
                Client::new_with_status_and_budget(
                    Url::parse("http://127.0.0.1:9/seed").expect("seed URL"),
                    None,
                    NetworkStatusHandle::default(),
                    budget.clone(),
                )
                .expect("active Tokio runtime")
            })
            .collect::<Vec<_>>();
        let url = CapabilityUrl::parse("http://127.0.0.1:9/post").expect("capability URL");
        let payload = Value::String("request".into());

        for client in &clients {
            let mut request = Box::pin(client.post(&url, &payload));
            assert!(poll_fn(|context| Poll::Ready(request.as_mut().poll(context))).await.is_pending());
            drop(request);
        }

        yield_now().await;
        assert_eq!(budget.ordinary_requests.available_permits(), MAX_CONCURRENT_CAPABILITY_REQUESTS);
        assert!(clients.iter().all(|client| client.commands.capacity() == COMMAND_CAPACITY));
    }

    #[tokio::test]
    async fn deadline_covers_admission_wait() {
        let budget = CapabilityBudget::new();
        let client = Client::new_with_status_and_budget(
            Url::parse("http://127.0.0.1:9/seed").expect("seed URL"),
            None,
            NetworkStatusHandle::default(),
            budget.clone(),
        )
        .expect("active Tokio runtime")
        .with_request_timeout(Duration::from_millis(30));
        let url = CapabilityUrl::parse("http://127.0.0.1:9/capability").expect("capability URL");
        let held = Arc::clone(&budget.ordinary_requests)
            .acquire_many_owned(MAX_CONCURRENT_CAPABILITY_REQUESTS as u32)
            .await
            .expect("hold every ordinary request slot");
        let started = Instant::now();

        let error = client.get_bytes(&url).await.expect_err("a request waiting for admission must hit its deadline");

        assert!(matches!(
            &error,
            Error::CapabilityHttp(http::Error::Timeout { elapsed, status: None, .. })
                if elapsed.as_millis() >= 20 && *elapsed < Duration::from_millis(500)
        ));
        assert!(started.elapsed() < Duration::from_millis(500), "the admission wait should be bounded by the configured deadline");
        assert_eq!(budget.ordinary_requests.available_permits(), 0);
        drop(held);
        assert_eq!(budget.ordinary_requests.available_permits(), MAX_CONCURRENT_CAPABILITY_REQUESTS);
    }
}

#[cfg(test)]
pub(crate) mod allocation_measurement {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
        hint::black_box,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use super::*;

    struct CountingAllocator;

    #[derive(Clone, Copy, Default)]
    struct AllocationCount {
        live: i128,
        peak: i128,
        active: bool,
    }

    thread_local! {
        static ALLOCATION_COUNT: Cell<AllocationCount> = const { Cell::new(AllocationCount { live: 0, peak: 0, active: false }) };
    }

    static PROCESS_LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
    static PROCESS_PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);
    static PROCESS_MEASUREMENT_ACTIVE: AtomicBool = AtomicBool::new(false);

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;

    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc(layout) };
            if !ptr.is_null() {
                account(layout.size() as i128);
            }
            ptr
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc_zeroed(layout) };
            if !ptr.is_null() {
                account(layout.size() as i128);
            }
            ptr
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) };
            account(-(layout.size() as i128));
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let resized = unsafe { System.realloc(ptr, layout, new_size) };
            if !resized.is_null() {
                account(new_size as i128);
                account(-(layout.size() as i128));
            }
            resized
        }
    }

    fn account(bytes: i128) {
        if bytes > 0 {
            let bytes = bytes as usize;
            let live = PROCESS_LIVE_BYTES.fetch_add(bytes, Ordering::Relaxed).saturating_add(bytes);
            if PROCESS_MEASUREMENT_ACTIVE.load(Ordering::Acquire) {
                PROCESS_PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
            }
        } else if bytes < 0 {
            PROCESS_LIVE_BYTES.fetch_sub((-bytes) as usize, Ordering::Relaxed);
        }

        let _ = ALLOCATION_COUNT.try_with(|count| {
            let mut state = count.get();
            state.live += bytes;
            if state.active {
                state.peak = state.peak.max(state.live);
            }
            count.set(state);
        });
    }

    pub(crate) fn measure<T>(operation: impl FnOnce() -> T) -> (T, i128, i128, u128) {
        let baseline = ALLOCATION_COUNT.with(|count| {
            let mut state = count.get();
            state.active = true;
            state.peak = state.live;
            count.set(state);
            state.live
        });
        let started = Instant::now();
        let value = operation();
        let elapsed = started.elapsed().as_nanos();
        let (live, peak) = ALLOCATION_COUNT.with(|count| {
            let mut state = count.get();
            state.active = false;
            count.set(state);
            (state.live - baseline, state.peak - baseline)
        });
        (value, live, peak, elapsed)
    }

    pub(crate) struct ProcessMeasurement {
        baseline: usize,
        started: Instant,
        active: bool,
    }

    pub(crate) fn begin_process_measurement() -> ProcessMeasurement {
        assert!(!PROCESS_MEASUREMENT_ACTIVE.load(Ordering::Acquire), "process allocation measurement is already active");
        let baseline = PROCESS_LIVE_BYTES.load(Ordering::Relaxed);
        PROCESS_PEAK_BYTES.store(baseline, Ordering::Relaxed);
        PROCESS_MEASUREMENT_ACTIVE.store(true, Ordering::Release);
        ProcessMeasurement { baseline, started: Instant::now(), active: true }
    }

    impl ProcessMeasurement {
        pub(crate) fn finish(mut self) -> (usize, i128, usize, u128) {
            PROCESS_MEASUREMENT_ACTIVE.store(false, Ordering::Release);
            self.active = false;
            let live = PROCESS_LIVE_BYTES.load(Ordering::Relaxed) as i128 - self.baseline as i128;
            let peak = PROCESS_PEAK_BYTES.load(Ordering::Relaxed);
            (self.baseline, live, peak, self.started.elapsed().as_nanos())
        }
    }

    impl Drop for ProcessMeasurement {
        fn drop(&mut self) {
            if self.active {
                PROCESS_MEASUREMENT_ACTIVE.store(false, Ordering::Release);
            }
        }
    }

    fn body(grants: &[(String, String)]) -> (Vec<u8>, Vec<String>) {
        let mut xml = String::from("<llsd><map>");
        let mut names = Vec::with_capacity(grants.len());
        for (name, url) in grants {
            names.push(name.clone());
            xml.push_str("<key>");
            xml.push_str(name);
            xml.push_str("</key><string>");
            xml.push_str(url);
            xml.push_str("</string>");
        }
        xml.push_str("</map></llsd>");
        (xml.into_bytes(), names)
    }

    #[test]
    fn url_admission_estimate_covers_measured_corpus() {
        let mut grants = vec![
            ("SpacePath".to_owned(), format!("https://cap.example/{}", " ".repeat(32 * 1024))),
            ("DottedHost".to_owned(), format!("http://{}", "1.".repeat(4096))),
            ("EncodedDottedHost".to_owned(), format!("http://{}1", "1%2E1%2e".repeat(2048))),
            ("UnicodeDottedHost".to_owned(), format!("http://{}1", "1。".repeat(4096))),
            ("IdnaHost".to_owned(), format!("https://{}.example/{}", "ü".repeat(12), "e\u{301}".repeat(1024))),
            ("QueryApostrophes".to_owned(), format!("https://cap.example/?q={}", "'".repeat(8192))),
        ];
        grants
            .extend((0..MAX_CAPABILITY_NAMES).map(|index| (format!("Capability{index:03}"), format!("https://cap.example/asset/{index}"))));
        let cases = [
            ("space path", vec![grants[0].clone()]),
            ("dotted host", vec![grants[1].clone()]),
            ("percent-dotted host", vec![grants[2].clone()]),
            ("Unicode-dotted host", vec![grants[3].clone()]),
            ("IDNA and combining path", vec![grants[4].clone()]),
            ("query apostrophes", vec![grants[5].clone()]),
            ("maximum grants", grants[6..].to_vec()),
        ];

        for (name, grants) in cases {
            let (body, names) = body(&grants);
            let limits = seed_capability_llsd_limits();
            let value = decode_capability_values(&body).expect("decode corpus for URL estimate");
            let url_estimate = capability_url_parse_memory_estimate(&value, &names).expect("estimate URL parser memory");
            drop(value);
            let llsd_bound = xml::value_allocation_bound(&body, limits).expect("bound capability LLSD value");
            let (scan_result, scan_live, scan_peak, scan_ns) =
                measure(|| xml::value_allocation_bound(black_box(&body), limits).expect("scan capability LLSD"));
            assert_eq!(scan_result, llsd_bound);
            assert_eq!(scan_live, 0, "scanner allocations return for {name}");
            let (decoded, retained, peak, decode_ns) = measure(|| {
                decode_capabilities_with_memory(black_box(&body), black_box(&names), None, |_, url, input_bytes| SeedCapability {
                    url,
                    input_bytes,
                })
            });
            assert!(peak >= retained.max(0));
            let url_peak = peak.saturating_sub(llsd_bound as i128).max(retained.max(0));
            assert!(
                url_estimate as i128 >= url_peak,
                "URL estimate undercount for {name}: peak={peak}, LLSD={llsd_bound}, retained={retained}, estimate={url_estimate}"
            );
            eprintln!(
                "{name}: input={} B, retained={retained} B, peak={peak} B, LLSD={llsd_bound} B, scan_peak={scan_peak} B, URL estimate={url_estimate} B, peak/LLSD={:.2}, estimate/observed={:.2}, scan/decode={:.2} ({scan_ns}/{decode_ns} ns)",
                body.len(),
                peak as f64 / llsd_bound.max(1) as f64,
                url_estimate as f64 / url_peak.max(1) as f64,
                scan_ns as f64 / decode_ns.max(1) as f64,
            );
            if let Ok(capabilities) = &decoded {
                let retained_estimate = capability_map_retained_memory_estimate(capabilities);
                assert!(
                    retained_estimate as i128 >= retained.max(0),
                    "retained map estimate undercount for {name}: retained={retained}, estimate={retained_estimate}"
                );
            }
            drop(decoded);
            if grants.len() == 1 {
                let (url, retained, peak, _) = measure(|| Url::parse(black_box(&grants[0].1)));
                if let Ok(url) = url {
                    let retained_estimate = url_retained_memory_estimate(&url, grants[0].1.len());
                    assert!(
                        retained_estimate as i128 >= retained.max(0),
                        "retained URL estimate undercount for {name}: retained={retained}, peak={peak}, estimate={retained_estimate}"
                    );
                }
            }
        }
    }
}
