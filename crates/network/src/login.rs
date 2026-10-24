pub use protocol::login::StartLocation;

use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    str::from_utf8,
    sync::{Arc, OnceLock},
    time::Duration,
};

use protocol::login::{self, ResponseData};
use reqwest::{StatusCode, header::CONTENT_TYPE};
use tokio::{sync::Semaphore, time};
use url::Url;
use uuid::Uuid;

use crate::{
    capabilities::spawn_blocking_with_permit,
    http::{Client, validate_http_url},
    status::{NetworkLifecycle, NetworkStatusHandle},
};

const LOGIN_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_LOGIN_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_LOGIN_XML_DEPTH: usize = 64;
const MAX_CONCURRENT_LOGIN_OPERATIONS: usize = 2;

static LOGIN_OPERATION_SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();

#[derive(Clone)]
pub struct AgentSession {
    pub first_name: String,
    pub last_name: String,
    pub grid: Grid,
    pub message: String,
    pub simulator_address: SocketAddr,
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub circuit_code: u32,
    pub(crate) seed_capability: Url,
}

impl fmt::Debug for AgentSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentSession")
            .field("first_name", &"[REDACTED]")
            .field("last_name", &"[REDACTED]")
            .field("grid", &self.grid)
            .field("message", &"[OMITTED]")
            .field("simulator_address", &"[REDACTED]")
            .field("agent_id", &"[REDACTED]")
            .field("session_id", &"[REDACTED]")
            .field("circuit_code", &"[REDACTED]")
            .field("seed_capability", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Grid {
    #[default]
    Agni,
    Aditi,
}

impl Grid {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Agni => "Main grid (Agni)",
            Self::Aditi => "Beta grid (Aditi)",
        }
    }

    pub const fn login_url(self) -> &'static str {
        match self {
            Self::Agni => "https://login.agni.lindenlab.com/cgi-bin/login.cgi",
            Self::Aditi => "https://login.aditi.lindenlab.com/cgi-bin/login.cgi",
        }
    }
}

#[derive(Clone)]
pub struct LoginCredentials {
    pub first_name: String,
    pub last_name: String,
    pub password: String,
    pub grid: Grid,
    pub start_location: StartLocation,
    pub mfa_hash: String,
}

impl fmt::Debug for LoginCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoginCredentials")
            .field("first_name", &"[REDACTED]")
            .field("last_name", &"[REDACTED]")
            .field("password", &"[REDACTED]")
            .field("grid", &self.grid)
            .field("start_location", &"[REDACTED]")
            .field("mfa_hash", &"[REDACTED]")
            .finish()
    }
}

pub(crate) enum LoginResponse {
    Authenticated { session: Box<AgentSession>, mfa_hash: Option<String>, data: ResponseData },
    MfaChallenge,
}

impl fmt::Debug for LoginResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Authenticated { session, mfa_hash, data } => formatter
                .debug_struct("Authenticated")
                .field("session", session)
                .field("mfa_hash", &mfa_hash.as_ref().map(|_| "[REDACTED]"))
                .field("data", data)
                .finish(),
            Self::MfaChallenge => formatter.write_str("MfaChallenge"),
        }
    }
}

#[tracing::instrument(name = "network.login", level = "info", skip_all, fields(grid = ?credentials.grid))]
pub(crate) async fn login_with_status_at(
    credentials: &LoginCredentials,
    mfa_token: &str,
    mfa_hash: &str,
    login_url: &str,
    status: NetworkStatusHandle,
) -> Result<LoginResponse, String> {
    status.set_lifecycle(NetworkLifecycle::LoggingIn);
    let login_url = match Url::parse(login_url) {
        Ok(url) => url,
        Err(error) => {
            let error = error.to_string();
            status.record_error(&error);
            status.set_lifecycle(NetworkLifecycle::Failed);
            return Err(error);
        }
    };
    let result = login_request(credentials, mfa_token, mfa_hash, &login_url, status.clone()).await;
    match &result {
        Ok(LoginResponse::Authenticated { .. }) => {
            status.set_lifecycle(NetworkLifecycle::Connecting);
        }
        Ok(LoginResponse::MfaChallenge) => tracing::info!("Login response requires MFA"),
        Err(error) => {
            tracing::warn!("Login response rejected");
            status.record_error(error);
            status.set_lifecycle(NetworkLifecycle::Failed);
        }
    }
    result
}

