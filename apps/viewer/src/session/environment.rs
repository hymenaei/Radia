use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};

use futures::{
    FutureExt, StreamExt,
    future::{AbortHandle, Abortable, BoxFuture, pending},
    stream::FuturesUnordered,
};
use llsd::{Limits, Map, Value, notation, xml};
use math::Vec3;
use network::{PacketOrder, SimulatorId, capabilities::Client};
use protocol::{lludp::Message, messages::ParcelProperties};
use tokio::{runtime::Handle, sync::Semaphore};
use uuid::Uuid;
use world::{
    EnvironmentAssignment, EnvironmentScope, EnvironmentSource, EnvironmentTarget, MAX_ENVIRONMENT_SOURCE_BYTES,
    MAX_PARCEL_DESCRIPTION_BYTES, MAX_PARCEL_NAME_BYTES, MAX_PARCEL_URL_BYTES, OrderEvidence, ParcelAccessSettings, ParcelBitmap,
    ParcelFlags, ParcelLocalId, ParcelPropertiesData, ParcelPropertiesObservation, Region, RegionOutcome, UpdateAuthority, UpdateSource,
};

use super::world::SessionWorld;

const MAX_FETCHES: usize = 4;
const MAX_REGIONS: usize = 33;

type Key = (Region, EnvironmentScope);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FetchError {
    Unavailable,
    Network,
    Malformed,
    Superseded,
    Worker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ParcelNotice {
    parcel: ParcelLocalId,
    sequence: i32,
    version: i32,
    overrides: bool,
}

struct Route {
    client: Client,
    parcel: Option<ParcelNotice>,
    parcel_order: Option<u64>,
}

#[derive(Clone, Copy)]
enum RouteChange {
    SessionReady,
    Promoted,
    Capabilities,
}

struct Request {
    target: EnvironmentTarget,
    source: UpdateSource,
    region_id: Option<Uuid>,
    version: Option<u32>,
    client: Client,
}

struct Response {
    region_id: Option<Uuid>,
    scope: EnvironmentScope,
    assignment: EnvironmentAssignment,
}

pub(super) struct Completion {
    request: Request,
    result: Result<Response, FetchError>,
}

pub(super) struct EnvironmentIntake {
    runtime: Handle,
    decoders: Arc<Semaphore>,
    routes: HashMap<Region, Route>,
    sequence: u64,
    order: VecDeque<Key>,
    queued: HashMap<Key, Request>,
    active: HashMap<Key, AbortHandle>,
    jobs: FuturesUnordered<BoxFuture<'static, (Key, Option<Completion>)>>,
}

impl EnvironmentIntake {
    pub(super) fn new(runtime: Handle) -> Self {
        Self {
            runtime,
            decoders: Arc::new(Semaphore::new(MAX_FETCHES)),
            routes: HashMap::new(),
            sequence: 0,
            order: VecDeque::new(),
            queued: HashMap::new(),
            active: HashMap::new(),
            jobs: FuturesUnordered::new(),
        }
    }

    fn source(&mut self, region: Region) -> Option<UpdateSource> {
        self.sequence = self.sequence.checked_add(1)?;
        Some(UpdateSource::new(region, 0, OrderEvidence::Ordered(self.sequence), 0))
    }

    fn queue(&mut self, world: &SessionWorld, region: Region, scope: EnvironmentScope) {
        let Some(target) = world.world().environment_target(region, scope) else { return };
        let Some(source) = self.source(region) else {
            tracing::error!(region = region.get(), "environment request sequence exhausted");
            return;
        };
        let Some(route) = self.routes.get(&region) else { return };
        let version = match scope {
            EnvironmentScope::Region => None,
            EnvironmentScope::Parcel(_) => route.parcel.and_then(|parcel| u32::try_from(parcel.version).ok()),
        };
        let request = Request {
            target,
            source,
            region_id: world.world().region(region).and_then(|info| info.region_id),
            version,
            client: route.client.clone(),
        };
        let key = (region, scope);
        if self.queued.insert(key, request).is_none() {
            self.order.push_back(key);
        }
    }

    fn clear_parcel(&mut self, world: &mut SessionWorld, region: Region) {
        if let Err(error) = world.clear_agent_parcel(region) {
            tracing::warn!(region = region.get(), %error, "could not clear current parcel state");
        }
        if let Some(route) = self.routes.get_mut(&region) {
            route.parcel = None;
        }
        self.cancel_parcel(region);
    }

    fn cancel_parcel(&mut self, region: Region) {
        self.queued.retain(|(known, scope), _| *known != region || *scope == EnvironmentScope::Region);
        self.order.retain(|key| self.queued.contains_key(key));
        for ((known, scope), cancel) in &self.active {
            if *known == region && *scope != EnvironmentScope::Region {
                cancel.abort();
            }
        }
    }

    fn parcel(&mut self, world: &mut SessionWorld, region: Region, notice: ParcelNotice, update_selection: bool) {
        let Some(route) = self.routes.get(&region) else { return };
        if let Some(previous) = route.parcel {
            if notice.sequence < 0 && previous.parcel != notice.parcel {
                return;
            }
            if notice.sequence > 0 && previous.sequence >= notice.sequence {
                return;
            }
            if previous.parcel == notice.parcel && notice.version >= 0 && previous.version > notice.version {
                return;
            }
            if previous.parcel == notice.parcel && previous.version == notice.version && previous.overrides == notice.overrides {
                if let Some(route) = self.routes.get_mut(&region) {
                    route.parcel = Some(ParcelNotice { sequence: previous.sequence.max(notice.sequence), ..notice });
                }
                return;
            }
        } else if notice.sequence < 0 {
            return;
        }
        let notice =
            ParcelNotice { sequence: route.parcel.map_or(notice.sequence, |previous| previous.sequence.max(notice.sequence)), ..notice };
        if route.parcel.is_some_and(|previous| previous.parcel != notice.parcel) {
            self.cancel_parcel(region);
        }
        let source = if update_selection {
            let Some(source) = self.source(region) else { return };
            let source = source.with_authority(UpdateAuthority::Untracked);
            if let Err(error) = world.observe_agent_parcel(source, Some(notice.parcel)) {
                tracing::warn!(region = region.get(), %error, "could not observe environment parcel selection");
                return;
            }
            Some(source)
        } else {
            None
        };
        let Some(source) = source.or_else(|| self.source(region)) else {
            return;
        };
        if let Some(route) = self.routes.get_mut(&region) {
            route.parcel = Some(notice);
        }
        if notice.overrides {
            self.queue(world, region, EnvironmentScope::Parcel(notice.parcel));
        } else if let Some(target) = world.world().environment_target(region, EnvironmentScope::Parcel(notice.parcel)) {
            self.cancel_parcel(region);
            self.apply(world, target, source, inherited(None));
        }
    }

    pub(super) fn session_ready(
        &mut self,
        world: &mut SessionWorld,
        simulator: SimulatorId,
        client: &Client,
        primary: Option<SimulatorId>,
    ) {
        self.route(world, simulator, client, primary, RouteChange::SessionReady);
    }

    pub(super) fn simulator_promoted(
        &mut self,
        world: &mut SessionWorld,
        previous: SimulatorId,
        simulator: SimulatorId,
        client: &Client,
        primary: Option<SimulatorId>,
    ) {
        self.route(world, simulator, client, primary, RouteChange::Promoted);
        if let Some(region) = world.region_for(previous) {
            self.clear_parcel(world, region);
        }
    }

    pub(super) fn simulator_capabilities(
        &mut self,
        world: &mut SessionWorld,
        simulator: SimulatorId,
        client: &Client,
        primary: Option<SimulatorId>,
    ) {
        self.route(world, simulator, client, primary, RouteChange::Capabilities);
    }

    fn route(
        &mut self,
        world: &mut SessionWorld,
        simulator: SimulatorId,
        client: &Client,
        primary: Option<SimulatorId>,
        change: RouteChange,
    ) {
        let Some(region) = world.region_for(simulator) else { return };
        let initial = !self.routes.contains_key(&region);
        if initial && self.routes.len() >= MAX_REGIONS {
            return;
        }
        self.routes.entry(region).or_insert_with(|| Route { client: client.clone(), parcel: None, parcel_order: None });
        let should_queue = match change {
            RouteChange::SessionReady | RouteChange::Promoted => true,
            RouteChange::Capabilities => initial && primary == Some(simulator),
        };
        if should_queue {
            self.queue(world, region, EnvironmentScope::Region);
        }
    }

    pub(super) fn simulator_disconnected(&mut self, world: &mut SessionWorld, simulator: SimulatorId) {
        let Some(region) = world.region_for(simulator) else { return };
        self.routes.remove(&region);
        self.queued.retain(|(known, _), _| *known != region);
        self.order.retain(|key| self.queued.contains_key(key));
        for ((known, _), cancel) in &self.active {
            if *known == region {
                cancel.abort();
            }
        }
    }

    pub(super) fn capability(
        &mut self,
        world: &mut SessionWorld,
        simulator: SimulatorId,
        message: &str,
        body: &Value,
        primary: Option<SimulatorId>,
    ) {
        if primary == Some(simulator) {
            self.capability_event(world, simulator, message, body);
        }
    }

    pub(super) fn capability_batch<'a>(
        &mut self,
        world: &mut SessionWorld,
        simulator: SimulatorId,
        events: impl IntoIterator<Item = (&'a str, &'a Value)>,
        primary: Option<SimulatorId>,
    ) {
        if primary == Some(simulator) {
            for (message, body) in events {
                self.capability_event(world, simulator, message, body);
            }
        }
    }

    pub(super) fn simulator_message(
        &mut self,
        world: &mut SessionWorld,
        simulator: SimulatorId,
        packet_order: PacketOrder,
        message: &Message,
        primary: Option<SimulatorId>,
    ) {
        if primary != Some(simulator) {
            return;
        }
        let Some(region) = world.region_for(simulator) else { return };
        match message {
            Message::ParcelProperties(message) => match parcel_message(message) {
                Ok(Some(notice)) => {
                    let PacketOrder::Ordered(order) = packet_order else { return };
                    let Some(route) = self.routes.get_mut(&region) else { return };
                    if route.parcel_order.is_some_and(|previous| previous >= order) {
                        return;
                    }
                    route.parcel_order = Some(order);
                    let current = world.world().parcel_state(region).and_then(|state| state.current);
                    if current.as_ref().is_none_or(|current| {
                        u32::try_from(notice.sequence).ok() != Some(current.sequence_id) || current.properties.local_id != notice.parcel
                    }) {
                        return;
                    }
                    self.parcel(world, region, notice, false);
                }
                Ok(None) => {}
                Err(error) => tracing::debug!(?error, "invalid parcel environment notification"),
            },
            Message::RegionInfo(_) => self.queue(world, region, EnvironmentScope::Region),
            _ => {}
        }
    }

    fn capability_event(&mut self, world: &mut SessionWorld, simulator: SimulatorId, message: &str, body: &Value) {
        if message != "ParcelProperties" {
            return;
        }
        let Some(region) = world.region_for(simulator) else { return };
        match parcel_value(body) {
            Ok(Some(notice)) => {
                match parcel_properties_value(body) {
                    Ok(observation) => {
                        if let Some(source) = self.source(region).map(|source| source.with_authority(UpdateAuthority::Untracked))
                            && world.observe_parcel_properties(source, observation).is_err()
                        {
                            self.record_parcel_rejection(world, simulator, region);
                        }
                    }
                    Err(()) => self.record_parcel_rejection(world, simulator, region),
                }
                self.parcel(world, region, notice, true);
            }
            Ok(None) => {}
            Err(_) => self.record_parcel_rejection(world, simulator, region),
        }
    }

    fn record_parcel_rejection(&self, world: &mut SessionWorld, simulator: SimulatorId, region: Region) {
        if let Some(rejected) = world.record_parcel_rejection() {
            tracing::debug!(simulator = simulator.get(), region = region.get(), rejected, "parcel updates were repeatedly rejected");
        }
    }

    fn schedule_queued(&mut self) {
        let queued = self.order.len();
        for _ in 0..queued {
            if self.jobs.len() >= MAX_FETCHES {
                break;
            }
            let key = self.order.pop_front().expect("bounded environment queue");
            if self.active.contains_key(&key) {
                self.order.push_back(key);
                continue;
            }
            let request = self.queued.remove(&key).expect("queued environment request");
            let (cancel, registration) = AbortHandle::new_pair();
            self.active.insert(key, cancel);
            let runtime = self.runtime.clone();
            let decoders = self.decoders.clone();
            self.jobs.push(
                async move {
                    let fetch = async move {
                        let result = fetch(&request, runtime, decoders).await;
                        Completion { request, result }
                    };
                    (key, Abortable::new(fetch, registration).await.ok())
                }
                .boxed(),
            );
        }
    }

    pub(super) async fn next_completion(&mut self) -> Option<Box<Completion>> {
        self.schedule_queued();
        let (key, completion) =
            async { if self.jobs.is_empty() { pending().await } else { self.jobs.next().await.expect("active environment jobs") } }.await;
        self.active.remove(&key);
        completion.map(Box::new)
    }

    pub(super) fn complete(&mut self, world: &mut SessionWorld, completion: Completion) {
        let Completion { request, result } = completion;
        let target = request.target;
        if world.world().environment_target(target.region(), target.scope()) != Some(target)
            || self.queued.contains_key(&(target.region(), target.scope()))
            || !self.routes.contains_key(&target.region())
        {
            return;
        }
        match result {
            Ok(response) => {
                if let (Some(expected), Some(received)) =
                    (world.world().region(target.region()).and_then(|info| info.region_id), response.region_id)
                    && expected != received
                {
                    return;
                }
                if response.scope == target.scope() {
                    self.apply(world, target, request.source, response.assignment);
                } else if let Some(region_target) = world.world().environment_target(target.region(), EnvironmentScope::Region) {
                    self.apply(world, region_target, request.source, response.assignment);
                    self.apply(world, target, request.source, inherited(None));
                }
            }
            Err(FetchError::Unavailable | FetchError::Superseded) => {
                tracing::debug!(region = target.region().get(), scope = ?target.scope(), "environment request unavailable or superseded");
            }
            Err(error) => {
                tracing::warn!(region = target.region().get(), ?error, scope = ?target.scope(), "environment could not be loaded")
            }
        }
    }

    fn apply(&self, world: &mut SessionWorld, target: EnvironmentTarget, source: UpdateSource, assignment: EnvironmentAssignment) {
        match world.observe_environment(target, source, assignment) {
            Ok(applied) if matches!(applied.outcome, RegionOutcome::Patched { .. }) => {
                tracing::info!(region = target.region().get(), scope = ?target.scope(), "environment assignment updated");
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(region = target.region().get(), %error, "environment assignment rejected"),
        }
    }
}

async fn fetch(request: &Request, runtime: Handle, decoders: Arc<Semaphore>) -> Result<Response, FetchError> {
    let mut capabilities = request.client.request_capabilities(&["ExtEnvironment"]).await.map_err(|_| FetchError::Network)?;
    let mut url = capabilities.remove("ExtEnvironment").ok_or(FetchError::Unavailable)?;
    if let EnvironmentScope::Parcel(parcel) = request.target.scope() {
        url = url.with_query_pair("parcelid", &parcel.get().to_string());
    }
    for attempt in 0..3 {
        let bytes = match request.client.get_bytes_with_response_limit(&url, MAX_ENVIRONMENT_SOURCE_BYTES).await {
            Ok(bytes) => bytes,
            Err(_) if attempt < 2 => {
                tokio::time::sleep(Duration::from_millis(250 << attempt)).await;
                continue;
            }
            Err(_) => return Err(FetchError::Network),
        };
        let permit = decoders.clone().acquire_owned().await.map_err(|_| FetchError::Worker)?;
        let scope = request.target.scope();
        let span = tracing::Span::current();
        let response = runtime
            .spawn_blocking(move || {
                let _entered = span.enter();
                let _permit = permit;
                decode(&bytes, scope)
            })
            .await
            .map_err(|_| FetchError::Worker)??;
        if response.region_id.is_some() && request.region_id.is_some() && response.region_id != request.region_id {
            return Err(FetchError::Superseded);
        }
        if response.scope == request.target.scope()
            && let (Some(expected), Some(version)) = (request.version, response.assignment.version)
            && version < expected
        {
            if attempt < 2 {
                tokio::time::sleep(Duration::from_millis(250 << attempt)).await;
                continue;
            }
            return Err(FetchError::Superseded);
        }
        return Ok(response);
    }
    Err(FetchError::Network)
}

fn inherited(version: Option<u32>) -> EnvironmentAssignment {
    EnvironmentAssignment {
        source: EnvironmentSource::Inherit,
        version,
        day_length_seconds: None,
        day_offset_seconds: None,
        altitudes: None,
    }
}

fn decode(bytes: &[u8], requested: EnvironmentScope) -> Result<Response, FetchError> {
    let limits = Limits {
        max_bytes: MAX_ENVIRONMENT_SOURCE_BYTES,
        max_depth: 16,
        max_values: 32_768,
        max_container_items: 280,
        max_string_bytes: 2048,
        max_map_key_bytes: 128,
        max_binary_bytes: 0,
    };
    let value: Value = xml::from_slice_with_limits(bytes, limits).map_err(|_| FetchError::Malformed)?;
    let Value::Map(root) = value else { return Err(FetchError::Malformed) };
    let Some(Value::Map(fields)) = root.get("environment") else { return Err(FetchError::Malformed) };
    let parcel = integer(fields.get("parcel_id"))?.unwrap_or(-1);
    let scope = if parcel == -1 {
        EnvironmentScope::Region
    } else {
        EnvironmentScope::Parcel(ParcelLocalId::new(parcel).ok_or(FetchError::Malformed)?)
    };
    if scope != requested && scope != EnvironmentScope::Region {
        return Err(FetchError::Superseded);
    }
    let region_id = match fields.get("region_id") {
        None | Some(Value::Undefined) => None,
        Some(Value::Uuid(id)) => (!id.is_nil()).then_some(*id),
        _ => return Err(FetchError::Malformed),
    };
    let mut assignment = inherited(integer(fields.get("env_version"))?.and_then(|version| u32::try_from(version).ok()));
    assignment.day_length_seconds = integer(fields.get("day_length"))?.and_then(|length| u32::try_from(length).ok());
    assignment.day_offset_seconds = integer(fields.get("day_offset"))?.filter(|offset| *offset != -1);
    assignment.altitudes = match fields.get("track_altitudes") {
        None | Some(Value::Undefined) => None,
        Some(Value::Array(values)) if values.len() == 3 => {
            let mut altitudes = [0.0; 3];
            for (altitude, value) in altitudes.iter_mut().zip(values) {
                *altitude = match value {
                    Value::Integer(value) => *value as f32,
                    Value::Real(value) => *value as f32,
                    _ => return Err(FetchError::Malformed),
                };
                if !altitude.is_finite() {
                    return Err(FetchError::Malformed);
                }
            }
            Some(altitudes)
        }
        _ => return Err(FetchError::Malformed),
    };
    if let Some(value) = fields.get("day_cycle").filter(|value| !matches!(value, Value::Undefined)) {
        if !matches!(value, Value::Map(_)) {
            return Err(FetchError::Malformed);
        }
        let bytes = notation::to_vec(value).map_err(|_| FetchError::Malformed)?;
        if bytes.len() > MAX_ENVIRONMENT_SOURCE_BYTES {
            return Err(FetchError::Malformed);
        }
        assignment.source = EnvironmentSource::Inline(bytes.into());
    }
    Ok(Response { region_id, scope, assignment })
}

fn integer(value: Option<&Value>) -> Result<Option<i32>, FetchError> {
    match value {
        None | Some(Value::Undefined) => Ok(None),
        Some(Value::Integer(value)) => Ok(Some(*value)),
        _ => Err(FetchError::Malformed),
    }
}

fn parcel_value(value: &Value) -> Result<Option<ParcelNotice>, FetchError> {
    let Value::Map(body) = value else { return Err(FetchError::Malformed) };
    let block = |name: &str| -> Result<&Map, FetchError> {
        let Some(Value::Array(values)) = body.get(name) else { return Err(FetchError::Malformed) };
        let [Value::Map(fields)] = values.as_slice() else { return Err(FetchError::Malformed) };
        Ok(fields)
    };
    let parcel = block("ParcelData")?;
    let result = integer(parcel.get("RequestResult"))?.ok_or(FetchError::Malformed)?;
    if result == -1 {
        return Ok(None);
    }
    if !(0..=1).contains(&result) {
        return Err(FetchError::Malformed);
    }
    let sequence = integer(parcel.get("SequenceID"))?.ok_or(FetchError::Malformed)?;
    let parcel = integer(parcel.get("LocalID"))?.and_then(ParcelLocalId::new).ok_or(FetchError::Malformed)?;
    let (version, overrides) = match body.get("ParcelEnvironmentBlock") {
        None => (-2, true),
        Some(Value::Array(values)) if values.is_empty() => (-2, true),
        Some(_) => {
            let environment = block("ParcelEnvironmentBlock")?;
            let version = integer(environment.get("ParcelEnvironmentVersion"))?.unwrap_or(-2);
            let overrides = match environment.get("RegionAllowEnvironmentOverride") {
                None => true,
                Some(Value::Boolean(overrides)) => *overrides,
                _ => return Err(FetchError::Malformed),
            };
            (version, overrides)
        }
    };
    Ok(Some(ParcelNotice { parcel, sequence, version, overrides }))
}

fn parcel_properties_value(value: &Value) -> Result<ParcelPropertiesObservation, ()> {
    let Value::Map(body) = value else { return Err(()) };
    let parcel = required_block(body, "ParcelData")?;
    let request_result = integer(parcel.get("RequestResult")).map_err(|_| ())?.ok_or(())?;
    let sequence_id = integer(parcel.get("SequenceID")).map_err(|_| ())?.ok_or(())?;
    if request_result == -1 || sequence_id < 0 {
        return Ok(ParcelPropertiesObservation { request_result, sequence_id, properties: None });
    }
    if !(0..=1).contains(&request_result) {
        return Err(());
    }
    let local_id = integer(parcel.get("LocalID")).map_err(|_| ())?.and_then(ParcelLocalId::new).ok_or(())?;
    let owner_id = optional_uuid(parcel.get("OwnerID"))?;
    let group_id = optional_uuid(parcel.get("GroupID"))?;
    let group_owned = boolean(parcel.get("IsGroupOwned"))?.ok_or(())?;
    let area = integer(parcel.get("Area")).map_err(|_| ())?.and_then(|area| u32::try_from(area).ok()).ok_or(())?;
    let flags = integer(parcel.get("ParcelFlags")).map_err(|_| ())?.ok_or(())? as u32;
    let bitmap = parcel.get("Bitmap").and_then(Value::as_binary).ok_or(())?;
    let name = parcel_string(parcel.get("Name"), MAX_PARCEL_NAME_BYTES)?;
    let description = parcel_string(parcel.get("Desc"), MAX_PARCEL_DESCRIPTION_BYTES)?;
    let music_url = parcel_string(parcel.get("MusicURL"), MAX_PARCEL_URL_BYTES)?;
    let media_url = parcel_string(parcel.get("MediaURL"), MAX_PARCEL_URL_BYTES)?;
    let user_location = vector3(parcel.get("UserLocation"))?;
    let user_look_at = vector3(parcel.get("UserLookAt"))?;
    let landing_type = optional_u8(parcel.get("LandingType"))?;
    let pass_price = integer(parcel.get("PassPrice")).map_err(|_| ())?.map(u32::try_from).transpose().map_err(|_| ())?;
    let pass_hours = optional_f32(parcel.get("PassHours"))?;
    let age = optional_block(body, "AgeVerificationBlock")?;
    let allow_access = optional_block(body, "RegionAllowAccessBlock")?;

    let properties = ParcelPropertiesData {
        local_id,
        name,
        description,
        owner_id,
        group_id,
        group_owned,
        area,
        flags: ParcelFlags::from_bits(flags),
        bitmap: ParcelBitmap::from_bytes(bitmap).map_err(|_| ())?,
        landing_point: user_location,
        landing_look_at: user_look_at,
        landing_type,
        music_url,
        media_url,
        access: ParcelAccessSettings {
            region_push_override: boolean(parcel.get("RegionPushOverride"))?,
            region_deny_anonymous: boolean(parcel.get("RegionDenyAnonymous"))?,
            region_deny_identified: boolean(parcel.get("RegionDenyIdentified"))?,
            region_deny_transacted: boolean(parcel.get("RegionDenyTransacted"))?,
            region_deny_age_unverified: age.map(|fields| boolean(fields.get("RegionDenyAgeUnverified"))).transpose()?.flatten(),
            region_allow_access_override: allow_access
                .map(|fields| boolean(fields.get("RegionAllowAccessOverride")))
                .transpose()?
                .flatten(),
            pass_price,
            pass_hours,
        },
    };
    Ok(ParcelPropertiesObservation { request_result, sequence_id, properties: Some(properties) })
}

fn required_block<'a>(body: &'a Map, name: &str) -> Result<&'a Map, ()> {
    let Some(Value::Array(values)) = body.get(name) else { return Err(()) };
    let [Value::Map(fields)] = values.as_slice() else { return Err(()) };
    Ok(fields)
}

