pub use dxr::{Fault, Value};

use std::{
    collections::BTreeMap,
    error, fmt,
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

use dxr::{FaultResponse, MethodCall, MethodResponse};
use md5::{Digest, Md5};
use quick_xml::{Reader, events::Event};
use uuid::Uuid;

const MAX_LOGIN_XML_ELEMENTS: usize = 262_144;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StartLocation {
    Last,
    Home,

    Region { name: String, x: u32, y: u32, z: u32 },
}

impl StartLocation {
    pub fn login_parameter(&self) -> String {
        match self {
            Self::Last => "last".into(),
            Self::Home => "home".into(),
            Self::Region { name, x, y, z } => format!("uri:{name}&{x}&{y}&{z}"),
        }
    }
}

pub struct Request<'a> {
    pub first_name: &'a str,
    pub last_name: &'a str,
    pub password: &'a str,
    pub start: &'a StartLocation,
    pub channel: &'a str,
    pub version: &'a str,
    pub platform: &'a str,
    pub mac: &'a str,
    pub id0: &'a str,
    pub viewer_digest: &'a str,
    pub agree_to_tos: &'a str,
    pub read_critical: &'a str,
    pub token: &'a str,
    pub mfa_hash: &'a str,
    pub options: &'a [&'a str],
}

impl fmt::Debug for Request<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request").field("fields", &"[REDACTED]").finish()
    }
}

impl Request<'_> {
    pub fn to_xml(&self) -> Result<String, Error> {
        let mut fields = BTreeMap::new();
        let digest = Md5::digest(self.password.as_bytes());
        for (name, value) in [
            ("first", self.first_name.to_owned()),
            ("last", self.last_name.to_owned()),
            ("passwd", format!("$1${}", utils::hex_lower(&digest))),
            ("start", self.start.login_parameter()),
            ("channel", self.channel.to_owned()),
            ("version", self.version.to_owned()),
            ("platform", self.platform.to_owned()),
            ("mac", self.mac.to_owned()),
            ("id0", self.id0.to_owned()),
            ("viewer_digest", self.viewer_digest.to_owned()),
            ("agree_to_tos", self.agree_to_tos.to_owned()),
            ("read_critical", self.read_critical.to_owned()),
            ("token", self.token.to_owned()),
            ("mfa_hash", self.mfa_hash.to_owned()),
        ] {
            fields.insert(name.to_owned(), Value::String(value));
        }
        fields.insert("options".into(), Value::Array(self.options.iter().map(|value| Value::String((*value).to_owned())).collect()));
        let call = MethodCall { name: "login_to_simulator".into(), params: vec![Value::Struct(fields)] };
        Ok(format!("<?xml version=\"1.0\"?>\n{}\n", call.to_xml().map_err(|_| Error::Encode)?))
    }
}

#[derive(Clone, Default)]
pub struct ResponseData {
    fields: BTreeMap<String, Value>,
}

impl ResponseData {
    pub fn from_fields(fields: BTreeMap<String, Value>) -> Self {
        Self { fields }
    }
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields.get(name)
    }

    pub fn inventory_bootstrap(&self) -> Result<InventoryBootstrap, InventoryBootstrapError> {
        let personal_root = option_uuid(&self.fields, "inventory-root", "folder_id", InventoryBootstrapError::InvalidPersonalRoot)?;
        let library_root = option_uuid(&self.fields, "inventory-lib-root", "folder_id", InventoryBootstrapError::InvalidLibraryRoot)?;
        let library_owner = option_uuid(&self.fields, "inventory-lib-owner", "agent_id", InventoryBootstrapError::InvalidLibraryOwner)?;
        let personal_skeleton = skeleton(
            &self.fields,
            "inventory-skeleton",
            InventoryBootstrapError::InvalidPersonalSkeleton,
            InventoryBootstrapError::TooManyPersonalFolders,
        )?;
        let library_skeleton = skeleton(
            &self.fields,
            "inventory-skel-lib",
            InventoryBootstrapError::InvalidLibrarySkeleton,
            InventoryBootstrapError::TooManyLibraryFolders,
        )?;
        Ok(InventoryBootstrap { personal_root, personal_skeleton, library_root, library_owner, library_skeleton })
    }
}

