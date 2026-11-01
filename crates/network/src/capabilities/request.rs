use std::{
    collections::HashMap,
    ops::Range,
    str::from_utf8,
    time::{Duration, Instant},
};

use llsd::{Limits, Value, xml};
use reqwest::{StatusCode, Url, header::CONTENT_RANGE};
use tokio::time;

use super::budget::{
    MAX_CAPABILITY_NAME_BYTES, MAX_CAPABILITY_NAMES, MAX_CAPABILITY_NAMES_BYTES, MAX_CAPABILITY_REQUEST_BODY_BYTES,
    MAX_CAPABILITY_RESPONSE_BYTES, MAX_LLSD_NESTING_DEPTH, MAX_LLSD_XML_ELEMENTS, MAX_SEED_CAPABILITY_RESPONSE_BYTES,
};
use super::client::{ContentRange, RangeResponse};
use crate::{
    error::Error,
    http::{self, ErrorKind, RedirectPolicy, Response},
};

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

    pub(super) fn error(self) -> Error {
        let elapsed = self.started.elapsed();
        Error::CapabilityHttp(http::Error::timeout(
            elapsed,
            format!("Capability request exceeded its deadline after {} ms.", elapsed.as_millis()),
        ))
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

pub(super) fn is_transient_seed_lookup(error: &Error) -> bool {
    let Error::CapabilityHttp(error) = error else {
        return false;
    };

    match error.kind {
        ErrorKind::Transport | ErrorKind::Timeout => true,
        ErrorKind::HttpStatus => error.status.is_some_and(|status| status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS),
        ErrorKind::Cancelled | ErrorKind::ResourceExhausted | ErrorKind::InvalidResponse | ErrorKind::InvalidUrl => false,
    }
}

pub(super) fn seed_lookup_retry_after(error: &Error) -> Option<Duration> {
    match error {
        Error::CapabilityHttp(error) => error.retry_after,
        _ => None,
    }
}

pub(super) async fn request_capabilities_response(
    client: &http::Client,
    seed: &Url,
    names: &[String],
    max_response_bytes: usize,
) -> Result<Response, Error> {
    let payload = Value::Array(names.iter().cloned().map(Value::String).collect());
    let body = xml::to_vec_with_max_bytes(&payload, MAX_CAPABILITY_REQUEST_BODY_BYTES)
        .map_err(|error| Error::Capability(format!("Could not encode an LLSD request: {error}")))?;
    let response = client
        .post("seed_capability", seed, "application/llsd+xml", Some("application/llsd+xml"), body, max_response_bytes)
        .await
        .map_err(Error::CapabilityHttp)?;
    if !response.status.is_success() {
        return Err(Error::CapabilityHttp(http::Error::http_status(
            response.status,
            response.elapsed,
            response.retry_after(),
            "Could not request capabilities",
        )));
    }
    Ok(response)
}

pub(super) fn decode_seed_capabilities(response: Response, names: &[String]) -> Result<HashMap<String, Url>, Error> {
    let body = from_utf8(&response.body)
        .map_err(|error| Error::Capability(format!("Could not request capabilities: response was not UTF-8: {error}")))?;
    let value = xml::from_slice_with_limits(body.as_bytes(), seed_capability_llsd_limits())
        .map_err(|error| Error::Capability(format!("Could not request capabilities: {}", llsd_error(error))))?;
    decode_capabilities_from_value(&value, names).map_err(Error::Capability)
}

fn decode_capabilities_from_value(response: &Value, names: &[String]) -> Result<HashMap<String, Url>, String> {
    let fields = response.as_map().ok_or_else(|| "Second Life returned an invalid capability response.".to_string())?;
    let grant_count = names.iter().filter(|name| fields.contains_key(name.as_str())).count();
    let mut granted = HashMap::with_capacity(grant_count);
    for name in names {
        let Some(value) = fields.get(name) else {
            continue;
        };
        let value = string_value(value).ok_or_else(|| format!("Second Life returned an invalid {name} capability."))?;
        let url = Url::parse(value).map_err(|_| format!("Second Life returned an invalid {name} capability."))?;
        http::validate_http_url(&url).map_err(|_| format!("Second Life returned an invalid {name} capability."))?;
        granted.insert(name.clone(), url);
    }
    Ok(granted)
}

pub(super) async fn post_capability(
    client: &http::Client,
    url: &Url,
    body: Vec<u8>,
    max_response_bytes: usize,
) -> Result<Response, http::Error> {
    let response =
        client.post("capability_post", url, "application/llsd+xml", Some("application/llsd+xml"), body, max_response_bytes).await?;
    if !response.status.is_success() {
        return Err(http::Error::http_status(response.status, response.elapsed, response.retry_after(), "Capability request failed"));
    }
    Ok(response)
}

pub(super) async fn patch_capability(
    client: &http::Client,
    url: &Url,
    body: Vec<u8>,
    max_response_bytes: usize,
) -> Result<Response, http::Error> {
    let response =
        client.patch("capability_patch", url, "application/llsd+xml", Some("application/llsd+xml"), body, max_response_bytes).await?;
    if !response.status.is_success() {
        return Err(http::Error::http_status(response.status, response.elapsed, response.retry_after(), "Capability request failed"));
    }
    Ok(response)
}

pub(super) async fn delete_capability(client: &http::Client, url: &Url, max_response_bytes: usize) -> Result<Response, http::Error> {
    let response = client.delete("capability_delete", url, Some("application/llsd+xml"), max_response_bytes).await?;
    if !response.status.is_success() {
        return Err(http::Error::http_status(response.status, response.elapsed, response.retry_after(), "Capability request failed"));
    }
    Ok(response)
}

pub(super) async fn get_capability_bytes(
    client: &http::Client,
    url: &Url,
    accept: Option<&str>,
    max_response_bytes: usize,
    redirect_policy: RedirectPolicy,
) -> Result<Response, http::Error> {
    let response = match (accept, redirect_policy) {
        (Some(accept), RedirectPolicy::None) => client.get_with_accept("capability_get", url, accept, max_response_bytes).await?,
        (None, RedirectPolicy::None) => client.get("capability_get", url, max_response_bytes).await?,
        (Some(accept), RedirectPolicy::ViewerAsset) => {
            client.get_with_accept_and_redirect_policy("capability_get", url, accept, max_response_bytes, redirect_policy).await?
        }
        (None, redirect_policy) => client.get_with_redirect_policy("capability_get", url, max_response_bytes, redirect_policy).await?,
    };
    if !response.status.is_success() || response.status == reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(http::Error::http_status(
            response.status,
            response.elapsed,
            response.retry_after(),
            "Could not fetch capability resource",
        ));
    }
    Ok(response)
}