fn optional_block<'a>(body: &'a Map, name: &str) -> Result<Option<&'a Map>, ()> {
    match body.get(name) {
        None => Ok(None),
        Some(Value::Array(values)) if values.is_empty() => Ok(None),
        Some(Value::Array(values)) => match values.as_slice() {
            [Value::Map(fields)] => Ok(Some(fields)),
            _ => Err(()),
        },
        _ => Err(()),
    }
}

fn boolean(value: Option<&Value>) -> Result<Option<bool>, ()> {
    match value {
        None | Some(Value::Undefined) => Ok(None),
        Some(Value::Boolean(value)) => Ok(Some(*value)),
        _ => Err(()),
    }
}

fn optional_uuid(value: Option<&Value>) -> Result<Option<Uuid>, ()> {
    match value {
        None | Some(Value::Undefined) => Ok(None),
        Some(Value::Uuid(value)) => Ok(Some(*value)),
        _ => Err(()),
    }
}

fn optional_u8(value: Option<&Value>) -> Result<Option<u8>, ()> {
    integer(value).map_err(|_| ())?.map(u8::try_from).transpose().map_err(|_| ())
}

fn optional_f32(value: Option<&Value>) -> Result<Option<f32>, ()> {
    match value {
        None | Some(Value::Undefined) => Ok(None),
        Some(value) => value.as_finite_f32().map(Some).ok_or(()),
    }
}