pub const MAX_LOGIN_INVENTORY_FOLDERS: usize = 65_536;
pub const MAX_INVENTORY_FOLDER_NAME_BYTES: usize = 1_024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventorySkeletonFolder {
    pub folder_id: Uuid,
    pub parent_id: Uuid,
    pub name: String,
    pub version: i32,
    pub type_default: Option<i32>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InventorySkeleton {
    pub folders: Vec<InventorySkeletonFolder>,
    pub rejected_entries: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InventoryBootstrap {
    pub personal_root: Option<Uuid>,
    pub personal_skeleton: InventorySkeleton,
    pub library_root: Option<Uuid>,
    pub library_owner: Option<Uuid>,
    pub library_skeleton: InventorySkeleton,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryBootstrapError {
    InvalidPersonalRoot,
    InvalidLibraryRoot,
    InvalidLibraryOwner,
    InvalidPersonalSkeleton,
    InvalidLibrarySkeleton,
    TooManyPersonalFolders,
    TooManyLibraryFolders,
}

impl fmt::Display for InventoryBootstrapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidPersonalRoot => "login response has an invalid personal inventory root",
            Self::InvalidLibraryRoot => "login response has an invalid Library root",
            Self::InvalidLibraryOwner => "login response has an invalid Library owner",
            Self::InvalidPersonalSkeleton => "login response has an invalid personal inventory skeleton",
            Self::InvalidLibrarySkeleton => "login response has an invalid Library skeleton",
            Self::TooManyPersonalFolders => "login response has too many personal inventory folders",
            Self::TooManyLibraryFolders => "login response has too many Library folders",
        })
    }
}

impl error::Error for InventoryBootstrapError {}

fn option_uuid(
    fields: &BTreeMap<String, Value>,
    field: &str,
    id_field: &str,
    error: InventoryBootstrapError,
) -> Result<Option<Uuid>, InventoryBootstrapError> {
    let Some(value) = fields.get(field) else { return Ok(None) };
    let Value::Array(entries) = value else { return Err(error) };
    let entry = match entries.first() {
        None => return Ok(None),
        Some(Value::Struct(entry)) => entry,
        Some(_) => return Err(error),
    };
    let Some(Value::String(value)) = entry.get(id_field) else { return Err(error) };
    let id = Uuid::parse_str(value).map_err(|_| error)?;
    if id.is_nil() {
        return Err(error);
    }
    Ok(Some(id))
}

fn skeleton(
    fields: &BTreeMap<String, Value>,
    field: &str,
    invalid: InventoryBootstrapError,
    too_many: InventoryBootstrapError,
) -> Result<InventorySkeleton, InventoryBootstrapError> {
    let Some(value) = fields.get(field) else { return Ok(InventorySkeleton::default()) };
    let Value::Array(entries) = value else { return Err(invalid) };
    if entries.len() > MAX_LOGIN_INVENTORY_FOLDERS {
        return Err(too_many);
    }
    let mut parsed = InventorySkeleton { folders: Vec::with_capacity(entries.len()), rejected_entries: 0 };
    for entry in entries {
        match skeleton_folder(entry) {
            Some(folder) => parsed.folders.push(folder),
            None => parsed.rejected_entries = parsed.rejected_entries.saturating_add(1),
        }
    }
    Ok(parsed)
}

fn skeleton_folder(value: &Value) -> Option<InventorySkeletonFolder> {
    let Value::Struct(fields) = value else { return None };
    let Value::String(folder_id) = fields.get("folder_id")? else { return None };
    let folder_id = Uuid::parse_str(folder_id).ok().filter(|id| !id.is_nil())?;
    let Value::String(parent_id) = fields.get("parent_id")? else { return None };
    let parent_id = Uuid::parse_str(parent_id).ok()?;
    let Value::String(name) = fields.get("name")? else { return None };
    if name.len() > MAX_INVENTORY_FOLDER_NAME_BYTES {
        return None;
    }
    let version = fields.get("version").and_then(inventory_i32)?;
    let type_default = match fields.get("type_default") {
        Some(value) => Some(inventory_i32(value)?),
        None => None,
    };
    Some(InventorySkeletonFolder { folder_id, parent_id, name: name.clone(), version, type_default })
}