#[cfg(test)]
pub(crate) async fn login_at(
    credentials: &LoginCredentials,
    mfa_token: &str,
    mfa_hash: &str,
    login_url: &Url,
) -> Result<LoginResponse, String> {
    login_request(credentials, mfa_token, mfa_hash, login_url, NetworkStatusHandle::default()).await
}

#[tracing::instrument(name = "network.login.request", level = "debug", skip_all, fields(grid = ?credentials.grid))]
async fn login_request(
    credentials: &LoginCredentials,
    mfa_token: &str,
    mfa_hash: &str,
    login_url: &Url,
    status: NetworkStatusHandle,
) -> Result<LoginResponse, String> {
    validate_http_url(login_url).map_err(|_| "The login server URL is invalid.".to_string())?;
    let body = login::Request {
        first_name: &credentials.first_name,
        last_name: &credentials.last_name,
        password: &credentials.password,
        start: &credentials.start_location,
        channel: "Radia",
        version: "0.0.1.0",
        platform: platform_name(),
        mac: "00:00:00:00:00:00",
        id0: "00000000-0000-0000-0000-000000000000",
        viewer_digest: "00000000-0000-0000-0000-000000000000",
        agree_to_tos: "",
        read_critical: "",
        token: mfa_token,
        mfa_hash,
        options: &["inventory-skeleton"],
    }
    .to_xml()
    .map_err(|error| error.to_string())?;
    let deadline = time::Instant::now() + LOGIN_REQUEST_TIMEOUT;
    let operation_permit = time::timeout_at(
        deadline,
        Arc::clone(LOGIN_OPERATION_SLOTS.get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_LOGIN_OPERATIONS)))).acquire_owned(),
    )
    .await
    .map_err(|_| "Could not contact Second Life: the login operation exceeded its deadline while waiting for admission.".to_string())?
    .map_err(|_| "Could not prepare the login request: login operation admission is unavailable.".to_string())?;
    let client = Client::new(Some(LOGIN_REQUEST_TIMEOUT), true, Some(concat!("Radia/", env!("CARGO_PKG_VERSION"))))
        .map_err(|error| format!("Could not prepare the login request: {error}"))?
        .with_status(status);
    tracing::debug!("Login request started");
    let response =
        time::timeout_at(deadline, client.post("login", login_url, "text/xml", None, body.into_bytes(), MAX_LOGIN_RESPONSE_BYTES))
            .await
            .map_err(|_| "Could not contact Second Life: the login operation exceeded its deadline.".to_string())?
            .map_err(|error| format!("Could not contact Second Life: {error}"))?;
    let status = response.status;
    let content_type = response.headers.get(CONTENT_TYPE).and_then(|value| value.to_str().ok()).unwrap_or_default().to_owned();
    let crate::http::Response { body, .. } = response;
    let parse = spawn_blocking_with_permit(operation_permit, move || {
        from_utf8(&body)
            .map_err(|_| "Second Life returned an XML-RPC login response that is not valid UTF-8.".to_string())
            .and_then(|body| parse_login_response(body, status, &content_type))
    });
    let (response, _operation_permit) = time::timeout_at(deadline, parse)
        .await
        .map_err(|_| "Second Life's login response exceeded the processing deadline.".to_string())?
        .map_err(|error| format!("Could not parse the Second Life login response: {error}"))?;
    let response = response?;

    match response {
        login::Response::MfaChallenge => Ok(LoginResponse::MfaChallenge),
        login::Response::Rejected { message, reason } => Err(match (message.is_empty(), reason.is_empty()) {
            (false, false) => format!("{message} ({reason})"),
            (false, true) => message,
            (true, false) => reason,
            (true, true) => "Second Life rejected the login.".into(),
        }),
        login::Response::Authenticated(value) => authenticated_response(*value, credentials),
    }
}

