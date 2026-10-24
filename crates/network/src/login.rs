use std::{
    error::Error as StdError,
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
};

pub use protocol::login::StartLocation;

const LOGIN_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_LOGIN_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_LOGIN_XML_DEPTH: usize = 64;
const MAX_CONCURRENT_LOGIN_OPERATIONS: usize = 2;
const LOGIN_ADMISSION_TIMEOUT_MESSAGE: &str =
    "Could not contact Second Life: the login operation exceeded its deadline while waiting for admission.";
const LOGIN_ADMISSION_UNAVAILABLE_MESSAGE: &str = "Could not prepare the login request: login operation admission is unavailable.";

static LOGIN_OPERATION_SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();

pub(crate) enum LoginError {
    Request(LoginFailure),
    Unavailable(LoginFailure),
    InvalidResponse { status: StatusCode, reason: InvalidResponseReason, content_type: Option<String> },
    Rejected { message: String, reason: String },
    InvalidAuthenticatedData(AuthenticatedDataError),
}

#[derive(Debug)]
pub(crate) enum InvalidResponseReason {
    GatewayHtml,
    HtmlInsteadOfXmlRpc,
    InvalidUtf8(std::str::Utf8Error),
    Protocol(login::Error),
    ExcessiveNesting,
    TooManyElements,
    MalformedXmlRpc(login::Error),
    NonSuccessHttpStatus,
}

pub(crate) struct LoginFailure {
    message: String,
    source: Box<dyn StdError + Send + Sync + 'static>,
}

impl LoginError {
    fn request(message: impl Into<String>, source: impl StdError + Send + Sync + 'static) -> Self {
        Self::Request(LoginFailure { message: message.into(), source: Box::new(source) })
    }

    fn unavailable(message: impl Into<String>, source: impl StdError + Send + Sync + 'static) -> Self {
        Self::Unavailable(LoginFailure { message: message.into(), source: Box::new(source) })
    }

    fn invalid_response(status: StatusCode, reason: InvalidResponseReason, content_type: Option<&str>) -> Self {
        Self::InvalidResponse { status, reason, content_type: content_type.map(str::to_owned) }
    }
}

impl fmt::Debug for LoginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Request(_) => "Request",
            Self::Unavailable(_) => "Unavailable",
            Self::InvalidResponse { .. } => "InvalidResponse",
            Self::Rejected { .. } => "Rejected { [REDACTED] }",
            Self::InvalidAuthenticatedData(_) => "InvalidAuthenticatedData",
        })
    }
}

impl fmt::Display for LoginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(error) | Self::Unavailable(error) => formatter.write_str(&error.message),
            Self::InvalidResponse { status, reason, content_type } => match reason {
                InvalidResponseReason::GatewayHtml => {
                    write!(formatter, "Second Life's login gateway returned an HTML error (HTTP {status}). Retry in a few minutes.")
                }
                InvalidResponseReason::HtmlInsteadOfXmlRpc => {
                    write!(formatter, "Second Life returned an HTML page instead of an XML-RPC login response (HTTP {status}).")
                }
                InvalidResponseReason::InvalidUtf8(_) => {
                    formatter.write_str("Second Life returned an XML-RPC login response that is not valid UTF-8.")
                }
                InvalidResponseReason::Protocol(error) => write!(formatter, "{error}"),
                InvalidResponseReason::ExcessiveNesting => {
                    write!(formatter, "Second Life returned an XML-RPC login response with excessive nesting (HTTP {status}).")
                }
                InvalidResponseReason::TooManyElements => {
                    write!(formatter, "Second Life returned an XML-RPC login response with too many elements (HTTP {status}).")
                }
                InvalidResponseReason::MalformedXmlRpc(_) | InvalidResponseReason::NonSuccessHttpStatus => {
                    let content_type = content_type.as_deref().unwrap_or_default();
                    let content_type = if content_type.is_empty() { "no content type" } else { content_type };
                    write!(formatter, "Second Life returned an invalid XML-RPC login response (HTTP {status}, {content_type}).")
                }
            },
            Self::Rejected { message, reason } => match (message.is_empty(), reason.is_empty()) {
                (false, false) => write!(formatter, "{message} ({reason})"),
                (false, true) => formatter.write_str(message),
                (true, false) => formatter.write_str(reason),
                (true, true) => formatter.write_str("Second Life rejected the login."),
            },
            Self::InvalidAuthenticatedData(error) => match error {
                AuthenticatedDataError::SimulatorIp => formatter.write_str("Second Life returned an invalid sim_ip address."),
                AuthenticatedDataError::SimulatorPort => formatter.write_str("Second Life returned an invalid simulator port."),
                AuthenticatedDataError::CapabilityUrl => formatter.write_str("Second Life returned an invalid simulator capability URL."),
            },
        }
    }
}

