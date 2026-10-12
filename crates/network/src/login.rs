use std::{
    collections::BTreeMap,
    fmt,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    str::from_utf8,
    time::Duration,
};

use base::UUID;
use dxr::{FaultResponse, MethodCall, MethodResponse, Value};
use md5::{Digest, Md5};
use quick_xml::{Reader, events::Event};
use reqwest::{
    StatusCode,
    header::{CONTENT_TYPE, RETRY_AFTER},
};
use tokio::time::sleep;
use url::Url;

use crate::{
    capture::CaptureRecorder,
    http::{Client, validate_http_url},
    status::{NetworkLifecycle, NetworkStatusHandle},
};

/// LLXML-RPC value returned in a login response.
pub use dxr::Value as LoginValue;

/// Account and simulator data returned after a successful login.
#[derive(Clone)]
pub struct AgentSession {
    pub first_name: String,
    pub last_name: String,
    pub grid: Grid,
    pub message: String,
    pub simulator_address: SocketAddr,
    pub agent_id: UUID,
    pub session_id: UUID,
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

#[derive(Clone, Default)]
/// Additional fields returned by the grid's login response.
pub struct LoginResponseData {
    fields: BTreeMap<String, LoginValue>,
}

impl LoginResponseData {
    pub fn from_fields(fields: BTreeMap<String, LoginValue>) -> Self {
        Self { fields }
    }

    pub fn get(&self, name: &str) -> Option<&LoginValue> {
        self.fields.get(name)
    }
}

impl fmt::Debug for LoginResponseData {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("LoginResponseData").field("field_names", &self.fields.keys().collect::<Vec<_>>()).finish()
    }
}