fn inventory_i32(value: &Value) -> Option<i32> {
    match value {
        Value::Integer(value) => Some(*value),
        Value::Long(value) => i32::try_from(*value).ok(),
        Value::String(value) => value.parse().ok(),
        _ => None,
    }
}

impl fmt::Debug for ResponseData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResponseData").field("field_names", &self.fields.keys().collect::<Vec<_>>()).finish()
    }
}

pub struct Authenticated {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub message: String,

    pub simulator_address: SocketAddr,
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub circuit_code: u32,

    pub seed_capability: String,
    pub mfa_hash: Option<String>,
    pub data: ResponseData,
}

impl fmt::Debug for Authenticated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Authenticated").field("session", &"[REDACTED]").field("data", &self.data).finish()
    }
}

pub enum Response {
    Authenticated(Box<Authenticated>),
    MfaChallenge,
    Rejected { message: String, reason: String },
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Authenticated(value) => value.fmt(f),
            Self::MfaChallenge => f.write_str("MfaChallenge"),
            Self::Rejected { .. } => f.write_str("Rejected { [REDACTED] }"),
        }
    }
}

impl Response {
    pub fn from_xml(input: &str, max_depth: usize) -> Result<Self, Error> {
        preflight_xml(input, max_depth, MAX_LOGIN_XML_ELEMENTS)?;
        if let Ok(response) = FaultResponse::from_xml(input) {
            return Err(Error::Fault(response.fault));
        }
        let response = MethodResponse::from_xml(input).map_err(|_| Error::InvalidXml)?;
        let Value::Struct(fields) = response.value else {
            return Err(Error::InvalidResponse);
        };
        if string_field(&fields, "reason").as_deref() == Some("mfa_challenge") {
            return Ok(Self::MfaChallenge);
        }
        let succeeded = match fields.get("login") {
            Some(Value::Boolean(value)) => *value,
            Some(Value::String(value)) => value.eq_ignore_ascii_case("true"),
            Some(Value::Integer(value)) => *value != 0,
            _ => false,
        };
        if !succeeded {
            return Ok(Self::Rejected {
                message: string_field(&fields, "message").unwrap_or_default(),
                reason: string_field(&fields, "reason").unwrap_or_default(),
            });
        }
        let ip = ip_field(&fields, "sim_ip")?;
        let port = integer_field(&fields, "sim_port")?.try_into().map_err(|_| Error::InvalidField("sim_port"))?;
        let value = Authenticated {
            first_name: string_field(&fields, "first_name"),
            last_name: string_field(&fields, "last_name"),
            message: string_field(&fields, "message").unwrap_or_default(),
            simulator_address: SocketAddr::new(ip, port),
            agent_id: uuid_field(&fields, "agent_id")?,
            session_id: uuid_field(&fields, "session_id")?,
            circuit_code: integer_field(&fields, "circuit_code")?.try_into().map_err(|_| Error::InvalidField("circuit_code"))?,
            seed_capability: required_string_field(&fields, "seed_capability")?.to_owned(),
            mfa_hash: string_field(&fields, "mfa_hash"),
            data: ResponseData::from_fields(fields),
        };
        Ok(Self::Authenticated(Box::new(value)))
    }
}

#[derive(Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum Error {
    Encode,
    InvalidXml,
    InvalidResponse,
    DepthExceeded,
    ElementLimitExceeded,
    Fault(Fault),
    MissingField(&'static str),
    InvalidField(&'static str),
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fault(fault) => f.debug_struct("Fault").field("code", &fault.code()).field("message", &"[REDACTED]").finish(),
            _ => fmt::Display::fmt(self, f),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode => f.write_str("Could not encode the Second Life login request."),
            Self::InvalidXml => f.write_str("Second Life returned an invalid XML-RPC login response."),
            Self::InvalidResponse => f.write_str("Second Life returned an invalid login response."),
            Self::DepthExceeded => f.write_str("Second Life returned an XML-RPC login response with excessive nesting."),
            Self::ElementLimitExceeded => f.write_str("Second Life returned an XML-RPC login response with too many elements."),
            Self::Fault(fault) => fault.fmt(f),
            Self::MissingField("seed_capability") => f.write_str("Second Life omitted the simulator capability URL."),
            Self::MissingField(name) => write!(f, "Second Life omitted {name}."),
            Self::InvalidField("sim_ip") => f.write_str("Second Life returned an invalid sim_ip address."),
            Self::InvalidField("sim_port") => f.write_str("Second Life returned an invalid simulator port."),
            Self::InvalidField("circuit_code") => f.write_str("Second Life returned an invalid circuit code."),
            Self::InvalidField(name) => write!(f, "Second Life returned an invalid {name}."),
        }
    }
}

