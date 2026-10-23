use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::Arc,
    time::Duration,
};

use futures::{
    FutureExt, StreamExt,
    future::{AbortHandle, Abortable, BoxFuture, pending},
    pin_mut,
    stream::FuturesUnordered,
};
use llsd::{Limits, Map, Value, notation, xml};
use network::{Event, PacketOrder, SimulatorId, capabilities::Client};
use protocol::lludp::{FieldValue, Message, SchemaMessage};
use tokio::{runtime::Handle, sync::Semaphore};
use uuid::Uuid;
use world::{
    EnvironmentAssignment, EnvironmentScope, EnvironmentSource, EnvironmentTarget, MAX_ENVIRONMENT_SOURCE_BYTES, OrderEvidence, ParcelId,
    Region, RegionOutcome, UpdateSource,
};

use super::world::SessionWorld;

#[cfg(test)]
#[path = "environment_tests.rs"]
mod tests;

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
    parcel: ParcelId,
    sequence: i32,
    version: i32,
    overrides: bool,
}

struct Route {
    client: Client,
    parcel: Option<ParcelNotice>,
    parcel_order: Option<u64>,
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
            tracing::error!("environment request sequence exhausted");
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
        let Some(source) = self.source(region) else { return };
        if let Err(error) = world.observe_agent_parcel(source, None) {
            tracing::warn!(?error, "could not clear environment parcel selection");
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

    fn parcel(&mut self, world: &mut SessionWorld, region: Region, notice: ParcelNotice) {
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
            self.clear_parcel(world, region);
        }
        let Some(source) = self.source(region) else { return };
        if let Err(error) = world.observe_agent_parcel(source, Some(notice.parcel)) {
            tracing::warn!(?error, "could not observe environment parcel selection");
            return;
        }
        self.routes.get_mut(&region).expect("registered environment route").parcel = Some(notice);
        if notice.overrides {
            self.queue(world, region, EnvironmentScope::Parcel(notice.parcel));
        } else if let Some(target) = world.world().environment_target(region, EnvironmentScope::Parcel(notice.parcel)) {
            self.cancel_parcel(region);
            self.apply(world, target, source, inherited(None));
        }
    }