pub(super) async fn get_capability_range_bytes(
    client: &http::Client,
    url: &Url,
    range: Range<u64>,
    max_response_bytes: usize,
    redirect_policy: RedirectPolicy,
) -> Result<RangeResponse, http::Error> {
    let end = range.end - 1;
    let value = format!("bytes={}-{}", range.start, end);
    let response = client.get_range_with_redirect_policy("capability_range_get", url, &value, max_response_bytes, redirect_policy).await?;
    match response.status {
        StatusCode::OK => Ok(RangeResponse::Full(response.body)),
        StatusCode::PARTIAL_CONTENT => {
            let mut values = response.headers.get_all(CONTENT_RANGE).iter();
            let content_range = match values.next() {
                Some(value) if values.next().is_none() => Some(parse_content_range(value).ok_or_else(|| {
                    http::Error::invalid_response(
                        response.status,
                        response.elapsed,
                        "Capability range response has an invalid Content-Range.",
                    )
                })?),
                None => None,
                Some(_) => {
                    return Err(http::Error::invalid_response(
                        response.status,
                        response.elapsed,
                        "Capability range response has multiple Content-Range headers.",
                    ));
                }
            };
            Ok(RangeResponse::Partial { bytes: response.body, content_range })
        }
        status => Err(http::Error::http_status(status, response.elapsed, response.retry_after(), "Could not fetch capability byte range")),
    }
}

