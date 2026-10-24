use std::{
    collections::HashMap,
    str::from_utf8,
    time::{Duration, Instant},
};

use llsd::{Limits, Value, xml};
use reqwest::Url;
use tokio::{sync::watch, time};

use super::budget::{
    MAX_CAPABILITY_NAME_BYTES, MAX_CAPABILITY_NAMES, MAX_CAPABILITY_NAMES_BYTES, MAX_CAPABILITY_REQUEST_BODY_BYTES,
    MAX_CAPABILITY_RESPONSE_BYTES, MAX_LLSD_NESTING_DEPTH, MAX_LLSD_XML_ELEMENTS, MAX_SEED_CAPABILITY_RESPONSE_BYTES,
};
use crate::{
    error::Error,
    http::{self, Response},
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
    matches!(error, Error::CapabilityHttp(error) if error.is_transient_seed_lookup())
}

pub(super) fn seed_lookup_retry_after(error: &Error) -> Option<Duration> {
    match error {
        Error::CapabilityHttp(error) => error.retry_after(),
        _ => None,
    }
}

pub(super) async fn wait_for_session_close(mut session_closed: watch::Receiver<bool>) {
    while !*session_closed.borrow() {
        if session_closed.changed().await.is_err() {
            break;
        }
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

pub(super) async fn get_capability_bytes(client: &http::Client, url: &Url, max_response_bytes: usize) -> Result<Response, http::Error> {
    let response = client.get("capability_get", url, max_response_bytes).await?;
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
pub(crate) mod allocation_measurement {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
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
        ALLOCATION_COUNT
            .try_with(|count| {
                let mut state = count.get();
                state.live += bytes;
                if state.active {
                    state.peak = state.peak.max(state.live);
                }
                count.set(state);
            })
            .unwrap_or_default();
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
}

#[cfg(test)]
mod tests {
    use std::{future::poll_fn, sync::Arc, task::Poll, time::Duration};

    use llsd::Value;
    use tokio::task::yield_now;
    use url::Url;

    use crate::{
        Error,
        capabilities::{CapabilityBudget, CapabilityUrl, Client},
        http::{self, ErrorKind},
        status::NetworkStatusHandle,
    };

    use super::super::budget::{MAX_CAPABILITY_RESPONSE_BYTES, MAX_CONCURRENT_CAPABILITY_REQUESTS};

    #[tokio::test]
    async fn cancel_before_admission_releases_no_slot() {
        let budget = CapabilityBudget::new();
        let clients = (0..4)
            .map(|_| {
                Client::new_with_status_and_budget(
                    Url::parse("http://127.0.0.1:9/seed").expect("seed URL"),
                    NetworkStatusHandle::default(),
                    budget.clone(),
                )
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

    #[tokio::test]
    async fn deadline_covers_admission_wait() {
        let budget = CapabilityBudget::new();
        let client = Client::new_with_status_and_budget(
            Url::parse("http://127.0.0.1:9/seed").expect("seed URL"),
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

        let error = client
            .get_bytes_with_response_limit(&url, MAX_CAPABILITY_RESPONSE_BYTES)
            .await
            .expect_err("a request waiting for admission must hit its deadline");

        assert!(matches!(
            error,
            Error::CapabilityHttp(http::Error { kind: ErrorKind::Timeout, status: None, elapsed, .. })
                if elapsed >= Duration::from_millis(20) && elapsed < Duration::from_millis(500)
        ));
        assert_eq!(budget.ordinary_requests.available_permits(), 0);
        drop(held);
        assert_eq!(budget.ordinary_requests.available_permits(), MAX_CONCURRENT_CAPABILITY_REQUESTS);
    }
}
