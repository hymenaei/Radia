use super::*;
use assets::Priority;
use llsd::{Value, binary, xml};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
};
use url::Url;
use utils::AssetId;
use uuid::Uuid;

struct Peer {
    url: Url,
    thread: thread::JoinHandle<Vec<Vec<u8>>>,
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let count = stream.read(&mut chunk).unwrap();
        assert!(count > 0, "request ended before framing completed");
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(header) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            let length = String::from_utf8_lossy(&bytes[..header])
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if bytes.len() >= header + 4 + length {
                return bytes;
            }
        }
    }
}

impl Peer {
    fn new(count: usize, mut respond: impl FnMut(usize, &[u8], &Url) -> (u16, Vec<u8>) + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let base = url.clone();
        let thread = thread::spawn(move || {
            let mut requests = Vec::new();
            for index in 0..count {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                            thread::sleep(Duration::from_millis(1))
                        }
                        Err(error) => panic!("expected capability request: {error}"),
                    }
                };
                let request = read_request(&mut stream);
                let (status, body) = respond(index, &request, &base);
                let header = format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(header.as_bytes()).and_then(|_| stream.write_all(&body));
                requests.push(request);
            }
            requests
        });
        Self { url, thread }
    }
    fn finish(self) -> Vec<Vec<u8>> {
        self.thread.join().unwrap()
    }
}

fn grants(base: &Url) -> Vec<u8> {
    xml::to_vec(&Value::Map(
        [
            ("ViewerAsset".into(), Value::String(base.join("content").unwrap().to_string())),
            ("RenderMaterials".into(), Value::String(base.join("materials").unwrap().to_string())),
        ]
        .into(),
    ))
    .unwrap()
}

fn resource(kind: Kind) -> assets::Resource {
    assets::Resource { id: AssetId::new(Uuid::from_u128(1)), kind }
}

#[tokio::test]
async fn capability_adapter_builds_texture_pbr_and_legacy_requests() {
    let peer = Peer::new(4, |index, request, base| {
        let header = String::from_utf8_lossy(request);
        match index {
            0 => {
                assert!(header.starts_with("POST /seed HTTP/1.1"));
                (200, grants(base))
            }
            1 => {
                assert!(header.starts_with("GET /content/?texture_id=00000000-0000-0000-0000-000000000001 HTTP/1.1"));
                (200, b"texture".to_vec())
            }
            2 => {
                assert!(header.starts_with("GET /content/?material_id=00000000-0000-0000-0000-000000000001 HTTP/1.1"));
                (200, b"pbr".to_vec())
            }
            3 => {
                assert!(header.starts_with("POST /materials HTTP/1.1"));
                let body = request.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap() + 4;
                let value: Value = xml::from_slice(&request[body..]).unwrap();
                assert!(matches!(value.as_map().unwrap().get("Zipped"), Some(Value::Binary(_))));
                (200, b"legacy".to_vec())
            }
            _ => unreachable!(),
        }
    });
    let transport = CapabilityTransport::new(Client::new(peer.url.join("seed").unwrap()).unwrap());
    let mut store = Store::new(Budget::default());
    let route = store.register_route().unwrap();
    for (kind, expected) in
        [(Kind::Texture, b"texture".as_slice()), (Kind::PbrMaterial, b"pbr".as_slice()), (Kind::LegacyMaterial, b"legacy".as_slice())]
    {
        let demand = store.request(route, assets::Request::new(resource(kind)), Priority::Visible, Instant::now()).unwrap();
        let job = store.next_job(Instant::now()).unwrap();
        let result = transport.fetch(&job).await.unwrap();
        assert_eq!(result, expected);
        drop(job);
        drop(demand);
    }
    assert_eq!(peer.finish().len(), 4, "capability discovery is shared across resource kinds");
}

#[tokio::test]
async fn missing_grants_and_discovery_retry() {
    let peer = Peer::new(2, |index, _, _| if index == 0 { (503, Vec::new()) } else { (200, b"<llsd><map/></llsd>".to_vec()) });
    let transport = CapabilityTransport::new(Client::new(peer.url.join("seed").unwrap()).unwrap());
    let mut store = Store::new(Budget::default());
    let route = store.register_route().unwrap();
    let now = Instant::now();
    let _demand = store.request(route, assets::Request::new(resource(Kind::Texture)), Priority::Visible, now).unwrap();
    let job = store.next_job(now).unwrap();
    let first = transport.fetch(&job).await;
    assert!(matches!(first, Err(Failure::Transient { .. })));
    let first = first.and_then(|bytes| job.prepare(bytes));
    store.complete(job, first, now);
    let job = store.next_job(now + Duration::from_secs(1)).unwrap();
    assert_eq!(transport.fetch(&job).await, Err(Failure::MissingCapability));
    store.complete(job, Err(Failure::MissingCapability), now + Duration::from_secs(1));
    assert!(store.next_job(now + Duration::from_secs(2)).is_none());
    assert_eq!(peer.finish().len(), 2);
}