fn parse_login_response(body: &str, status: StatusCode, content_type: &str) -> Result<login::Response, String> {
    let body_start = body.trim_start();
    let is_html = has_ascii_prefix(body_start, b"<!doctype html")
        || has_ascii_prefix(body_start, b"<html")
        || content_type.split(';').next().is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("text/html"));
    if is_html && contains_ascii_case_insensitive(body.as_bytes(), b"errors.edgesuite.net") {
        return Err(format!("Second Life's login gateway returned an HTML error (HTTP {status}). Retry in a few minutes."));
    }
    if is_html {
        return Err(format!("Second Life returned an HTML page instead of an XML-RPC login response (HTTP {status})."));
    }
    match login::Response::from_xml(body, MAX_LOGIN_XML_DEPTH) {
        Err(login::Error::Fault(fault)) => return Err(fault.to_string()),
        Err(login::Error::DepthExceeded) => {
            return Err(format!("Second Life returned an XML-RPC login response with excessive nesting (HTTP {status})."));
        }
        Err(login::Error::ElementLimitExceeded) => {
            return Err(format!("Second Life returned an XML-RPC login response with too many elements (HTTP {status})."));
        }
        Ok(response) if status.is_success() => return Ok(response),
        Err(error) if status.is_success() && !matches!(error, login::Error::InvalidXml) => return Err(error.to_string()),
        _ => {}
    }
    Err(format!(
        "Second Life returned an invalid XML-RPC login response (HTTP {status}, {}).",
        if content_type.is_empty() { "no content type" } else { content_type }
    ))
}

fn has_ascii_prefix(value: &str, prefix: &[u8]) -> bool {
    value.as_bytes().get(..prefix.len()).is_some_and(|start| start.eq_ignore_ascii_case(prefix))
}

fn contains_ascii_case_insensitive(value: &[u8], needle: &[u8]) -> bool {
    value.windows(needle.len()).any(|window| window.eq_ignore_ascii_case(needle))
}

fn platform_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "Win"
    } else if cfg!(target_os = "macos") {
        "Mac"
    } else {
        "Lin"
    }
}

fn authenticated_response(value: login::Authenticated, credentials: &LoginCredentials) -> Result<LoginResponse, String> {
    let ip = value.simulator_address.ip();
    if ip.is_unspecified() || ip.is_multicast() || ip == IpAddr::V4(Ipv4Addr::BROADCAST) {
        return Err("Second Life returned an invalid sim_ip address.".into());
    }
    if value.simulator_address.port() == 0 {
        return Err("Second Life returned an invalid simulator port.".into());
    }
    let seed_capability =
        Url::parse(&value.seed_capability).map_err(|_| "Second Life returned an invalid simulator capability URL.".to_string())?;
    validate_http_url(&seed_capability).map_err(|_| "Second Life returned an invalid simulator capability URL.".to_string())?;
    let session = AgentSession {
        first_name: value.first_name.unwrap_or_else(|| credentials.first_name.clone()),
        last_name: value.last_name.unwrap_or_else(|| credentials.last_name.clone()),
        grid: credentials.grid,
        message: value.message,
        simulator_address: value.simulator_address,
        agent_id: value.agent_id,
        session_id: value.session_id,
        circuit_code: value.circuit_code,
        seed_capability,
    };
    Ok(LoginResponse::Authenticated { session: Box::new(session), mfa_hash: value.mfa_hash, data: value.data })
}

#[cfg(test)]
mod tests {
    use crate::{AgentSession, Event, Grid, LoginCredentials, PacketOrder::Ordered, StartLocation, event::SimulatorId};

    use crate::login::{LoginResponse, login_at};
    use crate::test_support::http::{LocalServer, response};
    use math::Vec3;
    use protocol::{
        Audible, ChatSourceType, ChatType,
        lludp::Message,
        login,
        messages::{ChatFromSimulator, chat::chat_from_simulator::ChatData},
    };
    use std::net::SocketAddr;
    use url::Url;

    const AGENT_ID: &str = "00112233-4455-6677-8899-aabbccddeeff";
    const SESSION_ID: &str = "ffeeddcc-bbaa-9988-7766-554433221100";
    const FOLDER_ID: &str = "12345678-1234-5678-1234-567812345678";