impl error::Error for Error {}

fn preflight_xml(input: &str, max_depth: usize, max_elements: usize) -> Result<(), Error> {
    let mut reader = Reader::from_str(input);
    let mut depth = 0;
    let mut elements = 0;
    loop {
        match reader.read_event() {
            Ok(Event::Start(_)) => {
                if depth >= max_depth {
                    return Err(Error::DepthExceeded);
                }
                if elements == max_elements {
                    return Err(Error::ElementLimitExceeded);
                }
                elements += 1;
                depth += 1;
            }
            Ok(Event::Empty(_)) => {
                if depth >= max_depth {
                    return Err(Error::DepthExceeded);
                }
                if elements == max_elements {
                    return Err(Error::ElementLimitExceeded);
                }
                elements += 1;
            }
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Eof) | Err(_) => return Ok(()),
            _ => {}
        }
    }
}

fn string_field(fields: &BTreeMap<String, Value>, name: &str) -> Option<String> {
    match fields.get(name) {
        Some(Value::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn required_string_field<'a>(fields: &'a BTreeMap<String, Value>, name: &'static str) -> Result<&'a str, Error> {
    match fields.get(name) {
        Some(Value::String(value)) => Ok(value),
        Some(_) => Err(Error::InvalidField(name)),
        None => Err(Error::MissingField(name)),
    }
}

fn ip_field(fields: &BTreeMap<String, Value>, name: &'static str) -> Result<IpAddr, Error> {
    match fields.get(name) {
        Some(Value::String(value)) => value.parse().map_err(|_| Error::InvalidField(name)),
        Some(Value::Integer(value)) => Ok(IpAddr::V4(Ipv4Addr::from((*value as u32).to_be_bytes()))),
        Some(Value::Long(value)) if (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(value) => {
            Ok(IpAddr::V4(Ipv4Addr::from((*value as u32).to_be_bytes())))
        }
        _ => Err(Error::InvalidField(name)),
    }
}

fn integer_field(fields: &BTreeMap<String, Value>, name: &'static str) -> Result<i64, Error> {
    match fields.get(name) {
        Some(Value::Integer(value)) => Ok(i64::from(*value)),
        Some(Value::Long(value)) => Ok(*value),
        Some(Value::String(value)) => value.parse().map_err(|_| Error::InvalidField(name)),
        Some(_) => Err(Error::InvalidField(name)),
        None => Err(Error::MissingField(name)),
    }
}

fn uuid_field(fields: &BTreeMap<String, Value>, name: &'static str) -> Result<Uuid, Error> {
    Uuid::parse_str(required_string_field(fields, name)?).map_err(|_| Error::InvalidField(name))
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;

    fn xml_response(fields: &[(&str, &str)]) -> String {
        let members =
            fields.iter().map(|(name, value)| format!("<member><name>{name}</name><value>{value}</value></member>")).collect::<String>();
        format!("<methodResponse><params><param><value><struct>{members}</struct></value></param></params></methodResponse>")
    }

    fn success(ip: &str) -> String {
        xml_response(&[
            ("login", "<string>TRUE</string>"),
            ("sim_ip", ip),
            ("sim_port", "<string>13000</string>"),
            ("circuit_code", "<int>1</int>"),
            ("agent_id", "<string>00112233-4455-6677-8899-aabbccddeeff</string>"),
            ("session_id", "<string>ffeeddcc-bbaa-9988-7766-554433221100</string>"),
            ("seed_capability", "<string>https://example.test/seed</string>"),
            ("mfa_hash", "<string>private-hash</string>"),
            ("inventory-skeleton", "<array><data><value><string>preserved</string></value></data></array>"),
        ])
    }

    #[test]
    fn request_wire_fields() {
        let location = StartLocation::Region { name: "A&B".into(), x: 12, y: 34, z: 56 };
        let request = Request {
            first_name: "Test",
            last_name: "Resident",
            password: "password",
            start: &location,
            channel: "Test viewer",
            version: "1.2.3.4",
            platform: "Win",
            mac: "00:00:00:00:00:00",
            id0: "00000000-0000-0000-0000-000000000000",
            viewer_digest: "digest",
            agree_to_tos: "",
            read_critical: "",
            token: "secret-token",
            mfa_hash: "secret-hash",
            options: &["inventory-skeleton"],
        };
        let encoded = request.to_xml().expect("encode request");
        assert!(encoded.starts_with("<?xml version=\"1.0\"?>"));
        let call = MethodCall::from_xml(&encoded).expect("decode XML-RPC with dxr");
        assert_eq!(call.name, "login_to_simulator");
        let [Value::Struct(fields)] = call.params.as_slice() else { panic!("one struct parameter") };
        assert_eq!(
            fields.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "agree_to_tos",
                "channel",
                "first",
                "id0",
                "last",
                "mac",
                "mfa_hash",
                "options",
                "passwd",
                "platform",
                "read_critical",
                "start",
                "token",
                "version",
                "viewer_digest"
            ]
        );
        assert_eq!(fields.get("passwd"), Some(&Value::String("$1$5f4dcc3b5aa765d61d8327deb882cf99".into())));
        assert_eq!(fields.get("start"), Some(&Value::String("uri:A&B&12&34&56".into())));
        assert_eq!(fields.get("options"), Some(&Value::Array(vec![Value::String("inventory-skeleton".into())])));
        assert_eq!(fields.get("token"), Some(&Value::String("secret-token".into())));
        assert_eq!(StartLocation::Last.login_parameter(), "last");
        assert_eq!(StartLocation::Home.login_parameter(), "home");
        let debug = format!("{request:?}");
        for secret in ["password", "secret-token", "secret-hash", "A&B"] {
            assert!(!debug.contains(secret));
        }
    }

    #[test]
    fn response_outcomes() {
        let denied = xml_response(&[
            ("login", "<boolean>0</boolean>"),
            ("message", "<string>Denied</string>"),
            ("reason", "<string>login</string>"),
        ]);
        assert_matches!(
            Response::from_xml(&denied, 64).expect("decode rejection"),
            Response::Rejected { message, reason } if message == "Denied" && reason == "login"
        );
        let mfa = xml_response(&[("reason", "<string>mfa_challenge</string>")]);
        assert_matches!(Response::from_xml(&mfa, 64).expect("decode MFA"), Response::MfaChallenge);
        let fault = "<methodResponse><fault><value><struct><member><name>faultCode</name><value><int>7</int></value></member><member><name>faultString</name><value><string>private-fault</string></value></member></struct></value></fault></methodResponse>";
        let error = Response::from_xml(fault, 64).expect_err("XML-RPC fault");
        assert_matches!(&error, Error::Fault(value) if value.code() == 7 && value.string() == "private-fault");
        assert!(!format!("{error:?}").contains("private-fault"));
        assert_eq!(Response::from_xml("<broken>", 64).expect_err("malformed XML"), Error::InvalidXml);
        assert_eq!(
            Response::from_xml("<methodResponse><params><param><value><int>1</int></value></param></params></methodResponse>", 64)
                .expect_err("struct required"),
            Error::InvalidResponse
        );
    }

    #[test]
    fn authenticated_numeric_fields() {
        for (wire, expected) in [
            ("<int>-2147483648</int>", "128.0.0.0"),
            ("<i8>-2147483648</i8>", "128.0.0.0"),
            ("<i8>4294967295</i8>", "255.255.255.255"),
            ("<string>127.0.0.1</string>", "127.0.0.1"),
        ] {
            let Response::Authenticated(value) = Response::from_xml(&success(wire), 64).expect("decode typed wire fields") else {
                panic!("authenticated")
            };
            assert_eq!(value.simulator_address, SocketAddr::new(expected.parse().expect("IP"), 13000));
            assert_eq!(value.agent_id.to_string(), "00112233-4455-6677-8899-aabbccddeeff");
            assert_eq!(value.session_id.to_string(), "ffeeddcc-bbaa-9988-7766-554433221100");
            assert_eq!(value.circuit_code, 1);
            assert_eq!(value.first_name, None);
            assert_eq!(value.last_name, None);
            assert_eq!(value.mfa_hash.as_deref(), Some("private-hash"));
            assert_eq!(value.data.get("inventory-skeleton"), Some(&Value::Array(vec![Value::String("preserved".into())])));
            assert!(!format!("{value:?}").contains("private-hash"));
        }
        for wire in ["<i8>-2147483649</i8>", "<i8>4294967296</i8>", "<string>invalid-ip</string>"] {
            assert_eq!(Response::from_xml(&success(wire), 64).expect_err("invalid numeric address"), Error::InvalidField("sim_ip"));
        }
        let missing = xml_response(&[("login", "<boolean>1</boolean>"), ("sim_ip", "<string>127.0.0.1</string>")]);
        assert_eq!(Response::from_xml(&missing, 64).expect_err("missing port"), Error::MissingField("sim_port"));
    }

    #[test]
    fn wrong_field_types_are_invalid() {
        let original = success("<string>127.0.0.1</string>");
        for (field, value) in [
            ("sim_port", "<string>13000</string>"),
            ("circuit_code", "<int>1</int>"),
            ("agent_id", "<string>00112233-4455-6677-8899-aabbccddeeff</string>"),
            ("seed_capability", "<string>https://example.test/seed</string>"),
        ] {
            let xml = original.replace(
                &format!("<name>{field}</name><value>{value}</value>"),
                &format!("<name>{field}</name><value><boolean>1</boolean></value>"),
            );
            assert_eq!(Response::from_xml(&xml, 64).expect_err("wrong field type"), Error::InvalidField(field), "{field}");
        }
    }

    #[test]
    fn caller_depth_limit() {
        let shallow = xml_response(&[
            ("login", "<boolean>0</boolean>"),
            ("extra", &format!("<array><data>{}</data></array>", "<value><string>x</string></value>".repeat(1024))),
        ]);
        assert!(Response::from_xml(&shallow, 64).is_ok());
        let nested = xml_response(&[(
            "extra",
            &format!("{}<string>x</string>{}", "<array><data><value>".repeat(24), "</value></data></array>".repeat(24)),
        )]);
        assert_eq!(Response::from_xml(&nested, 64).expect_err("depth policy"), Error::DepthExceeded);
        assert_eq!(Response::from_xml(&shallow, 0).expect_err("zero depth limit"), Error::DepthExceeded);
        let empty = "<methodResponse><params><param><value><struct/></value></param></params></methodResponse>";
        assert!(Response::from_xml(empty, 5).is_ok());
        assert_eq!(Response::from_xml(empty, 4).expect_err("empty element depth"), Error::DepthExceeded);
    }

    #[test]
    fn xml_preflight_counts_empty_elements() {
        assert_eq!(preflight_xml("<root><empty/><child></child></root>", 2, 3), Ok(()));
        assert_eq!(preflight_xml("<root><empty/><child></child></root>", 2, 2), Err(Error::ElementLimitExceeded));
        assert_eq!(preflight_xml("<root><empty/></root>", 1, 3), Err(Error::DepthExceeded));
        assert_eq!(preflight_xml("<root><empty/></root>", 2, 1), Err(Error::ElementLimitExceeded));
    }

    #[test]
    fn response_bounds_element_count() {
        let items = "<value><string>x</string></value>".repeat(MAX_LOGIN_XML_ELEMENTS / 2 + 1);
        let xml =
            format!("<methodResponse><params><param><value><array><data>{items}</data></array></value></param></params></methodResponse>");

        assert_eq!(Response::from_xml(&xml, 64).expect_err("element policy"), Error::ElementLimitExceeded);
    }
}
