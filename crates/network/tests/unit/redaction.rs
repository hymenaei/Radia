use std::time::Duration;

use reqwest::{
    StatusCode, Url,
    header::{HeaderMap, HeaderValue},
};

use crate::{
    Error, Event,
    event::SimulatorId,
    http::{Error as HttpError, Response},
    status::{NetworkLifecycle, NetworkStatusSnapshot},
};

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

#[test]
fn event_debug_hides_server_errors_and_endpoints() {
    let event = Event::Failed("server-session-secret".into());
    assert!(!format!("{event:?}").contains("server-session-secret"));

    let event = Event::CapabilityEvent {
        simulator_id: SimulatorId::new(1),
        source: "192.0.2.90:13000".parse().expect("valid endpoint"),
        message: "session-event-secret".into(),
        body: llsd::Value::String("event-body-secret".into()),
    };
    let debug = format!("{event:?}");
    assert!(!debug.contains("192.0.2.90"));
    assert!(!debug.contains("session-event-secret"));
    assert!(!debug.contains("event-body-secret"));

    let event = Event::CapabilityError {
        simulator_id: SimulatorId::new(2),
        source: "192.0.2.91:13000".parse().expect("valid endpoint"),
        capability: "private-capability-name".into(),
        reason: "origin-error-secret".into(),
    };
    let debug = format!("{event:?}");
    for secret in ["192.0.2.91", "private-capability-name", "origin-error-secret"] {
        assert!(!debug.contains(secret));
    }
    assert!(matches!(&event, Event::CapabilityError { reason, .. } if reason == "origin-error-secret"));

    let event = Event::SimulatorFailed {
        simulator_id: SimulatorId::new(3),
        region_handle: Some(0x1234),
        endpoint: "192.0.2.92:13000".parse().expect("valid endpoint"),
        reason: "simulator-error-secret".into(),
    };
    let debug = format!("{event:?}");
    for secret in ["192.0.2.92", "4660", "simulator-error-secret"] {
        assert!(!debug.contains(secret));
    }
}

#[test]
fn error_debug_redacts_messages_but_display_preserves_them() {
    let error = Error::Capability("capability-secret".into());
    assert!(!format!("{error:?}").contains("capability-secret"));
    assert!(format!("{error}").contains("capability-secret"));

    let error = HttpError::Message("http-response-secret".into());
    assert!(!format!("{error:?}").contains("http-response-secret"));
    assert!(format!("{error}").contains("http-response-secret"));
}

#[test]
fn capture_event_debug_redacts_local_details() {
    let finished = Event::CaptureFinished {
        path: std::path::PathBuf::from("C:/users/alice/captures/private-session.vcap"),
        error: Some("could not write private-session-token".into()),
    };
    let interrupted = Event::CaptureInterrupted {
        path: std::path::PathBuf::from("C:/users/alice/captures/private-session.vcap"),
        error: Some("could not write private-session-token".into()),
    };

    for debug in [format!("{finished:?}"), format!("{interrupted:?}")] {
        assert!(!debug.contains("alice"));
        assert!(!debug.contains("private-session"));
        assert!(!debug.contains("private-session-token"));
        assert!(debug.contains("[REDACTED]"));
    }
}

#[test]
fn status_debug_redacts_secrets() {
    let snapshot = NetworkStatusSnapshot {
        lifecycle: NetworkLifecycle::Connected,
        simulator_endpoint: Some("192.0.2.77:13000".parse().expect("valid endpoint")),
        circuit_established: true,
        udp_packets_sent: 1,
        udp_packets_received: 2,
        http_requests: 3,
        http_responses: 3,
        capability_count: 4,
        reliable_packets_pending: 0,
        reliable_retransmits: 1,
        last_error: Some("server-error-secret".into()),
    };

    let debug = format!("{snapshot:?}");

    assert!(!debug.contains("192.0.2.77"));
    assert!(!debug.contains("server-error-secret"));
    assert!(format!("{snapshot}").contains("server-error-secret"));

    let status = crate::status::NetworkStatusHandle::new();
    status.set_simulator_endpoint("192.0.2.78:13000".parse().expect("valid endpoint"));
    status.record_error("server-handle-error-secret");
    let debug = format!("{status:?}");
    assert!(!debug.contains("192.0.2.78"));
    assert!(!debug.contains("server-handle-error-secret"));
}