    #[tokio::test]
    async fn local_login_succeeds() {
        let body = xml_response(&[
            ("login", "<boolean>1</boolean>"),
            ("message", "<string>Welcome</string>"),
            ("first_name", "<string>Test</string>"),
            ("last_name", "<string>Resident</string>"),
            ("sim_ip", "<string>127.0.0.1</string>"),
            ("sim_port", "<int>13000</int>"),
            ("circuit_code", "<int>305419896</int>"),
            ("agent_id", &format!("<string>{AGENT_ID}</string>")),
            ("session_id", &format!("<string>{SESSION_ID}</string>")),
            ("seed_capability", "<string>http://127.0.0.1:9000/cap/seed</string>"),
            ("mfa_hash", "<string>remembered-mfa</string>"),
            (
                "inventory-skeleton",
                concat!(
                    "<array><data><value><struct>",
                    "<member><name>folder_id</name><value><string>",
                    "12345678-1234-5678-1234-567812345678",
                    "</string></value></member>",
                    "<member><name>parent_id</name><value><string>",
                    "00000000-0000-0000-0000-000000000000",
                    "</string></value></member>",
                    "<member><name>name</name><value><string>Favorites</string></value></member>",
                    "<member><name>type_default</name><value><int>23</int></value></member>",
                    "</struct></value></data></array>",
                ),
            ),
        ]);
        let server = LocalServer::start(vec![response(200, "OK", "text/xml", None, &body)]);
        let url = server.url("/login.cgi");
        let credentials = credentials();

        let result = login_at(&credentials, "123456", "saved-mfa", &url).await;
        let request = String::from_utf8(server.finish().remove(0)).expect("request is UTF-8");
        let (headers, body) = request.split_once("\r\n\r\n").expect("request has a body");

        assert!(headers.starts_with("POST /login.cgi HTTP/1.1"));
        assert!(headers.to_ascii_lowercase().contains("content-type: text/xml"));
        assert!(body.contains("login_to_simulator"));
        assert!(body.contains("<name>first</name>"));
        assert!(body.contains("<name>last</name>"));
        assert!(body.contains("<name>passwd</name>"));
        assert!(body.contains("<name>token</name>"));
        assert!(body.contains("123456"));
        assert!(body.contains("saved-mfa"));
        assert!(!body.contains(&credentials.password));

        match result.expect("local login response should be accepted") {
            LoginResponse::Authenticated { session, mfa_hash, data } => {
                assert_eq!(session.first_name, "Test");
                assert_eq!(session.last_name, "Resident");
                assert_eq!(session.agent_id.to_string(), AGENT_ID);
                assert_eq!(session.session_id.to_string(), SESSION_ID);
                assert_eq!(session.simulator_address.port(), 13000);
                assert_eq!(mfa_hash.as_deref(), Some("remembered-mfa"));
                let login::Value::Array(inventory_skeleton) =
                    data.get("inventory-skeleton").expect("login metadata retains unparsed fields")
                else {
                    panic!("expected the raw inventory skeleton");
                };
                assert_eq!(inventory_skeleton.len(), 1);
                let login::Value::Struct(folder) = &inventory_skeleton[0] else {
                    panic!("expected a raw XML-RPC folder map");
                };
                assert_eq!(folder.get("folder_id"), Some(&login::Value::String(FOLDER_ID.into())));
                assert_eq!(folder.get("name"), Some(&login::Value::String("Favorites".into())));
                assert_eq!(folder.get("type_default"), Some(&login::Value::Integer(23)));
            }
            LoginResponse::MfaChallenge => panic!("expected an authenticated session"),
        }
    }

    #[tokio::test]
    async fn login_reports_rejection() {
        let body = xml_response(&[
            ("login", "<boolean>0</boolean>"),
            ("message", "<string>Credentials were rejected</string>"),
            ("reason", "<string>login</string>"),
        ]);
        let server = LocalServer::start(vec![response(200, "OK", "text/xml", None, &body)]);
        let url = server.url("/login.cgi");

        let error = login_at(&credentials(), "", "", &url).await.expect_err("local grid should reject this login");
        server.finish();

        assert_eq!(error, "Credentials were rejected (login)");
    }