const LOGIN_RETRY_ATTEMPTS: u8 = 3;
const LOGIN_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_LOGIN_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_LOGIN_XML_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// Second Life grid selected for login.
pub enum Grid {
    #[default]
    /// Main grid.
    Agni,
    /// Beta grid.
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

/// Credentials and login options consumed by [`crate::Session::start`].
#[derive(Clone)]
pub struct LoginCredentials {
    pub first_name: String,
    pub last_name: String,
    pub password: String,
    pub grid: Grid,
    pub start_location: StartLocation,
    /// MFA hash from a previous login, or an empty string if absent.
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

#[derive(Clone, Debug, Eq, PartialEq)]
/// Initial region and position requested during login.
pub enum StartLocation {
    Last,
    Home,
    /// Start in a named region at the supplied meter coordinates.
    Region {
        name: String,
        x: u32,
        y: u32,
        z: u32,
    },
}

impl StartLocation {
    /// Encodes this location in the login protocol's start-location parameter.
    pub fn login_parameter(&self) -> String {
        match self {
            Self::Last => "last".into(),
            Self::Home => "home".into(),
            Self::Region { name, x, y, z } => format!("uri:{name}&{x}&{y}&{z}"),
        }
    }
}

pub(crate) enum LoginResponse {
    Authenticated { session: Box<AgentSession>, mfa_hash: Option<String>, data: LoginResponseData },
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
    capture: Option<CaptureRecorder>,
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
    let result = login_at_with_capture(credentials, mfa_token, mfa_hash, &login_url, capture, status.clone()).await;
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
    login_at_with_capture(credentials, mfa_token, mfa_hash, login_url, None, NetworkStatusHandle::default()).await
}

#[tracing::instrument(name = "network.login.request", level = "debug", skip_all, fields(grid = ?credentials.grid))]
async fn login_at_with_capture(
    credentials: &LoginCredentials,
    mfa_token: &str,
    mfa_hash: &str,
    login_url: &Url,
    capture: Option<CaptureRecorder>,
    status: NetworkStatusHandle,
) -> Result<LoginResponse, String> {
    validate_http_url(login_url).map_err(|_| "The login server URL is invalid.".to_string())?;
    let call = MethodCall { name: "login_to_simulator".into(), params: vec![login_parameters(credentials, mfa_token, mfa_hash)] };
    let body =
        format!("<?xml version=\"1.0\"?>\n{}\n", call.to_xml().map_err(|_| "Could not encode the Second Life login request.".to_string())?);
    let client = Client::new(Some(LOGIN_REQUEST_TIMEOUT), true, Some(concat!("Radia/", env!("CARGO_PKG_VERSION"))))
        .map_err(|error| format!("Could not prepare the login request: {error}"))?
        .with_capture(capture.clone())
        .with_status(status);
    let mut attempt = 1;
    let response = loop {
        tracing::debug!(attempt, "Login request started");
        let response = client
            .post_operation_bounded("login", None, login_url, "text/xml", None, body.clone().into_bytes(), MAX_LOGIN_RESPONSE_BYTES)
            .await
            .map_err(|error| format!("Could not contact Second Life: {error}"))?;
        let status = response.status;

        if is_retryable_gateway_status(status) {
            tracing::warn!(attempt, http_status = status.as_u16(), "Login gateway retry");
            if attempt == LOGIN_RETRY_ATTEMPTS {
                let host = response.url.host_str().unwrap_or("the login server");
                return Err(format!(
                    "Second Life's login gateway at {host} remained unavailable (HTTP {status}) after {LOGIN_RETRY_ATTEMPTS} attempts. Try again shortly."
                ));
            }
            if let Some(capture) = &capture {
                capture.lifecycle("login_retry");
            }
            let retry_after =
                response.headers.get(RETRY_AFTER).and_then(|value| value.to_str().ok()).and_then(|value| value.parse::<u64>().ok());
            sleep(login_retry_delay(retry_after, attempt)).await;
            attempt += 1;
            continue;
        }

        let content_type = response.headers.get(CONTENT_TYPE).and_then(|value| value.to_str().ok()).unwrap_or_default().to_owned();
        let body =
            from_utf8(&response.body).map_err(|_| "Second Life returned an XML-RPC login response that is not valid UTF-8.".to_string())?;
        break parse_login_response(body, status, &content_type)?;
    };

    let fields = match response {
        Value::Struct(fields) => fields,
        _ => return Err("Second Life returned an invalid login response.".into()),
    };

    if string_field(&fields, "reason").as_deref() == Some("mfa_challenge") {
        return Ok(LoginResponse::MfaChallenge);
    }

    if !login_succeeded(&fields) {
        let message = string_field(&fields, "message").unwrap_or_default();
        let reason = string_field(&fields, "reason").unwrap_or_default();
        return Err(match (message.is_empty(), reason.is_empty()) {
            (false, false) => format!("{message} ({reason})"),
            (false, true) => message,
            (true, false) => reason,
            (true, true) => "Second Life rejected the login.".into(),
        });
    }

    let session = parse_agent_session(&fields, credentials)?;
    let mfa_hash = string_field(&fields, "mfa_hash");
    Ok(LoginResponse::Authenticated { session: Box::new(session), mfa_hash, data: LoginResponseData::from_fields(fields) })
}

fn parse_login_response(body: &str, status: StatusCode, content_type: &str) -> Result<Value, String> {
    let normalized_body = body.trim_start().to_ascii_lowercase();
    let is_html = normalized_body.starts_with("<!doctype html")
        || normalized_body.starts_with("<html")
        || content_type.to_ascii_lowercase().contains("text/html");
    if is_html && body.to_ascii_lowercase().contains("errors.edgesuite.net") {
        return Err(format!("Second Life's login gateway returned an HTML error (HTTP {status}). Retry in a few minutes."));
    }
    if is_html {
        return Err(format!("Second Life returned an HTML page instead of an XML-RPC login response (HTTP {status})."));
    }
    if login_xml_exceeds_depth(body) {
        return Err(format!("Second Life returned an XML-RPC login response with excessive nesting (HTTP {status})."));
    }

    if let Ok(response) = FaultResponse::from_xml(body) {
        return Err(response.fault.to_string());
    }

    if status.is_success()
        && let Ok(response) = MethodResponse::from_xml(body)
    {
        return Ok(response.value);
    }

    Err(format!(
        "Second Life returned an invalid XML-RPC login response (HTTP {status}, {}).",
        if content_type.is_empty() { "no content type" } else { content_type }
    ))
}

fn login_xml_exceeds_depth(body: &str) -> bool {
    let mut reader = Reader::from_str(body);
    let mut depth = 0;
    loop {
        match reader.read_event() {
            Ok(Event::Start(_)) => {
                depth += 1;
                if depth > MAX_LOGIN_XML_DEPTH {
                    return true;
                }
            }
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Eof) | Err(_) => return false,
            _ => {}
        }
    }
}

fn is_retryable_gateway_status(status: StatusCode) -> bool {
    matches!(status.as_u16(), 502..=504)
}

fn login_retry_delay(retry_after: Option<u64>, attempt: u8) -> Duration {
    Duration::from_secs(retry_after.unwrap_or(u64::from(attempt) * 2).min(30))
}

fn login_parameters(credentials: &LoginCredentials, mfa_token: &str, mfa_hash: &str) -> Value {
    let mut fields = BTreeMap::new();
    insert_string(&mut fields, "first", credentials.first_name.clone());
    insert_string(&mut fields, "last", credentials.last_name.clone());
    insert_string(&mut fields, "passwd", password_hash(&credentials.password));
    insert_string(&mut fields, "start", credentials.start_location.login_parameter());
    insert_string(&mut fields, "channel", "Radia".into());
    insert_string(&mut fields, "version", "0.0.1.0".into());
    insert_string(&mut fields, "platform", platform_name().into());
    insert_string(&mut fields, "mac", "00:00:00:00:00:00".into());
    insert_string(&mut fields, "id0", "00000000-0000-0000-0000-000000000000".into());
    insert_string(&mut fields, "viewer_digest", "00000000-0000-0000-0000-000000000000".into());
    insert_string(&mut fields, "agree_to_tos", String::new());
    insert_string(&mut fields, "read_critical", String::new());
    insert_string(&mut fields, "token", mfa_token.into());
    insert_string(&mut fields, "mfa_hash", mfa_hash.into());
    fields.insert("options".into(), Value::Array(vec![Value::String("inventory-skeleton".into())]));
    Value::Struct(fields)
}

fn insert_string(fields: &mut BTreeMap<String, Value>, key: &str, value: String) {
    fields.insert(key.into(), Value::String(value));
}

fn password_hash(password: &str) -> String {
    let digest = Md5::digest(password.as_bytes());
    format!("$1${digest:x}")
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

fn login_succeeded(fields: &BTreeMap<String, Value>) -> bool {
    match fields.get("login") {
        Some(Value::Boolean(value)) => *value,
        Some(Value::String(value)) => value.eq_ignore_ascii_case("true"),
        Some(Value::Integer(value)) => *value != 0,
        _ => false,
    }
}

fn parse_agent_session(fields: &BTreeMap<String, Value>, credentials: &LoginCredentials) -> Result<AgentSession, String> {
    let first_name = string_field(fields, "first_name").unwrap_or_else(|| credentials.first_name.clone());
    let last_name = string_field(fields, "last_name").unwrap_or_else(|| credentials.last_name.clone());
    let message = string_field(fields, "message").unwrap_or_default();
    let ip = ip_field(fields, "sim_ip")?;
    if ip.is_unspecified() || ip.is_multicast() || ip == IpAddr::V4(Ipv4Addr::BROADCAST) {
        return Err("Second Life returned an invalid sim_ip address.".into());
    }
    let port: u16 =
        integer_field(fields, "sim_port")?.try_into().map_err(|_| "Second Life returned an invalid simulator port.".to_string())?;
    if port == 0 {
        return Err("Second Life returned an invalid simulator port.".into());
    }
    let circuit_code =
        integer_field(fields, "circuit_code")?.try_into().map_err(|_| "Second Life returned an invalid circuit code.".to_string())?;
    let agent_id = uuid_field(fields, "agent_id")?;
    let session_id = uuid_field(fields, "session_id")?;
    let seed_capability = Url::parse(
        &string_field(fields, "seed_capability").ok_or_else(|| "Second Life omitted the simulator capability URL.".to_string())?,
    )
    .map_err(|_| "Second Life returned an invalid simulator capability URL.".to_string())?;
    validate_http_url(&seed_capability).map_err(|_| "Second Life returned an invalid simulator capability URL.".to_string())?;

    Ok(AgentSession {
        first_name,
        last_name,
        grid: credentials.grid,
        message,
        simulator_address: SocketAddr::new(ip, port),
        agent_id,
        session_id,
        circuit_code,
        seed_capability,
    })
}

fn string_field(fields: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    match fields.get(key) {
        Some(Value::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn ip_field(fields: &BTreeMap<String, Value>, key: &str) -> Result<IpAddr, String> {
    let invalid = || format!("Second Life returned an invalid {key} address.");
    match fields.get(key) {
        Some(Value::String(value)) => value.parse().map_err(|_| invalid()),
        Some(Value::Integer(value)) => Ok(IpAddr::V4(Ipv4Addr::from((*value as u32).to_be_bytes()))),
        Some(Value::Long(value)) if (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(value) => {
            Ok(IpAddr::V4(Ipv4Addr::from((*value as u32).to_be_bytes())))
        }
        Some(Value::Long(_)) => Err(invalid()),
        _ => Err(invalid()),
    }
}

fn integer_field(fields: &BTreeMap<String, Value>, key: &str) -> Result<i64, String> {
    match fields.get(key) {
        Some(Value::Integer(value)) => Ok(i64::from(*value)),
        Some(Value::Long(value)) => Ok(*value),
        Some(Value::String(value)) => value.parse().map_err(|_| format!("Second Life returned an invalid {key}.")),
        _ => Err(format!("Second Life omitted {key}.")),
    }
}

fn uuid_field(fields: &BTreeMap<String, Value>, key: &str) -> Result<UUID, String> {
    let value = string_field(fields, key).ok_or_else(|| format!("Second Life omitted {key}."))?;
    UUID::parse_str(&value).map_err(|_| format!("Second Life returned an invalid {key}."))
}

#[cfg(test)]
mod tests {
    use crate::{AgentSession, Event, Grid, LoginCredentials, LoginValue, PacketOrder::Ordered, StartLocation, event::SimulatorId};

    use crate::http::test_support::{LocalServer, response};
    use crate::login::{LoginResponse, login_at};
    use math::Vec3;
    use protocol::{
        Audible, ChatSourceType, ChatType,
        lludp::Message,
        messages::{ChatFromSimulator, chat_from_simulator::ChatData},
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
                let LoginValue::Array(inventory_skeleton) = data.get("inventory-skeleton").expect("login metadata retains unparsed fields")
                else {
                    panic!("expected the raw inventory skeleton");
                };
                assert_eq!(inventory_skeleton.len(), 1);
                let LoginValue::Struct(folder) = &inventory_skeleton[0] else {
                    panic!("expected a raw XML-RPC folder map");
                };
                assert_eq!(folder.get("folder_id"), Some(&LoginValue::String(FOLDER_ID.into())));
                assert_eq!(folder.get("name"), Some(&LoginValue::String("Favorites".into())));
                assert_eq!(folder.get("type_default"), Some(&LoginValue::Integer(23)));
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
    async fn flat_inventory() {
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
        let server = LocalServer::start(vec![response(200, "OK", "text/xml", None, &body)]);
        let result = login_at(&credentials(), "", "", &server.url("/login.cgi")).await;
        server.finish();

        let LoginResponse::Authenticated { data, .. } = result.expect("large shallow inventory should be accepted") else {
            panic!("expected authenticated session");
        };
        let LoginValue::Array(inventory) = data.get("inventory-skeleton").expect("inventory metadata should be retained") else {
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
    async fn gateway_503_limit() {
        let body = "<!doctype html><html><body>errors.edgesuite.net</body></html>";
        let responses = (0..3).map(|_| response(503, "Service Unavailable", "text/html", Some("0"), body)).collect();
        let server = LocalServer::start(responses);
        let url = server.url("/login.cgi");

        let error = login_at(&credentials(), "", "", &url).await.expect_err("local gateway should remain unavailable");
        let requests = server.finish();

        assert_eq!(requests.len(), 3);
        assert!(error.contains("HTTP 503"));
        assert!(error.contains("after 3 attempts"));
        assert!(!error.contains("<html>"));
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