    pub(super) fn observe(&mut self, event: &Event, world: &mut SessionWorld, primary: Option<SimulatorId>) {
        match event {
            Event::SessionReady { simulator_id, capabilities, .. }
            | Event::SimulatorPromoted { simulator_id, capabilities, .. }
            | Event::SimulatorCapabilities { simulator_id, capabilities, .. } => {
                let Some(region) = world.region_for(*simulator_id) else { return };
                let initial = !self.routes.contains_key(&region);
                if initial && self.routes.len() >= MAX_REGIONS {
                    return;
                }
                self.routes.entry(region).or_insert_with(|| Route { client: capabilities.clone(), parcel: None, parcel_order: None });
                if let Event::SimulatorPromoted { previous_simulator_id, .. } = event
                    && let Some(previous) = world.region_for(*previous_simulator_id)
                {
                    self.clear_parcel(world, previous);
                }
                if (initial && primary == Some(*simulator_id)) || !matches!(event, Event::SimulatorCapabilities { .. }) {
                    self.queue(world, region, EnvironmentScope::Region);
                }
            }
            Event::SimulatorDisconnected { simulator_id, .. } | Event::SimulatorFailed { simulator_id, .. } => {
                if let Some(region) = world.region_for(*simulator_id) {
                    self.routes.remove(&region);
                    self.queued.retain(|(known, _), _| *known != region);
                    self.order.retain(|key| self.queued.contains_key(key));
                    for ((known, _), cancel) in &self.active {
                        if *known == region {
                            cancel.abort();
                        }
                    }
                }
            }
            Event::CapabilityEvent { simulator_id, message, body, .. } if primary == Some(*simulator_id) => {
                self.capability(world, *simulator_id, message, body);
            }
            Event::CapabilityEventBatch { simulator_id, events, .. } if primary == Some(*simulator_id) => {
                for event in events {
                    self.capability(world, *simulator_id, &event.message, &event.body);
                }
            }
            Event::SimulatorMessage { simulator_id, packet_order, message: Message::Schema(message), .. }
                if primary == Some(*simulator_id) =>
            {
                let Some(region) = world.region_for(*simulator_id) else { return };
                match message.schema().name {
                    "ParcelProperties" => match parcel_schema(message) {
                        Ok(Some(notice)) => {
                            let PacketOrder::Ordered(order) = packet_order else { return };
                            let Some(route) = self.routes.get_mut(&region) else { return };
                            if route.parcel_order.is_some_and(|previous| previous >= *order) {
                                return;
                            }
                            route.parcel_order = Some(*order);
                            self.parcel(world, region, notice);
                        }
                        Ok(None) => {}
                        Err(error) => tracing::debug!(?error, "invalid parcel environment notification"),
                    },
                    "RegionInfo" => self.queue(world, region, EnvironmentScope::Region),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn capability(&mut self, world: &mut SessionWorld, simulator: SimulatorId, message: &str, body: &Value) {
        if message != "ParcelProperties" {
            return;
        }
        let Some(region) = world.region_for(simulator) else { return };
        match parcel_value(body) {
            Ok(Some(notice)) => self.parcel(world, region, notice),
            Ok(None) => {}
            Err(error) => tracing::debug!(?error, "invalid parcel environment notification"),
        }
    }

    pub(super) async fn select<T>(&mut self, input: impl Future<Output = T>) -> Result<T, Option<Box<Completion>>> {
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
        let next =
            async { if self.jobs.is_empty() { pending().await } else { self.jobs.next().await.expect("active environment jobs") } }.fuse();
        let input = input.fuse();
        pin_mut!(next, input);
        futures::select! {
            input = input => Ok(input),
            (key, completion) = next => {
                self.active.remove(&key);
                Err(completion.map(Box::new))
            }
        }
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
                tracing::debug!(scope = ?target.scope(), "environment request unavailable or superseded");
            }
            Err(error) => tracing::warn!(?error, scope = ?target.scope(), "environment could not be loaded"),
        }
    }

    fn apply(&self, world: &mut SessionWorld, target: EnvironmentTarget, source: UpdateSource, assignment: EnvironmentAssignment) {
        match world.observe_environment(target, source, assignment) {
            Ok(applied) if matches!(applied.outcome, RegionOutcome::Patched { .. }) => {
                tracing::info!(region = target.region().get(), scope = ?target.scope(), "environment assignment updated");
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(?error, "environment assignment rejected"),
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
        let response = runtime
            .spawn_blocking(move || {
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
        max_input_bytes: MAX_ENVIRONMENT_SOURCE_BYTES,
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
    let scope =
        if parcel == -1 { EnvironmentScope::Region } else { EnvironmentScope::Parcel(ParcelId::new(parcel).ok_or(FetchError::Malformed)?) };
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
    let parcel = integer(parcel.get("LocalID"))?.and_then(ParcelId::new).ok_or(FetchError::Malformed)?;
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

fn parcel_schema(message: &SchemaMessage) -> Result<Option<ParcelNotice>, FetchError> {
    let mut body = Map::new();
    for (block_index, (schema, block)) in message.schema().blocks.iter().zip(message.blocks()).enumerate() {
        if !matches!(schema.name, "ParcelData" | "ParcelEnvironmentBlock") {
            continue;
        }
        let mut values = Vec::new();
        for (instance_index, instance) in block.instances.iter().enumerate() {
            let mut fields = Map::new();
            for (field_index, (field, value)) in schema.fields.iter().zip(&instance.fields).enumerate() {
                if message.field_was_supplied(block_index, instance_index, field_index) != Some(true) {
                    continue;
                }
                match value {
                    FieldValue::S32(value) => {
                        fields.insert(field.name.into(), Value::Integer(*value));
                    }
                    FieldValue::Bool(value) => {
                        fields.insert(field.name.into(), Value::Boolean(*value));
                    }
                    _ => {}
                }
            }
            values.push(Value::Map(fields));
        }
        body.insert(schema.name.into(), Value::Array(values));
    }
    parcel_value(&Value::Map(body))
}
