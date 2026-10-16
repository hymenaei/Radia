use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    num::NonZeroU32,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use base::UUID;
use futures::{FutureExt, channel::mpsc, pin_mut, sink::SinkExt, stream::StreamExt};
use gpui::BackgroundExecutor;
use llsd::{Map, Value};
use network::{
    SimulatorId,
    capabilities::{CapabilityUrl, Client},
};
use tokio::sync::watch;
use world::{
    Entity, LocalAddress, MAX_MEDIA_DIMENSION, MAX_MEDIA_URL_BYTES, MAX_MEDIA_WHITELIST_BYTES, MAX_MEDIA_WHITELIST_ENTRIES,
    MAX_SURFACE_FACES, MediaControls, MediaEntry, ObjectMediaError, ObjectMediaUpdate, OrderEvidence, UpdateSource,
};

const REQUEST_QUEUE_CAPACITY: usize = 64;
const RESULT_QUEUE_CAPACITY: usize = 16;
const CAPABILITY_CACHE_CAPACITY: usize = 32;
const RESPONSE_LIMIT_BYTES: usize = 256 * 1024;
const MAX_FETCH_ATTEMPTS: u8 = 3;
const RETRY_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ObjectMediaTarget {
    pub(super) simulator_id: SimulatorId,
    pub(super) entity: Entity,
    pub(super) source: UpdateSource,
    pub(super) object_id: UUID,
    pub(super) local_id: NonZeroU32,
    pub(super) version: u32,
}

impl ObjectMediaTarget {
    fn key(self) -> MediaKey {
        MediaKey {
            simulator_id: self.simulator_id,
            address: LocalAddress::new(self.source.region(), self.local_id),
            object_id: self.object_id,
        }
    }

