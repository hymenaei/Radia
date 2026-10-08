use crate::{AgentSession, Grid, LoginCredentials, LoginValue, StartLocation};

use super::http::{LocalServer, response};
use crate::login::{LoginResponse, login_at};
use protocol::{
    Audible, ChatSourceType, ChatType, Vector3,
    messages::{ChatFromSimulator, chat_from_simulator::ChatData},
    udp::Message,
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
        format!("HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
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
            format!("<value><struct><member><name>folder_id</name><value><string>{FOLDER_ID}</string></value></member></struct></value>")
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

    let event = crate::Event::LoginAccepted { mfa_hash: Some("secret-mfa-hash".into()) };
    let event_debug = format!("{event:?}");
    assert!(!event_debug.contains("secret-mfa-hash"));
    assert!(event_debug.contains("[REDACTED]"));

    let event = crate::Event::SimulatorMessage {
        simulator_id: crate::event::SimulatorId::new(1),
        source: "192.0.2.123:13000".parse().expect("valid simulator address"),
        sequence: 7,
        message: Message::ChatFromSimulator(ChatFromSimulator {
            chat_data: ChatData {
                from_name: "private-avatar-name".into(),
                source_id: AGENT_ID.parse().expect("valid source ID"),
                owner_id: SESSION_ID.parse().expect("valid owner ID"),
                source_type: ChatSourceType::Agent,
                chat_type: ChatType::Normal,
                audible: Audible::Fully,
                position: Vector3::default(),
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