    #[tokio::test]
    async fn xmlrpc_fault_and_http_status() {
        let fault = "<methodResponse><fault><value><struct><member><name>faultCode</name><value><int>7</int></value></member><member><name>faultString</name><value><string>Denied</string></value></member></struct></value></fault></methodResponse>";
        let server = LocalServer::start(vec![response(500, "Error", "text/xml", None, fault)]);
        let error = login_at(&credentials(), "", "", &server.url("/login.cgi")).await.expect_err("XML-RPC fault on HTTP error");
        assert_eq!(error, "Server Fault 7: Denied");
        assert_eq!(server.finish().len(), 1);

        let body = successful_response("<string>127.0.0.1</string>");
        let server = LocalServer::start(vec![response(500, "Error", "text/xml", None, &body)]);
        let error = login_at(&credentials(), "", "", &server.url("/login.cgi")).await.expect_err("HTTP failure cannot authenticate");
        assert!(error.contains("invalid XML-RPC login response (HTTP 500"));
        assert_eq!(server.finish().len(), 1);
    }

    #[tokio::test]
    async fn invalid_utf8_response() {
        let mut body = xml_response(&[("login", "<boolean>1</boolean>"), ("message", "<string>Welcome</string>")]).into_bytes();
        let message_start = body.windows(b"Welcome".len()).position(|window| window == b"Welcome").expect("message exists");
        body[message_start] = 0xff;
        let mut response =
            format!("HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
                .into_bytes();
        response.extend_from_slice(&body);
        let server = LocalServer::start(vec![response]);

        let error = login_at(&credentials(), "", "", &server.url("/login.cgi")).await.expect_err("invalid UTF-8 must be rejected");
        server.finish();

        assert_eq!(error, "Second Life returned an XML-RPC login response that is not valid UTF-8.");
    }

    #[tokio::test]
    async fn login_depth_limit() {
        let open = "<array><data><value>";
        let close = "</value></data></array>";
        let nested = format!("{}<string>deep</string>{}", open.repeat(24), close.repeat(24));
        let body = xml_response(&[("login", "<boolean>1</boolean>"), ("nested", &nested)]);
        let server = LocalServer::start(vec![response(200, "OK", "text/xml", None, &body)]);
        let error = login_at(&credentials(), "", "", &server.url("/login.cgi")).await.expect_err("deep XML must be rejected");
        server.finish();

        assert!(error.contains("excessive nesting"));
    }

    #[tokio::test]
    async fn crowded_login_memory() {
        let folders = (0..1024)
            .map(|_| {
                format!(
                    "<value><struct><member><name>folder_id</name><value><string>{FOLDER_ID}</string></value></member></struct></value>"
                )
            })
            .collect::<String>();
        let inventory = format!("<array><data>{folders}</data></array>");
        let body = xml_response(&[
            ("login", "<boolean>1</boolean>"),
            ("sim_ip", "<string>127.0.0.1</string>"),
            ("sim_port", "<int>13000</int>"),
            ("circuit_code", "<int>1</int>"),
            ("agent_id", &format!("<string>{AGENT_ID}</string>")),
            ("session_id", &format!("<string>{SESSION_ID}</string>")),
            ("seed_capability", "<string>http://127.0.0.1:9000/cap/seed</string>"),
            ("inventory-skeleton", &inventory),
        ]);

        let (parsed, retained_bytes, peak_bytes, elapsed_ns): (Result<login::Response, String>, i128, i128, u128) =
            crate::capabilities::measure_allocations(|| super::parse_login_response(&body, reqwest::StatusCode::OK, "text/xml"));
        let parsed = parsed.expect("crowded login response should decode");
        let login::Response::Authenticated(parsed) = parsed else { panic!("expected authenticated response") };
        let login::Value::Array(parsed_inventory) = parsed.data.get("inventory-skeleton").expect("inventory should be retained") else {
            panic!("expected inventory array");
        };
        assert_eq!(parsed_inventory.len(), 1024);
        assert!(retained_bytes > 0, "decoded inventory should retain allocations");
        assert!(peak_bytes >= retained_bytes, "parse peak must include retained output allocations");
        eprintln!("crowded login parse: input={} B, retained={retained_bytes} B, peak={peak_bytes} B, parse={elapsed_ns} ns", body.len());
        drop(parsed);

        let server = LocalServer::start(vec![response(200, "OK", "text/xml", None, &body)]);
        let result = login_at(&credentials(), "", "", &server.url("/login.cgi")).await;
        server.finish();

        let LoginResponse::Authenticated { data, .. } = result.expect("large shallow inventory should be accepted") else {
            panic!("expected authenticated session");
        };
        let login::Value::Array(inventory) = data.get("inventory-skeleton").expect("inventory metadata should be retained") else {
            panic!("expected inventory array");
        };
        assert_eq!(inventory.len(), 1024);
    }

    #[tokio::test]
    async fn invalid_simulator_ips() {
        for ip in ["0.0.0.0", "224.0.0.1", "255.255.255.255"] {
            let body = successful_response(&format!("<string>{ip}</string>"));
            let server = LocalServer::start(vec![response(200, "OK", "text/xml", None, &body)]);
            let url = server.url("/login.cgi");

            let error = login_at(&credentials(), "", "", &url).await.expect_err("simulator IP must be unicast");
            server.finish();

            assert_eq!(error, "Second Life returned an invalid sim_ip address.");
        }
    }

    #[tokio::test]
    async fn zero_sim_port_is_rejected() {
        let body = xml_response(&[
            ("login", "<boolean>1</boolean>"),
            ("sim_ip", "<string>127.0.0.1</string>"),
            ("sim_port", "<int>0</int>"),
            ("circuit_code", "<int>1</int>"),
            ("agent_id", &format!("<string>{AGENT_ID}</string>")),
            ("session_id", &format!("<string>{SESSION_ID}</string>")),
            ("seed_capability", "<string>http://127.0.0.1:9000/cap/seed</string>"),
        ]);
        let server = LocalServer::start(vec![response(200, "OK", "text/xml", None, &body)]);
        let url = server.url("/login.cgi");

        let error = login_at(&credentials(), "", "", &url).await.expect_err("port zero cannot identify a simulator endpoint");
        server.finish();

        assert_eq!(error, "Second Life returned an invalid simulator port.");
    }

    #[tokio::test]
    async fn simulator_ip_range() {
        let body = successful_response("<i8>4294967296</i8>");
        let server = LocalServer::start(vec![response(200, "OK", "text/xml", None, &body)]);
        let url = server.url("/login.cgi");

        let error = login_at(&credentials(), "", "", &url).await.expect_err("out-of-range simulator IP must be rejected");
        server.finish();

        assert_eq!(error, "Second Life returned an invalid sim_ip address.");
    }

    #[tokio::test]
    async fn simulator_ip_long_bounds() {
        for (wire_value, expected_ip) in [("<i8>-2147483648</i8>", "128.0.0.0"), ("<i8>4294967294</i8>", "255.255.255.254")] {
            let body = successful_response(wire_value);
            let server = LocalServer::start(vec![response(200, "OK", "text/xml", None, &body)]);
            let url = server.url("/login.cgi");

            let LoginResponse::Authenticated { session, .. } =
                login_at(&credentials(), "", "", &url).await.expect("in-range IP should be accepted")
            else {
                panic!("expected authenticated session");
            };
            server.finish();
            assert_eq!(session.simulator_address.ip().to_string(), expected_ip);
        }
    }

    #[tokio::test]
    async fn gateway_503_is_not_replayed() {
        let body = "<!doctype html><html><body>errors.edgesuite.net</body></html>";
        let server = LocalServer::start(vec![response(503, "Service Unavailable", "text/html", Some("0"), body)]);
        let url = server.url("/login.cgi");

        let error = login_at(&credentials(), "mfa-token", "mfa-hash", &url).await.expect_err("local gateway should remain unavailable");
        let requests = server.finish();

        assert_eq!(requests.len(), 1, "an ambiguous gateway response must not replay login or MFA credentials");
        assert!(error.contains("HTTP 503"));
        assert!(!error.contains("after 3 attempts"));
        assert!(!error.contains("<html>"));
    }

    #[test]
    fn gateway_html_detection_is_ascii_case_insensitive() {
        let error = super::parse_login_response(
            " \t<!DOCTYPE HTML><HTML>Errors.EdgeSuite.Net</HTML>",
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            "TEXT/HTML; charset=utf-8",
        )
        .expect_err("gateway HTML should be identified without normalizing the whole body");

        assert!(error.contains("login gateway returned an HTML error (HTTP 503"));
    }

    #[test]
    fn login_xml_element_limit_is_reported() {
        let body = "<value/>".repeat(262_145);
        let error = super::parse_login_response(&body, reqwest::StatusCode::OK, "text/xml")
            .expect_err("excessive element counts are rejected before XML-RPC deserialization");

        assert!(error.contains("too many elements"), "unexpected parser error: {error}");
    }

    #[test]
    fn debug_redacts_private_data() {
        let mut credentials = credentials();
        credentials.first_name = "private-first-name".into();
        credentials.last_name = "private-last-name".into();
        credentials.password = "secret-password".into();
        credentials.mfa_hash = "secret-mfa-hash".into();
        credentials.start_location = StartLocation::Region { name: "Secret Region".into(), x: 12, y: 34, z: 56 };
        let credentials_debug = format!("{credentials:?}");
        for private_value in ["private-first-name", "private-last-name", "secret-password", "secret-mfa-hash", "Secret Region"] {
            assert!(!credentials_debug.contains(private_value));
        }
        assert!(credentials_debug.contains("[REDACTED]"));

        let session = AgentSession {
            first_name: "private-first-name".into(),
            last_name: "private-last-name".into(),
            grid: Grid::Agni,
            message: "private-login-message".into(),
            simulator_address: "192.0.2.123:13000".parse::<SocketAddr>().expect("valid simulator address"),
            agent_id: AGENT_ID.parse().expect("valid test agent ID"),
            session_id: SESSION_ID.parse().expect("valid test session ID"),
            circuit_code: 987654321,
            seed_capability: Url::parse("https://caps.example.net/cap?token=secret-capability").expect("valid capability URL"),
        };
        let session_debug = format!("{session:?}");
        for private_value in [
            "private-first-name",
            "private-last-name",
            "private-login-message",
            "192.0.2.123",
            AGENT_ID,
            SESSION_ID,
            "987654321",
            "secret-capability",
        ] {
            assert!(!session_debug.contains(private_value));
        }
        assert!(session_debug.contains("[REDACTED]"));

        let event = Event::LoginAccepted { mfa_hash: Some("secret-mfa-hash".into()) };
        let event_debug = format!("{event:?}");
        assert!(!event_debug.contains("secret-mfa-hash"));
        assert!(event_debug.contains("[REDACTED]"));

        let event = Event::SimulatorMessage {
            simulator_id: SimulatorId::new(1),
            source: "192.0.2.123:13000".parse().expect("valid simulator address"),
            sequence: 7,
            packet_order: Ordered(7),
            message: Message::ChatFromSimulator(ChatFromSimulator {
                chat_data: ChatData {
                    from_name: "private-avatar-name".into(),
                    source_id: AGENT_ID.parse().expect("valid source ID"),
                    owner_id: SESSION_ID.parse().expect("valid owner ID"),
                    source_type: ChatSourceType::Agent,
                    chat_type: ChatType::Normal,
                    audible: Audible::Fully,
                    position: Vec3::default(),
                    message: "private-chat-text".into(),
                },
            }),
        };
        let event_debug = format!("{event:?}");
        for private_value in ["192.0.2.123", "private-avatar-name", AGENT_ID, SESSION_ID, "private-chat-text"] {
            assert!(!event_debug.contains(private_value));
        }
        assert!(event_debug.contains("[REDACTED]"));
        assert!(event_debug.contains("[OMITTED]"));
    }

    fn credentials() -> LoginCredentials {
        LoginCredentials {
            first_name: "Test".into(),
            last_name: "Resident".into(),
            password: "fixture-password".into(),
            grid: Grid::Agni,
            start_location: StartLocation::Last,
            mfa_hash: "".into(),
        }
    }

    fn xml_response(fields: &[(&str, &str)]) -> String {
        let members =
            fields.iter().map(|(name, value)| format!("<member><name>{name}</name><value>{value}</value></member>")).collect::<String>();
        format!(
            "<?xml version=\"1.0\"?><methodResponse><params><param><value><struct>{members}</struct></value></param></params></methodResponse>"
        )
    }

    fn successful_response(sim_ip: &str) -> String {
        xml_response(&[
            ("login", "<boolean>1</boolean>"),
            ("sim_ip", sim_ip),
            ("sim_port", "<int>13000</int>"),
            ("circuit_code", "<int>1</int>"),
            ("agent_id", &format!("<string>{AGENT_ID}</string>")),
            ("session_id", &format!("<string>{SESSION_ID}</string>")),
            ("seed_capability", "<string>http://127.0.0.1:9000/cap/seed</string>"),
        ])
    }
}