    pub(super) fn is_newer_than(self, other: Self) -> bool {
        self.version > other.version
            || (self.version == other.version
                && matches!((self.source.order(), other.source.order()), (OrderEvidence::Ordered(a), OrderEvidence::Ordered(b)) if a > b))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct MediaKey {
    simulator_id: SimulatorId,
    address: LocalAddress,
    object_id: UUID,
}

struct FetchRequest {
    target: ObjectMediaTarget,
    client: Client,
}

struct PendingMedia {
    active: ObjectMediaTarget,
    retry: Option<ObjectMediaTarget>,
}

impl PendingMedia {
    fn coalesce(&mut self, target: ObjectMediaTarget) {
        let latest = self.retry.unwrap_or(self.active);
        if target.entity != latest.entity || target.is_newer_than(latest) {
            self.retry = Some(target);
        }
    }
}

pub(super) struct ObjectMediaResult {
    pub(super) target: ObjectMediaTarget,
    pub(super) update: Result<ObjectMediaUpdate, ()>,
}

#[derive(Default)]
struct RetirementState {
    outstanding: HashMap<SimulatorId, usize>,
    retired: HashSet<SimulatorId>,
}

impl RetirementState {
    fn reserve(&mut self, simulator_id: SimulatorId) -> bool {
        if self.retired.contains(&simulator_id) {
            return false;
        }
        *self.outstanding.entry(simulator_id).or_default() += 1;
        true
    }

    fn retire(&mut self, simulator_id: SimulatorId) -> bool {
        if !self.outstanding.contains_key(&simulator_id) {
            return false;
        }
        self.retired.insert(simulator_id)
    }

    fn finish(&mut self, simulator_id: SimulatorId) {
        let Some(count) = self.outstanding.get_mut(&simulator_id) else { return };
        if *count > 1 {
            *count -= 1;
            return;
        }
        self.outstanding.remove(&simulator_id);
        self.retired.remove(&simulator_id);
    }

    fn is_retired(&self, simulator_id: SimulatorId) -> bool {
        self.retired.contains(&simulator_id)
    }
}

pub(super) fn response_is_stale(response_version: u32, requested_version: u32) -> bool {
    response_version < requested_version
}

/// Fetches ObjectMedia for a session and tracks in-flight work and simulator retirement.
pub(super) struct ObjectMediaFetcher {
    requests: mpsc::Sender<FetchRequest>,
    results: mpsc::Receiver<ObjectMediaResult>,
    pending: HashMap<MediaKey, PendingMedia>,
    retirement: Arc<Mutex<RetirementState>>,
    retirement_generation: watch::Sender<u64>,
}

impl ObjectMediaFetcher {
    pub(super) fn new<F>(background: BackgroundExecutor, cancellation: F) -> Self
    where
        F: Future<Output = ()> + Clone + Send + 'static,
    {
        let (request_sender, request_receiver) = mpsc::channel(REQUEST_QUEUE_CAPACITY);
        let (result_sender, result_receiver) = mpsc::channel(RESULT_QUEUE_CAPACITY);
        let retirement = Arc::new(Mutex::new(RetirementState::default()));
        let (retirement_generation, retirement_updates) = watch::channel(0);
        background
            .clone()
            .spawn(run(request_receiver, result_sender, background, retirement.clone(), retirement_updates, cancellation))
            .detach();
        Self { requests: request_sender, results: result_receiver, pending: HashMap::new(), retirement, retirement_generation }
    }

    pub(super) fn enqueue(&mut self, target: ObjectMediaTarget, client: Client) -> bool {
        if self.retirement.lock().unwrap_or_else(PoisonError::into_inner).is_retired(target.simulator_id) {
            return false;
        }
        let key = target.key();
        if let Some(pending) = self.pending.get_mut(&key) {
            pending.coalesce(target);
            return true;
        }
        if !self.retirement.lock().unwrap_or_else(PoisonError::into_inner).reserve(target.simulator_id) {
            return false;
        }
        if self.requests.try_send(FetchRequest { target, client }).is_err() {
            self.retirement.lock().unwrap_or_else(PoisonError::into_inner).finish(target.simulator_id);
            return false;
        }
        self.pending.insert(key, PendingMedia { active: target, retry: None });
        true
    }

    pub(super) async fn next_result(&mut self) -> Option<ObjectMediaResult> {
        self.results.next().await
    }

    pub(super) fn finish(&mut self, result: &ObjectMediaResult) -> Option<ObjectMediaTarget> {
        self.pending.remove(&result.target.key()).and_then(|pending| pending.retry)
    }

    pub(super) fn forget_simulator(&mut self, simulator_id: SimulatorId) {
        self.pending.retain(|key, _| key.simulator_id != simulator_id);
        if self.retirement.lock().unwrap_or_else(PoisonError::into_inner).retire(simulator_id) {
            self.retirement_generation.send_modify(|generation| *generation = generation.saturating_add(1));
        }
    }
}

async fn run<F>(
    mut requests: mpsc::Receiver<FetchRequest>,
    mut results: mpsc::Sender<ObjectMediaResult>,
    background: BackgroundExecutor,
    retirement: Arc<Mutex<RetirementState>>,
    mut retirement_updates: watch::Receiver<u64>,
    cancellation: F,
) where
    F: Future<Output = ()> + Clone + Send + 'static,
{
    let mut capabilities = HashMap::<SimulatorId, CapabilityUrl>::new();
    let mut cache_order = VecDeque::new();
    loop {
        let cancelled = cancellation.clone().fuse();
        let request = requests.next().fuse();
        pin_mut!(cancelled, request);
        let request = futures::select! {
            _ = cancelled => return,
            request = request => request,
        };
        let Some(request) = request else { return };
        if is_retired(&retirement, request.target.simulator_id) {
            finish_request(&retirement, request.target.simulator_id);
            continue;
        }

        let outcome = {
            let fetch = fetch_with_retries(&request, &mut capabilities, &mut cache_order, &background).fuse();
            pin_mut!(fetch);
            loop {
                let cancelled = cancellation.clone().fuse();
                let retired_change = retirement_updates.changed().fuse();
                pin_mut!(cancelled, retired_change);
                futures::select! {
                    _ = cancelled => break FetchOutcome::Cancelled,
                    changed = retired_change => {
                        if changed.is_err() || is_retired(&retirement, request.target.simulator_id) {
                            break FetchOutcome::Retired;
                        }
                    }
                    update = fetch => break FetchOutcome::Complete(update),
                }
            }
        };
        let update = match outcome {
            FetchOutcome::Cancelled => {
                finish_request(&retirement, request.target.simulator_id);
                return;
            }
            FetchOutcome::Retired => {
                finish_request(&retirement, request.target.simulator_id);
                continue;
            }
            FetchOutcome::Complete(update) => update,
        };
        let result = ObjectMediaResult { target: request.target, update };
        let send = results.send(result).fuse();
        pin_mut!(send);
        let cancelled = cancellation.clone().fuse();
        pin_mut!(cancelled);
        if futures::select! { _ = cancelled => true, sent = send => sent.is_err() } {
            finish_request(&retirement, request.target.simulator_id);
            return;
        }
        finish_request(&retirement, request.target.simulator_id);
    }
}

enum FetchOutcome {
    Cancelled,
    Retired,
    Complete(Result<ObjectMediaUpdate, ()>),
}

fn is_retired(retirement: &Mutex<RetirementState>, simulator_id: SimulatorId) -> bool {
    retirement.lock().unwrap_or_else(PoisonError::into_inner).is_retired(simulator_id)
}

fn finish_request(retirement: &Mutex<RetirementState>, simulator_id: SimulatorId) {
    retirement.lock().unwrap_or_else(PoisonError::into_inner).finish(simulator_id);
}

async fn fetch_with_retries(
    request: &FetchRequest,
    capabilities: &mut HashMap<SimulatorId, CapabilityUrl>,
    cache_order: &mut VecDeque<SimulatorId>,
    background: &BackgroundExecutor,
) -> Result<ObjectMediaUpdate, ()> {
    for attempt in 0..MAX_FETCH_ATTEMPTS {
        match fetch(request, capabilities, cache_order).await {
            Ok(update) if update.version() >= request.target.version => return Ok(update),
            Ok(_) => {}
            Err(FetchError::Transport) => {
                capabilities.remove(&request.target.simulator_id);
                cache_order.retain(|simulator_id| *simulator_id != request.target.simulator_id);
            }
            Err(FetchError::Capability) if attempt + 1 < MAX_FETCH_ATTEMPTS => {}
            Err(FetchError::Malformed) => return Err(()),
            Err(FetchError::Capability) => return Err(()),
        }
        if attempt + 1 < MAX_FETCH_ATTEMPTS {
            background.timer(RETRY_DELAY).await;
        }
    }
    Err(())
}

async fn fetch(
    request: &FetchRequest,
    capabilities: &mut HashMap<SimulatorId, CapabilityUrl>,
    cache_order: &mut VecDeque<SimulatorId>,
) -> Result<ObjectMediaUpdate, FetchError> {
    let simulator_id = request.target.simulator_id;
    let capability = if let Some(capability) = capabilities.get(&simulator_id) {
        capability.clone()
    } else {
        let mut grants = request.client.request_capabilities(&["ObjectMedia"]).await.map_err(|_| FetchError::Capability)?;
        let capability = grants.remove("ObjectMedia").ok_or(FetchError::Capability)?;
        if capabilities.len() >= CAPABILITY_CACHE_CAPACITY
            && let Some(oldest) = cache_order.pop_front()
        {
            capabilities.remove(&oldest);
        }
        cache_order.push_back(simulator_id);
        capabilities.insert(simulator_id, capability.clone());
        capability
    };

    let mut payload = Map::new();
    payload.insert("verb".into(), Value::String("GET".into()));
    payload.insert("object_id".into(), Value::UUID(request.target.object_id));
    let response =
        request.client.post_with_response_limit(&capability, &payload, RESPONSE_LIMIT_BYTES).await.map_err(|_| FetchError::Transport)?;
    parse_response(&response, request.target.object_id).map_err(|_| FetchError::Malformed)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FetchError {
    Capability,
    Malformed,
    Transport,
}

fn parse_response(response: &Value, expected_id: UUID) -> Result<ObjectMediaUpdate, ParseError> {
    let response = response.as_map().ok_or(ParseError::Malformed)?;
    if response.contains_key("error") {
        return Err(ParseError::Malformed);
    }
    if response.get("object_id").and_then(Value::as_uuid) != Some(expected_id) {
        return Err(ParseError::ObjectMismatch);
    }
    let version = response.get("object_media_version").and_then(Value::as_str).and_then(media_version).ok_or(ParseError::Malformed)?;
    let faces = response.get("object_media_data").and_then(Value::as_array).ok_or(ParseError::Malformed)?;
    if faces.len() > MAX_SURFACE_FACES {
        return Err(ParseError::Malformed);
    }
    let faces = faces
        .iter()
        .map(|face| match face {
            Value::Undefined => Ok(None),
            Value::Map(fields) => parse_entry(fields).map(Some),
            _ => Err(ParseError::Malformed),
        })
        .collect::<Result<Vec<_>, _>>()?;
    ObjectMediaUpdate::new(version, faces).map_err(ParseError::Invalid)
}

fn parse_entry(fields: &Map) -> Result<MediaEntry, ParseError> {
    let mut entry = MediaEntry::default();
    entry.alt_image_enable = bool_field(fields, "alt_image_enable", entry.alt_image_enable)?;
    entry.controls = match fields.get("controls") {
        None | Some(Value::Integer(0)) => MediaControls::Standard,
        Some(Value::Integer(1)) => MediaControls::Mini,
        Some(_) => return Err(ParseError::Malformed),
    };
    entry.current_url = owned_string(fields, "current_url", &entry.current_url)?;
    entry.home_url = owned_string(fields, "home_url", &entry.home_url)?;
    entry.auto_loop = bool_field(fields, "auto_loop", entry.auto_loop)?;
    entry.auto_play = bool_field(fields, "auto_play", entry.auto_play)?;
    entry.auto_scale = bool_field(fields, "auto_scale", entry.auto_scale)?;
    entry.auto_zoom = bool_field(fields, "auto_zoom", entry.auto_zoom)?;
    entry.first_click_interact = bool_field(fields, "first_click_interact", entry.first_click_interact)?;
    entry.width_pixels = dimension_field(fields, "width_pixels", entry.width_pixels)?;
    entry.height_pixels = dimension_field(fields, "height_pixels", entry.height_pixels)?;
    entry.whitelist_enable = bool_field(fields, "whitelist_enable", entry.whitelist_enable)?;
    entry.whitelist = whitelist_field(fields)?;
    entry.perms_interact = permission_field(fields, "perms_interact", entry.perms_interact)?;
    entry.perms_control = permission_field(fields, "perms_control", entry.perms_control)?;
    Ok(entry)
}

fn bool_field(fields: &Map, key: &str, default: bool) -> Result<bool, ParseError> {
    match fields.get(key) {
        None => Ok(default),
        Some(value) => value.as_bool().ok_or(ParseError::Malformed),
    }
}

fn string_field<'a>(fields: &'a Map, key: &str, default: &'a str) -> Result<&'a str, ParseError> {
    match fields.get(key) {
        None => Ok(default),
        Some(value) => value.as_str().ok_or(ParseError::Malformed),
    }
}

fn owned_string(fields: &Map, key: &str, default: &str) -> Result<String, ParseError> {
    let value = string_field(fields, key, default)?;
    if value.len() > MAX_MEDIA_URL_BYTES {
        return Err(ParseError::Invalid(ObjectMediaError::UrlTooLong));
    }
    Ok(value.to_owned())
}

fn dimension_field(fields: &Map, key: &str, default: u16) -> Result<u16, ParseError> {
    let Some(value) = fields.get(key) else { return Ok(default) };
    let value = value.as_integer().ok_or(ParseError::Malformed)?;
    let value = u16::try_from(value).map_err(|_| ParseError::Malformed)?;
    if value > MAX_MEDIA_DIMENSION {
        return Err(ParseError::Invalid(ObjectMediaError::DimensionTooLarge));
    }
    Ok(value)
}

fn permission_field(fields: &Map, key: &str, default: u8) -> Result<u8, ParseError> {
    let Some(value) = fields.get(key) else { return Ok(default) };
    u8::try_from(value.as_integer().ok_or(ParseError::Malformed)?).map_err(|_| ParseError::Malformed)
}

fn whitelist_field(fields: &Map) -> Result<Vec<String>, ParseError> {
    let Some(value) = fields.get("whitelist") else { return Ok(Vec::new()) };
    let values = value.as_array().ok_or(ParseError::Malformed)?;
    if values.len() > MAX_MEDIA_WHITELIST_ENTRIES {
        return Err(ParseError::Invalid(ObjectMediaError::WhitelistTooLarge));
    }
    let mut total_bytes = 0usize;
    values
        .iter()
        .map(|value| {
            let value = value.as_str().ok_or(ParseError::Malformed)?;
            total_bytes = total_bytes.saturating_add(value.len());
            if total_bytes > MAX_MEDIA_WHITELIST_BYTES {
                return Err(ParseError::Invalid(ObjectMediaError::WhitelistTooLarge));
            }
            Ok(value.to_owned())
        })
        .collect()
}

pub(super) fn media_version(value: &str) -> Option<u32> {
    let value = value.strip_prefix("x-mv:")?;
    let (version, agent_id) = value.split_once('/')?;
    UUID::parse_str(agent_id).ok()?;
    version.parse().ok()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParseError {
    Malformed,
    ObjectMismatch,
    Invalid(ObjectMediaError),
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use base::UUID;
    use llsd::{Map, Value};
    use network::testing::simulator_id;
    use world::{
        Appearance, Kind, MAX_MEDIA_URL_BYTES, MAX_MEDIA_WHITELIST_BYTES, MediaControls, ObjectObservation, OrderEvidence, ParentUpdate,
        Shape, UpdateSource,
    };

    use crate::session::{
        inbound::{ObjectBatch, ObjectMediaRequest, ObjectOperation},
        world::{SessionWorld, viewer_world_limits},
    };

    use super::{ObjectMediaTarget, PendingMedia, RetirementState, media_version, parse_entry, parse_response, response_is_stale};

    fn replacement_targets() -> (ObjectMediaTarget, ObjectMediaTarget) {
        let simulator_id = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        let region = world.register_simulator(simulator_id, None).expect("register simulator");
        let local_id = NonZeroU32::new(7).expect("nonzero local ID");
        let object_id = UUID::from_u128(7);
        let observation = |order| ObjectObservation {
            source: UpdateSource::new(region, order, OrderEvidence::Ordered(u64::from(order)), 0),
            id: object_id,
            local_id,
            kind: Kind::Object,
            parent: ParentUpdate::Root,
            pose: None,
            linear_motion: None,
            angular_velocity: None,
            scale: None,
            attachment_point: None,
            shape: Some(Some(Shape::Prim)),
            appearance: Some(Some(Appearance::Surface)),
            surface_appearance: None,
            render_materials: None,
        };
        let apply = |world: &mut SessionWorld, operation| {
            world
                .apply_object_batch(
                    simulator_id,
                    ObjectBatch { region, region_handle: None, blocks: vec![Some(operation)], invalid_blocks: 0 },
                )
                .expect("registered simulator")
        };
        let request = |order| ObjectMediaRequest {
            source: UpdateSource::new(region, order, OrderEvidence::Ordered(u64::from(order)), 0),
            object_id,
            local_id,
            version: 5,
        };

        apply(&mut world, ObjectOperation::Observe(observation(1)));
        let active = world.prepare_object_media(simulator_id, request(2)).expect("current surface target");
        apply(&mut world, ObjectOperation::Remove { source: UpdateSource::new(region, 3, OrderEvidence::Ordered(3), 0), local_id });
        apply(&mut world, ObjectOperation::Observe(observation(4)));
        let replacement = world.prepare_object_media(simulator_id, request(5)).expect("current replacement target");
        (active, replacement)
    }

    fn response(object_id: UUID) -> Value {
        let mut fields = Map::new();
        fields.insert("object_id".into(), Value::UUID(object_id));
        fields.insert("object_media_version".into(), Value::String("x-mv:8/00000000-0000-0000-0000-000000000001".into()));
        fields.insert("object_media_data".into(), Value::Array(vec![Value::Map(Map::new())]));
        Value::Map(fields)
    }

    #[test]
    fn version_marker_requires_version_and_agent() {
        assert_eq!(media_version("x-mv:12/00000000-0000-0000-0000-000000000001"), Some(12));
        assert_eq!(media_version("https://example.test/media"), None);
        assert_eq!(media_version("x-mv:bad/00000000-0000-0000-0000-000000000001"), None);
    }

    #[test]
    fn retry_discards_older_media_version() {
        assert!(response_is_stale(7, 8));
        assert!(!response_is_stale(8, 8));
    }

    #[test]
    fn replacement_target_is_retried() {
        let (active, replacement) = replacement_targets();
        assert_ne!(active.entity, replacement.entity);
        assert_eq!(active.version, replacement.version);

        let mut pending = PendingMedia { active, retry: None };
        pending.coalesce(replacement);

        assert_eq!(pending.retry, Some(replacement));
    }

    #[test]
    fn older_marker_does_not_replace_retry() {
        let (active, _) = replacement_targets();
        let latest = ObjectMediaTarget { version: 8, ..active };
        let older = ObjectMediaTarget { version: 7, ..active };
        let mut pending = PendingMedia { active, retry: Some(latest) };

        pending.coalesce(older);

        assert_eq!(pending.retry, Some(latest));
    }

    #[test]
    fn retirement_lives_with_requests() {
        let simulator_id = simulator_id();
        let mut retirement = RetirementState::default();

        assert!(!retirement.retire(simulator_id));
        assert!(retirement.reserve(simulator_id));
        assert!(retirement.reserve(simulator_id));
        assert!(retirement.retire(simulator_id));
        assert!(!retirement.reserve(simulator_id));

        retirement.finish(simulator_id);
        assert!(retirement.is_retired(simulator_id));
        retirement.finish(simulator_id);
        assert!(!retirement.is_retired(simulator_id));
        assert!(retirement.outstanding.is_empty());
        assert!(retirement.reserve(simulator_id));
    }

    #[test]
    fn response_checks_identity_and_faces() {
        let id = UUID::from_u128(7);
        assert_eq!(parse_response(&response(id), id).unwrap().version(), 8);
        assert!(parse_response(&response(UUID::from_u128(9)), id).is_err());
        let mut value = response(id);
        let Value::Map(fields) = &mut value else { panic!("test response is a map") };
        fields.insert("object_media_data".into(), Value::String("bad".into()));
        assert!(parse_response(&value, id).is_err());
    }

    #[test]
    fn media_controls_use_integer_values() {
        let mut fields = Map::new();
        fields.insert("controls".into(), Value::Integer(0));
        assert_eq!(parse_entry(&fields).unwrap().controls, MediaControls::Standard);

        fields.insert("controls".into(), Value::Integer(1));
        assert_eq!(parse_entry(&fields).unwrap().controls, MediaControls::Mini);

        fields.remove("controls");
        assert_eq!(parse_entry(&fields).unwrap().controls, MediaControls::Standard);
    }

    #[test]
    fn media_controls_reject_invalid_values() {
        for value in [Value::Integer(-1), Value::Integer(2), Value::String("MINI".into())] {
            let fields = Map::from([("controls".into(), value)]);
            assert!(parse_entry(&fields).is_err());
        }
    }

    #[test]
    fn response_rejects_oversized_fields() {
        let id = UUID::from_u128(7);
        let mut value = response(id);
        let Value::Map(fields) = &mut value else { panic!("test response is a map") };
        let Value::Array(faces) = fields.get_mut("object_media_data").unwrap() else { panic!("test faces are an array") };
        let mut media = Map::new();
        media.insert("current_url".into(), Value::String("x".repeat(MAX_MEDIA_URL_BYTES + 1)));
        faces[0] = Value::Map(media);
        assert!(parse_response(&value, id).is_err());

        let mut value = response(id);
        let Value::Map(fields) = &mut value else { panic!("test response is a map") };
        let Value::Array(faces) = fields.get_mut("object_media_data").unwrap() else { panic!("test faces are an array") };
        let mut media = Map::new();
        media.insert("whitelist".into(), Value::Array(vec![Value::String("x".repeat(MAX_MEDIA_WHITELIST_BYTES + 1))]));
        faces[0] = Value::Map(media);
        assert!(parse_response(&value, id).is_err());
    }
}
