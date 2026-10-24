use std::{future::poll_fn, sync::Arc, task::Poll, time::Duration};

use llsd::Value;
use tokio::task::yield_now;
use url::Url;

use crate::{
    Error,
    capabilities::{CapabilityBudget, CapabilityUrl, Client},
    fixtures::http::client_with_request_timeout,
    http::{self, ErrorKind},
};

use crate::capabilities::budget::{MAX_CAPABILITY_RESPONSE_BYTES, MAX_CONCURRENT_CAPABILITY_REQUESTS};

#[tokio::test]
async fn cancel_before_admission_releases_no_slot() {
    let budget = CapabilityBudget::new();
    let clients = (0..4)
        .map(|_| {
            Client::new_with_budget(Url::parse("http://127.0.0.1:9/seed").expect("seed URL"), budget.clone()).expect("active Tokio runtime")
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
    let client = client_with_request_timeout(
        Client::new_with_budget(Url::parse("http://127.0.0.1:9/seed").expect("seed URL"), budget.clone()).expect("active Tokio runtime"),
        Duration::from_millis(30),
    );
    let url = CapabilityUrl::parse("http://127.0.0.1:9/capability").expect("capability URL");
    let held = Arc::clone(&budget.ordinary_requests)
        .acquire_many_owned(MAX_CONCURRENT_CAPABILITY_REQUESTS as u32)
        .await
        .expect("hold every ordinary request slot");

    let error = client
        .get_bytes_with_response_limit(&url, MAX_CAPABILITY_RESPONSE_BYTES)
        .await
        .expect_err("a request waiting for admission must hit its deadline");

    std::assert_matches!(
        error,
        Error::CapabilityHttp(http::Error { kind: ErrorKind::Timeout, status: None, elapsed, .. })
            if elapsed >= Duration::from_millis(20) && elapsed < Duration::from_millis(500)
    );
    assert_eq!(budget.ordinary_requests.available_permits(), 0);
    drop(held);
    assert_eq!(budget.ordinary_requests.available_permits(), MAX_CONCURRENT_CAPABILITY_REQUESTS);
}