fn parse_content_range(value: &reqwest::header::HeaderValue) -> Option<ContentRange> {
    let value = value.to_str().ok()?;
    let (unit, value) = value.split_once(' ')?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let (bounds, total) = value.split_once('/')?;
    let (start, end) = bounds.split_once('-')?;
    let start = start.parse().ok()?;
    let end = end.parse().ok()?;
    let total = if total == "*" { None } else { Some(total.parse().ok()?) };
    (start <= end && total.is_none_or(|total| end < total)).then_some(ContentRange { start, end, total })
}

pub(super) fn decode_llsd_xml(body: &str, context: &str) -> Result<Value, String> {
    xml::from_slice_with_limits(body.as_bytes(), capability_llsd_limits()).map_err(|error| {
        let reason = llsd_error(error);
        format!("{context}: {reason}")
    })
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

fn seed_capability_llsd_limits() -> Limits {
    Limits { max_bytes: MAX_SEED_CAPABILITY_RESPONSE_BYTES, ..capability_llsd_limits() }
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
mod tests {
    use std::{future::poll_fn, sync::Arc, task::Poll};

    use llsd::Value;
    use tokio::{task::yield_now, time};
    use url::Url;

    use crate::{
        Error,
        capabilities::{CapabilityBudget, CapabilityUrl, Client},
        http::{self, ErrorKind},
    };

    use crate::capabilities::budget::{MAX_CAPABILITY_RESPONSE_BYTES, MAX_CONCURRENT_CAPABILITY_REQUESTS, REQUEST_TIMEOUT};

    #[tokio::test]
    async fn cancel_before_admission_releases_no_slot() {
        let budget = CapabilityBudget::new();
        let clients = (0..4)
            .map(|_| {
                Client::new_with_budget(Url::parse("http://127.0.0.1:9/seed").expect("seed URL"), budget.clone())
                    .expect("active Tokio runtime")
            })
            .collect::<Vec<_>>();
        let url = CapabilityUrl::parse("http://127.0.0.1:9/post").expect("capability URL");
        let payload = Value::String("request".into());
        let held = Arc::clone(&budget.ordinary_requests)
            .acquire_many_owned(MAX_CONCURRENT_CAPABILITY_REQUESTS as u32)
            .await
            .expect("hold every ordinary request slot");

        for client in &clients {
            let mut request = Box::pin(client.post_with_response_limit(&url, &payload, MAX_CAPABILITY_RESPONSE_BYTES));
            assert!(poll_fn(|context| Poll::Ready(request.as_mut().poll(context))).await.is_pending());
            drop(request);
        }
        drop(held);
        yield_now().await;

        assert_eq!(budget.ordinary_requests.available_permits(), MAX_CONCURRENT_CAPABILITY_REQUESTS);
    }

    #[tokio::test(start_paused = true)]
    async fn deadline_covers_admission_wait() {
        let budget = CapabilityBudget::new();
        let client = Client::new_with_budget(Url::parse("http://127.0.0.1:9/seed").expect("seed URL"), budget.clone())
            .expect("active Tokio runtime");
        let url = CapabilityUrl::parse("http://127.0.0.1:9/capability").expect("capability URL");
        let held = Arc::clone(&budget.ordinary_requests)
            .acquire_many_owned(MAX_CONCURRENT_CAPABILITY_REQUESTS as u32)
            .await
            .expect("hold every ordinary request slot");

        let mut request = Box::pin(client.get_bytes_with_response_limit(&url, MAX_CAPABILITY_RESPONSE_BYTES));
        assert!(poll_fn(|context| Poll::Ready(request.as_mut().poll(context))).await.is_pending());
        time::advance(REQUEST_TIMEOUT).await;
        let error = request.await.expect_err("a request waiting for admission must hit its deadline");

        std::assert_matches!(error, Error::CapabilityHttp(http::Error { kind: ErrorKind::Timeout, status: None, .. }));
        assert_eq!(budget.ordinary_requests.available_permits(), 0);
        drop(held);
        assert_eq!(budget.ordinary_requests.available_permits(), MAX_CONCURRENT_CAPABILITY_REQUESTS);
    }
}