#[tokio::test]
async fn material_preparation_publishes_content() {
    use assets::{AlphaMode, Content, MaterialDescription, Status};

    let peer = Peer::new(2, |index, _, base| {
        if index == 0 {
            return (200, grants(base));
        }
        let fields = Value::Map(
            [
                ("version".into(), Value::String("1.0".into())),
                ("type".into(), Value::String("GLTF 2.0".into())),
                (
                    "data".into(),
                    Value::String(r#"{"asset":{"version":"2.0"},"materials":[{"alphaMode":"MASK","alphaCutoff":0.75}]}"#.into()),
                ),
            ]
            .into(),
        );
        (200, binary::to_vec(&fields).unwrap())
    });
    let client = Client::new(peer.url.join("seed").unwrap()).unwrap();
    let (cancel, cancelled) = oneshot::channel();
    let budget = Budget::default();
    let service = AssetCoordinator::with_budget(
        Handle::current(),
        async {
            let _ = cancelled.await;
        },
        budget.clone(),
    );
    let region = Region::new(1);
    service.register(region, client);
    let mut demand = service.handle().request(region, assets::Request::new(resource(Kind::PbrMaterial)), Priority::Visible).await.unwrap();
    let material = loop {
        match demand.status() {
            Status::Ready(Content::Material(material)) => break material,
            Status::Failed { failure, .. } => panic!("unexpected material failure: {failure:?}"),
            Status::StaleSession | Status::Ready(Content::Texture(_)) => panic!("unexpected content or stale session"),
            _ => {}
        }
        demand.changed().await;
    };
    let MaterialDescription::Pbr(description) = &*material else { panic!("expected PBR material") };
    assert_eq!(description.alpha_mode, AlphaMode::Mask);
    assert_eq!(description.alpha_cutoff, 0.75);
    cancel.send(()).unwrap();
    service.stop().await;
    assert_eq!(budget.stats().charged_bytes, size_of::<MaterialDescription>());
    drop(material);
    assert_eq!(budget.stats().charged_bytes, 0);
    assert_eq!(peer.finish().len(), 2);
}

#[tokio::test]
async fn shared_images_outlive_session() {
    let (started, received) = oneshot::channel();
    let mut started = Some(started);
    let (release, hold) = std::sync::mpsc::channel();
    let peer = Peer::new(2, move |index, _, base| {
        if index == 0 {
            return (200, grants(base));
        }
        started.take().unwrap().send(()).unwrap();
        hold.recv_timeout(Duration::from_secs(5)).unwrap();
        (200, include_bytes!("../../../crates/assets/tests/fixtures/rgba.j2c").to_vec())
    });
    let client = Client::new(peer.url.join("seed").unwrap()).unwrap();
    let (cancel, cancelled) = oneshot::channel();
    let budget = Budget::default();
    let service = AssetCoordinator::with_budget(
        Handle::current(),
        async {
            let _ = cancelled.await;
        },
        budget.clone(),
    );
    let region = Region::new(1);
    service.register(region, client);
    let handle = service.handle();
    let first = handle.request(region, assets::Request::new(resource(Kind::Texture)), Priority::Visible).await.unwrap();
    let mut second = handle.request(region, assets::Request::new(resource(Kind::Texture)), Priority::Visible).await.unwrap();
    received.await.unwrap();
    drop(first);
    assert_eq!(budget.stats().charged_bytes, Kind::Texture.max_response_bytes(), "the remaining face still owns the active fetch");
    release.send(()).unwrap();
    let blob = loop {
        match second.status() {
            assets::Status::Ready(assets::Content::Texture(blob)) => break blob,
            assets::Status::Failed { failure, .. } => panic!("unexpected {failure:?}"),
            _ => {}
        }
        second.changed().await;
    };
    assert_eq!(blob.image().unwrap().pixels(), include_bytes!("../../../crates/assets/tests/fixtures/rgba.raw"));
    assert_eq!(budget.stats().high_water_bytes, 192 * 1024 * 1024);
    cancel.send(()).unwrap();
    service.stop().await;
    assert!(matches!(second.status(), assets::Status::StaleSession));
    assert_eq!(
        budget.stats().resident_bytes,
        blob.image().unwrap().pixels().len() + include_bytes!("../../../crates/assets/tests/fixtures/rgba.j2c").len()
    );
    assert!(matches!(
        handle.request(region, assets::Request::new(resource(Kind::Texture)), Priority::Visible).await,
        Err(RequestError::Closed)
    ));
    drop(second);
    drop(blob);
    assert_eq!(budget.stats().charged_bytes, 0);
    assert_eq!(peer.finish().len(), 2);
}

#[tokio::test]
async fn shutdown_cancels_active_fetch() {
    let (started, received) = oneshot::channel();
    let mut started = Some(started);
    let (release, hold) = std::sync::mpsc::channel();
    let peer = Peer::new(2, move |index, _, base| {
        if index == 0 {
            return (200, grants(base));
        }
        started.take().unwrap().send(()).unwrap();
        hold.recv_timeout(Duration::from_secs(5)).unwrap();
        (200, b"late data".to_vec())
    });
    let client = Client::new(peer.url.join("seed").unwrap()).unwrap();
    let (cancel, cancelled) = oneshot::channel();
    let budget = Budget::default();
    let service = AssetCoordinator::with_budget(
        Handle::current(),
        async {
            let _ = cancelled.await;
        },
        budget.clone(),
    );
    let region = Region::new(1);
    service.register(region, client);
    let demand = service.handle().request(region, assets::Request::new(resource(Kind::Texture)), Priority::Visible).await.unwrap();
    received.await.unwrap();
    assert_eq!(budget.stats().charged_bytes, Kind::Texture.max_response_bytes());
    drop(demand);
    cancel.send(()).unwrap();
    service.stop().await;
    assert_eq!(budget.stats().charged_bytes, 0);
    release.send(()).unwrap();
    assert_eq!(peer.finish().len(), 2);
}