impl StdError for LoginError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Request(error) | Self::Unavailable(error) => Some(&*error.source),
            Self::InvalidResponse { reason: InvalidResponseReason::InvalidUtf8(error), .. } => Some(error),
            Self::InvalidResponse {
                reason: InvalidResponseReason::Protocol(error) | InvalidResponseReason::MalformedXmlRpc(error),
                ..
            } => Some(error),
            Self::InvalidResponse { .. } | Self::Rejected { .. } | Self::InvalidAuthenticatedData(_) => None,
        }
    }
}

#[derive(Debug)]
pub(crate) enum AuthenticatedDataError {
    SimulatorIp,
    SimulatorPort,
    CapabilityUrl,
}

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
pub(crate) async fn login_at_url(
    credentials: &LoginCredentials,
    mfa_token: &str,
    mfa_hash: &str,
    login_url: &str,
) -> Result<LoginResponse, LoginError> {
    let login_url = Url::parse(login_url).map_err(|error| LoginError::request(error.to_string(), error))?;
    let result = login_request(credentials, mfa_token, mfa_hash, &login_url).await;
    if matches!(&result, Ok(LoginResponse::MfaChallenge)) {
        tracing::info!("Login response requires MFA");
    }
    result
}

#[tracing::instrument(name = "network.login.request", level = "debug", skip_all, fields(grid = ?credentials.grid))]
async fn login_request(
    credentials: &LoginCredentials,
    mfa_token: &str,
    mfa_hash: &str,
    login_url: &Url,
) -> Result<LoginResponse, LoginError> {
    validate_http_url(login_url).map_err(|error| LoginError::request("The login server URL is invalid.", error))?;
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
    .map_err(|error| LoginError::request(error.to_string(), error))?;
    let deadline = time::Instant::now() + LOGIN_REQUEST_TIMEOUT;
    let operation_permit = time::timeout_at(
        deadline,
        Arc::clone(LOGIN_OPERATION_SLOTS.get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_LOGIN_OPERATIONS)))).acquire_owned(),
    )
    .await
    .map_err(|error| LoginError::unavailable(LOGIN_ADMISSION_TIMEOUT_MESSAGE, error))?
    .map_err(|error| LoginError::unavailable(LOGIN_ADMISSION_UNAVAILABLE_MESSAGE, error))?;
    let client = Client::new(Some(LOGIN_REQUEST_TIMEOUT), true, Some(concat!("Radia/", env!("CARGO_PKG_VERSION"))))
        .map_err(|error| LoginError::request(format!("Could not prepare the login request: {error}"), error))?;
    tracing::debug!("Login request started");
    let response =
        time::timeout_at(deadline, client.post("login", login_url, "text/xml", None, body.into_bytes(), MAX_LOGIN_RESPONSE_BYTES))
            .await
            .map_err(|error| LoginError::request("Could not contact Second Life: the login operation exceeded its deadline.", error))?
            .map_err(|error| LoginError::request(format!("Could not contact Second Life: {error}"), error))?;
    let status = response.status;
    let content_type = response.headers.get(CONTENT_TYPE).and_then(|value| value.to_str().ok()).unwrap_or_default().to_owned();
    let crate::http::Response { body, .. } = response;
    let parse = spawn_blocking_with_permit(operation_permit, move || {
        from_utf8(&body)
            .map_err(|error| LoginError::invalid_response(status, InvalidResponseReason::InvalidUtf8(error), None))
            .and_then(|body| parse_login_response(body, status, &content_type))
    });
    let (response, _operation_permit) = time::timeout_at(deadline, parse)
        .await
        .map_err(|error| LoginError::unavailable("Second Life's login response exceeded the processing deadline.", error))?
        .map_err(|error| LoginError::unavailable(format!("Could not parse the Second Life login response: {error}"), error))?;
    let response = response?;

    match response {
        login::Response::MfaChallenge => Ok(LoginResponse::MfaChallenge),
        login::Response::Rejected { message, reason } => Err(LoginError::Rejected { message, reason }),
        login::Response::Authenticated(value) => authenticated_response(*value, credentials),
    }
}

