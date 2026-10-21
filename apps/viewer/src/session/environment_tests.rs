use super::*;
use crate::session::world::viewer_world_limits;
use network::testing::simulator_id;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

const RESPONSE: &str = r#"<llsd><map><key>environment</key><map>
<key>region_id</key><uuid>00112233-4455-6677-8899-aabbccddeeff</uuid>
<key>parcel_id</key><integer>42</integer><key>env_version</key><integer>7</integer>
<key>day_length</key><integer>14400</integer><key>day_offset</key><integer>0</integer>
<key>track_altitudes</key><array><real>1000</real><integer>2000</integer><real>3000</real></array>
<key>day_cycle</key><map><key>type</key><string>daycycle</string>
<key>frames</key><map><key>sky</key><map><key>type</key><string>sky</string></map>
<key>water</key><map><key>type</key><string>water</string></map></map>
<key>tracks</key><array><array><map><key>key_keyframe</key><real>0</real><key>key_name</key><string>water</string></map></array>
<array><map><key>key_keyframe</key><real>0</real><key>key_name</key><string>sky</string></map></array><array/><array/><array/></array>
</map></map></map></llsd>"#;

fn scope() -> EnvironmentScope {
    EnvironmentScope::Parcel(ParcelId::new(42).unwrap())
}

#[test]
fn response_preserves_source_and_metadata() {
    let response = decode(RESPONSE.as_bytes(), scope()).unwrap();
    assert_eq!(response.scope, scope());
    assert_eq!(response.region_id, Some(UUID::from_u128(0x00112233445566778899aabbccddeeff)));
    assert_eq!(response.assignment.version, Some(7));
    assert_eq!(response.assignment.day_length_seconds, Some(14_400));
    assert_eq!(response.assignment.day_offset_seconds, Some(0));
    assert_eq!(response.assignment.altitudes, Some([1000.0, 2000.0, 3000.0]));
    let EnvironmentSource::Inline(bytes) = response.assignment.source else { panic!("inline day cycle missing") };
    let value: Value = notation::from_slice(&bytes).unwrap();
    let Value::Map(fields) = value else { panic!("day cycle must remain a map") };
    assert_eq!(fields.get("type"), Some(&Value::String("daycycle".into())));
    let Some(Value::Array(tracks)) = fields.get("tracks") else { panic!("tracks missing") };
    assert_eq!(tracks.len(), 5, "all authored sky layers survive intake");
    for value in [
        RESPONSE.replace("<integer>14400</integer>", "<integer>-1</integer>"),
        RESPONSE.replace("<integer>7</integer>", "<integer>-2</integer>"),
    ] {
        let response = decode(value.as_bytes(), scope()).unwrap();
        assert!(response.assignment.day_length_seconds.is_none() || response.assignment.version.is_none());
    }
    let fallback = br#"<llsd><map><key>environment</key><map><key>parcel_id</key><integer>-1</integer></map></map></llsd>"#;
    let response = decode(fallback, scope()).unwrap();
    assert_eq!(response.scope, EnvironmentScope::Region);
    assert_eq!(response.assignment.source, EnvironmentSource::Inherit);
}

#[test]
fn malformed_responses_are_rejected() {
    for (name, bytes, expected) in [
        ("foreign parcel", RESPONSE.replace("<integer>42</integer>", "<integer>43</integer>"), FetchError::Superseded),
        ("bad scope", RESPONSE.replace("<integer>42</integer>", "<integer>-2</integer>"), FetchError::Malformed),
        ("nonfinite altitude", RESPONSE.replace("<real>1000</real>", "<real>inf</real>"), FetchError::Malformed),
        ("bad day cycle", "<llsd><map><key>environment</key><map><key>parcel_id</key><integer>42</integer><key>day_cycle</key><array/></map></map></llsd>".into(), FetchError::Malformed),
        ("bad version", RESPONSE.replace("<integer>7</integer>", "<string>7</string>"), FetchError::Malformed),
        ("oversized source", " ".repeat(MAX_ENVIRONMENT_SOURCE_BYTES + 1), FetchError::Malformed),
    ] {
        assert_eq!(decode(bytes.as_bytes(), scope()).err(), Some(expected), "{name}");
    }
    assert_eq!(decode(RESPONSE.as_bytes(), EnvironmentScope::Region).err(), Some(FetchError::Superseded));
}