fn vector3(value: Option<&Value>) -> Result<Option<Vec3>, ()> {
    match value {
        None | Some(Value::Undefined) => Ok(None),
        Some(Value::Array(values)) => {
            let [x, y, z] = values.as_slice() else { return Err(()) };
            Ok(Some(Vec3::new(x.as_finite_f32().ok_or(())?, y.as_finite_f32().ok_or(())?, z.as_finite_f32().ok_or(())?)))
        }
        _ => Err(()),
    }
}

fn parcel_string(value: Option<&Value>, limit: usize) -> Result<Arc<str>, ()> {
    let value = value.and_then(Value::as_str).ok_or(())?;
    if value.len() > limit {
        return Err(());
    }
    Ok(Arc::from(value))
}

fn parcel_message(message: &ParcelProperties) -> Result<Option<ParcelNotice>, FetchError> {
    let parcel = &message.parcel_data;
    let result = parcel.request_result.ok_or(FetchError::Malformed)?;
    if result == -1 {
        return Ok(None);
    }
    if !(0..=1).contains(&result) {
        return Err(FetchError::Malformed);
    }
    let sequence = parcel.sequence_id.ok_or(FetchError::Malformed)?;
    let parcel = parcel.local_id.and_then(ParcelLocalId::new).ok_or(FetchError::Malformed)?;
    let version = message.parcel_environment_block.parcel_environment_version.unwrap_or(-2);
    let overrides = message.parcel_environment_block.region_allow_environment_override.unwrap_or(true);
    Ok(Some(ParcelNotice { parcel, sequence, version, overrides }))
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::simulator_id;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    use url::Url;

    use super::*;
    use crate::session::world::viewer_world_limits;

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
        EnvironmentScope::Parcel(ParcelLocalId::new(42).unwrap())
    }

    fn parcel_properties_event(sequence: i32, local_id: i32, name: &str) -> Value {
        let data = Map::from([
            ("RequestResult".into(), Value::Integer(0)),
            ("SequenceID".into(), Value::Integer(sequence)),
            ("LocalID".into(), Value::Integer(local_id)),
            ("OwnerID".into(), Value::Uuid(Uuid::from_u128(11))),
            ("IsGroupOwned".into(), Value::Boolean(false)),
            ("Bitmap".into(), Value::Binary(vec![0; world::PARCEL_BITMAP_BYTES].into())),
            ("Area".into(), Value::Integer(1024)),
            ("ParcelFlags".into(), Value::Integer((1 << 0) | (1 << 1) | (1 << 5) | (1 << 6))),
            ("Name".into(), Value::String(name.to_owned())),
            ("Desc".into(), Value::String("Parcel description".to_owned())),
            ("MusicURL".into(), Value::String("https://music.invalid/stream".to_owned())),
            ("MediaURL".into(), Value::String("https://media.invalid/page".to_owned())),
            ("GroupID".into(), Value::Uuid(Uuid::from_u128(12))),
            ("PassPrice".into(), Value::Integer(5)),
            ("PassHours".into(), Value::Real(1.5)),
            ("UserLocation".into(), Value::Array(vec![Value::Real(1.0), Value::Real(2.0), Value::Real(3.0)])),
            ("UserLookAt".into(), Value::Array(vec![Value::Real(0.0), Value::Real(1.0), Value::Real(0.0)])),
            ("LandingType".into(), Value::Integer(1)),
            ("RegionPushOverride".into(), Value::Boolean(true)),
            ("RegionDenyAnonymous".into(), Value::Boolean(false)),
            ("RegionDenyIdentified".into(), Value::Boolean(false)),
            ("RegionDenyTransacted".into(), Value::Boolean(false)),
        ]);
        let age = Map::from([("RegionDenyAgeUnverified".into(), Value::Boolean(false))]);
        let access = Map::from([("RegionAllowAccessOverride".into(), Value::Boolean(true))]);
        Value::Map(Map::from([
            ("ParcelData".into(), Value::Array(vec![Value::Map(data)])),
            ("AgeVerificationBlock".into(), Value::Array(vec![Value::Map(age)])),
            ("RegionAllowAccessBlock".into(), Value::Array(vec![Value::Map(access)])),
        ]))
    }

    #[test]
    fn response_preserves_source_and_metadata() {
        let response = decode(RESPONSE.as_bytes(), scope()).unwrap();
        assert_eq!(response.scope, scope());
        assert_eq!(response.region_id, Some(Uuid::from_u128(0x00112233445566778899aabbccddeeff)));
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
            Some(ParcelNotice { parcel: ParcelLocalId::new(42).unwrap(), sequence: 0, version: -1, overrides: false })
        );
        let legacy: Value = notation::from_slice(br#"{'ParcelData':[{'RequestResult':i0,'SequenceID':i1,'LocalID':i0}]}"#).unwrap();
        assert_eq!(
            parcel_value(&legacy).unwrap(),
            Some(ParcelNotice { parcel: ParcelLocalId::new(0).unwrap(), sequence: 1, version: -2, overrides: true })
        );
        let empty: Value = notation::from_slice(br#"{'ParcelData':[{'RequestResult':i-1}]}"#).unwrap();
        assert_eq!(parcel_value(&empty).unwrap(), None);
        let bad: Value = notation::from_slice(br#"{'ParcelData':[{'RequestResult':i0,'SequenceID':i0,'LocalID':i-1}]}"#).unwrap();
        assert_eq!(parcel_value(&bad), Err(FetchError::Malformed));
    }

    #[tokio::test]
    async fn event_queue_parcel_properties_update_current_state_and_reject_stale_sequences() {
        let mut world = SessionWorld::new(viewer_world_limits());
        let simulator = simulator_id();
        let region = world.register_simulator(simulator, None).unwrap();
        let client = Client::new(Url::parse("http://127.0.0.1:1/").unwrap()).unwrap();
        let mut intake = EnvironmentIntake::new(Handle::current());
        intake.routes.insert(region, Route { client, parcel: None, parcel_order: None });

        let first = parcel_properties_event(40, 41, "First parcel");
        intake.capability(&mut world, simulator, "ParcelProperties", &first, Some(simulator));
        let current = world.world().parcel_state(region).unwrap().current.unwrap();
        assert_eq!(current.sequence_id, 40);
        assert_eq!(current.properties.local_id, ParcelLocalId::new(41).unwrap());
        assert_eq!(current.properties.name.as_ref(), "First parcel");
        assert_eq!(current.properties.access.pass_price, Some(5));
        assert_eq!(current.properties.media_url.as_ref(), "https://media.invalid/page");

        let tracked = parcel_properties_value(&parcel_properties_event(41, 42, "UDP parcel")).unwrap();
        let _applied = world.observe_parcel_properties(UpdateSource::new(region, 1, OrderEvidence::Ordered(100), 0), tracked).unwrap();
        let next = parcel_properties_event(42, 43, "Next parcel");
        intake.capability(&mut world, simulator, "ParcelProperties", &next, Some(simulator));
        let stale = parcel_properties_event(41, 42, "Stale parcel");
        intake.capability(&mut world, simulator, "ParcelProperties", &stale, Some(simulator));

        let current = world.world().parcel_state(region).unwrap().current.unwrap();
        assert_eq!(current.sequence_id, 42);
        assert_eq!(current.properties.local_id, ParcelLocalId::new(43).unwrap());
        assert_eq!(current.properties.name.as_ref(), "Next parcel");
        assert_eq!(world.world().environment(region).unwrap().parcel.unwrap().parcel, ParcelLocalId::new(43).unwrap());
    }

    #[tokio::test]
    async fn delayed_sequence_zero_lludp_reply_cannot_overwrite_newer_event_queue_properties() {
        let mut world = SessionWorld::new(viewer_world_limits());
        let simulator = simulator_id();
        let region = world.register_simulator(simulator, None).unwrap();
        let client = Client::new(Url::parse("http://127.0.0.1:1/").unwrap()).unwrap();
        let mut intake = EnvironmentIntake::new(Handle::current());
        intake.routes.insert(region, Route { client, parcel: None, parcel_order: None });

        let initial = parcel_properties_value(&parcel_properties_event(0, 41, "Initial parcel")).unwrap();
        let _applied = world.observe_parcel_properties(UpdateSource::new(region, 1, OrderEvidence::Ordered(100), 0), initial).unwrap();
        let event = parcel_properties_event(42, 43, "EventQueue parcel");
        intake.capability(&mut world, simulator, "ParcelProperties", &event, Some(simulator));

        let delayed = parcel_properties_value(&parcel_properties_event(0, 44, "Delayed UDP parcel")).unwrap();
        let _applied = world.observe_parcel_properties(UpdateSource::new(region, 2, OrderEvidence::Ordered(99), 0), delayed).unwrap();

        let current = world.world().parcel_state(region).unwrap().current.unwrap();
        assert_eq!(current.sequence_id, 42);
        assert_eq!(current.properties.local_id, ParcelLocalId::new(43).unwrap());
        assert_eq!(current.properties.name.as_ref(), "EventQueue parcel");
    }

    #[test]
    fn event_queue_parcel_properties_bound_untrusted_urls() {
        let mut value = parcel_properties_event(1, 2, "Parcel");
        let Value::Map(body) = &mut value else { panic!("event body must be a map") };
        let Some(Value::Array(blocks)) = body.get_mut("ParcelData") else { panic!("parcel data block missing") };
        let Some(Value::Map(data)) = blocks.first_mut() else { panic!("parcel data fields missing") };
        data.insert("MediaURL".into(), Value::String("m".repeat(MAX_PARCEL_URL_BYTES + 1)));

        assert_eq!(parcel_properties_value(&value), Err(()));
    }

    #[tokio::test]
    async fn parcel_transitions_reject_late_results() {
        let mut world = SessionWorld::new(viewer_world_limits());
        let simulator = simulator_id();
        let region = world.register_simulator(simulator, None).unwrap();
        let client = Client::new(Url::parse("http://127.0.0.1:1/").unwrap()).unwrap();
        let mut intake = EnvironmentIntake::new(Handle::current());
        intake.routes.insert(region, Route { client, parcel: None, parcel_order: None });
        let notice = ParcelNotice { parcel: ParcelLocalId::new(42).unwrap(), sequence: 1, version: 7, overrides: true };
        intake.parcel(&mut world, region, ParcelNotice { sequence: -10000, ..notice }, true);
        assert_eq!(world.world().environment(region).unwrap().parcel, None, "selected parcels do not select the avatar parcel");
        intake.parcel(&mut world, region, notice, true);
        let old = intake.queued.remove(&(region, scope())).unwrap();
        intake.order.clear();
        intake.parcel(&mut world, region, ParcelNotice { parcel: ParcelLocalId::new(43).unwrap(), sequence: 2, ..notice }, true);
        intake.parcel(&mut world, region, ParcelNotice { sequence: 3, ..notice }, true);
        intake.queued.clear();
        intake.order.clear();
        intake.complete(&mut world, Completion { request: old, result: decode(RESPONSE.as_bytes(), scope()) });
        assert_eq!(world.world().environment(region).unwrap().parcel.unwrap().assignment, None, "late A results cannot survive A → B → A");
        intake.parcel(&mut world, region, ParcelNotice { sequence: 0, overrides: false, ..notice }, true);
        let parcel = world.world().environment(region).unwrap().parcel.unwrap();
        assert_eq!(parcel.assignment.unwrap().source, EnvironmentSource::Inherit, "disabling parcel overrides clears active settings");
        intake.parcel(&mut world, region, ParcelNotice { sequence: 1, parcel: ParcelLocalId::new(43).unwrap(), ..notice }, true);
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
        intake.parcel(
            &mut world,
            region,
            ParcelNotice { parcel: ParcelLocalId::new(42).unwrap(), sequence: 1, version: 7, overrides: true },
            true,
        );
        let Some(completion) = intake.next_completion().await else { panic!("environment completion missing") };
        intake.complete(&mut world, *completion);
        let observed = world.world().environment(region).unwrap().parcel.unwrap().assignment.unwrap();
        assert_eq!(observed.version, Some(7));
        assert!(matches!(observed.source, EnvironmentSource::Inline(_)));
        assert_eq!(server.await.unwrap().len(), 2);
    }
}