fn parse_login_response(body: &str, status: StatusCode, content_type: &str) -> Result<login::Response, LoginError> {
    let body_start = body.trim_start();
    let is_html = has_ascii_prefix(body_start, b"<!doctype html")
        || has_ascii_prefix(body_start, b"<html")
        || content_type.split(';').next().is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("text/html"));
    if is_html && contains_ascii_case_insensitive(body.as_bytes(), b"errors.edgesuite.net") {
        return Err(LoginError::invalid_response(status, InvalidResponseReason::GatewayHtml, None));
    }
    if is_html {
        return Err(LoginError::invalid_response(status, InvalidResponseReason::HtmlInsteadOfXmlRpc, None));
    }
    match login::Response::from_xml(body, MAX_LOGIN_XML_DEPTH) {
        Err(error @ login::Error::Fault(_)) => {
            return Err(LoginError::invalid_response(status, InvalidResponseReason::Protocol(error), None));
        }
        Err(login::Error::DepthExceeded) => {
            return Err(LoginError::invalid_response(status, InvalidResponseReason::ExcessiveNesting, None));
        }
        Err(login::Error::ElementLimitExceeded) => {
            return Err(LoginError::invalid_response(status, InvalidResponseReason::TooManyElements, None));
        }
        Ok(response) if status.is_success() => return Ok(response),
        Err(error) if status.is_success() && !matches!(error, login::Error::InvalidXml) => {
            return Err(LoginError::invalid_response(status, InvalidResponseReason::Protocol(error), None));
        }
        Err(error) => {
            return Err(LoginError::invalid_response(status, InvalidResponseReason::MalformedXmlRpc(error), Some(content_type)));
        }
        Ok(_) => {}
    }
    Err(LoginError::invalid_response(status, InvalidResponseReason::NonSuccessHttpStatus, Some(content_type)))
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

fn authenticated_response(value: login::Authenticated, credentials: &LoginCredentials) -> Result<LoginResponse, LoginError> {
    let ip = value.simulator_address.ip();
    if ip.is_unspecified() || ip.is_multicast() || ip == IpAddr::V4(Ipv4Addr::BROADCAST) {
        return Err(LoginError::InvalidAuthenticatedData(AuthenticatedDataError::SimulatorIp));
    }
    if value.simulator_address.port() == 0 {
        return Err(LoginError::InvalidAuthenticatedData(AuthenticatedDataError::SimulatorPort));
    }
    let seed_capability =
        Url::parse(&value.seed_capability).map_err(|_| LoginError::InvalidAuthenticatedData(AuthenticatedDataError::CapabilityUrl))?;
    validate_http_url(&seed_capability).map_err(|_| LoginError::InvalidAuthenticatedData(AuthenticatedDataError::CapabilityUrl))?;
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
#[path = "login_tests.rs"]
mod tests;