#[test]
fn parcel_notifications_preserve_sentinels() {
    let value: Value = notation::from_slice(
        br#"{'ParcelData':[{'RequestResult':i0,'SequenceID':i0,'LocalID':i42}],
        'ParcelEnvironmentBlock':[{'ParcelEnvironmentVersion':i-1,'RegionAllowEnvironmentOverride':false}]}"#,
    )
    .unwrap();
    assert_eq!(
        parcel_value(&value).unwrap(),
        Some(ParcelNotice { parcel: ParcelId::new(42).unwrap(), sequence: 0, version: -1, overrides: false })
    );
    let legacy: Value = notation::from_slice(br#"{'ParcelData':[{'RequestResult':i0,'SequenceID':i1,'LocalID':i0}]}"#).unwrap();
    assert_eq!(
        parcel_value(&legacy).unwrap(),
        Some(ParcelNotice { parcel: ParcelId::new(0).unwrap(), sequence: 1, version: -2, overrides: true })
    );
    let empty: Value = notation::from_slice(br#"{'ParcelData':[{'RequestResult':i-1}]}"#).unwrap();
    assert_eq!(parcel_value(&empty).unwrap(), None);
    let bad: Value = notation::from_slice(br#"{'ParcelData':[{'RequestResult':i0,'SequenceID':i0,'LocalID':i-1}]}"#).unwrap();
    assert_eq!(parcel_value(&bad), Err(FetchError::Malformed));
}

#[tokio::test]
async fn parcel_transitions_reject_late_results() {
    let mut world = SessionWorld::new(viewer_world_limits());
    let simulator = simulator_id();
    let region = world.register_simulator(simulator, None).unwrap();
    let client = Client::new(Url::parse("http://127.0.0.1:1/").unwrap()).unwrap();
    let mut intake = EnvironmentIntake::new(Handle::current());
    intake.routes.insert(region, Route { client, parcel: None, parcel_order: None });
    let notice = ParcelNotice { parcel: ParcelId::new(42).unwrap(), sequence: 1, version: 7, overrides: true };
    intake.parcel(&mut world, region, ParcelNotice { sequence: -10000, ..notice });
    assert_eq!(world.world().environment(region).unwrap().parcel, None, "selected parcels do not select the avatar parcel");
    intake.parcel(&mut world, region, notice);
    let old = intake.queued.remove(&(region, scope())).unwrap();
    intake.order.clear();
    intake.parcel(&mut world, region, ParcelNotice { parcel: ParcelId::new(43).unwrap(), sequence: 2, ..notice });
    intake.parcel(&mut world, region, ParcelNotice { sequence: 3, ..notice });
    intake.queued.clear();
    intake.order.clear();
    intake.complete(&mut world, Completion { request: old, result: decode(RESPONSE.as_bytes(), scope()) });
    assert_eq!(world.world().environment(region).unwrap().parcel.unwrap().assignment, None, "late A results cannot survive A → B → A");
    intake.parcel(&mut world, region, ParcelNotice { sequence: 0, overrides: false, ..notice });
    let parcel = world.world().environment(region).unwrap().parcel.unwrap();
    assert_eq!(parcel.assignment.unwrap().source, EnvironmentSource::Inherit, "disabling parcel overrides clears active settings");
    intake.parcel(&mut world, region, ParcelNotice { sequence: 1, parcel: ParcelId::new(43).unwrap(), ..notice });
    assert_eq!(
        world.world().environment(region).unwrap().parcel.unwrap().parcel,
        notice.parcel,
        "older parcel sequences cannot change selection"
    );
}

#[tokio::test]
async fn capability_fetch_reaches_world() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut paths = Vec::new();
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = stream.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0, "request framing incomplete");
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(header) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..header]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= header + 4 + length {
                        break;
                    }
                }
            }
            let path = String::from_utf8_lossy(&bytes).lines().next().unwrap().to_owned();
            let body = if path.starts_with("POST /seed ") {
                format!("<llsd><map><key>ExtEnvironment</key><string>http://{address}/environment</string></map></llsd>")
            } else {
                assert_eq!(path, "GET /environment?parcelid=42 HTTP/1.1");
                RESPONSE.to_owned()
            };
            let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(body.as_bytes()).await.unwrap();
            paths.push(path);
        }
        paths
    });
    let mut world = SessionWorld::new(viewer_world_limits());
    let simulator = simulator_id();
    let region = world.register_simulator(simulator, None).unwrap();
    let client = Client::new(Url::parse(&format!("http://{address}/seed")).unwrap()).unwrap();
    let mut intake = EnvironmentIntake::new(Handle::current());
    intake.routes.insert(region, Route { client, parcel: None, parcel_order: None });
    intake.parcel(&mut world, region, ParcelNotice { parcel: ParcelId::new(42).unwrap(), sequence: 1, version: 7, overrides: true });
    let Err(Some(completion)) = intake.select(pending::<()>()).await else { panic!("environment completion missing") };
    intake.complete(&mut world, *completion);
    let observed = world.world().environment(region).unwrap().parcel.unwrap().assignment.unwrap();
    assert_eq!(observed.version, Some(7));
    assert!(matches!(observed.source, EnvironmentSource::Inline(_)));
    assert_eq!(server.await.unwrap().len(), 2);
}
