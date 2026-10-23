mod events;
mod media;
mod persistence;
mod presentation;

pub(crate) use self::persistence::{WriteLock, save_favorite_store, write_lock};
pub(crate) use self::presentation::{
    AuthenticationUpdate, FavoritesUpdate, PresentationMailbox, PresentationSnapshot, SessionReadyUpdate, ViewerUpdate,
};

use std::{
    collections::HashMap,
    future::pending,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use futures::{
    FutureExt,
    channel::{mpsc, oneshot},
    executor::block_on,
    pin_mut,
};
use network::{AgentSession, CoverageLoss, CoverageSource, Delivery, Event, Grid, PacketOrder, SimulatorId};
use protocol::lludp::Message;
use scene::Camera;
use tokio::runtime::Handle;
use tokio::sync::watch;
use world::{LocalAddress, UpdateSource};

use self::events::{
    SimulatorScope, apply_world_event_cooperatively, begin_world_handoff, complete_promoted_handoff, handle_simulator_message,
    order_evidence, route_recovery_targets,
};
use self::media::ObjectMediaIntake;
use crate::{
    asset_coordinator::AssetCoordinator,
    inventory::favorites::load_favorites,
    remembered_login::{self, RememberedLogin},
    session::{
        RegionLookup, Session, SessionHandle, agent,
        environment::EnvironmentIntake,
        experience_environment::{self, EnvironmentCommand, EnvironmentInbox},
        inbound::object_media_requests,
        object_media::{ObjectMediaFetcher, ObjectMediaResult, response_is_stale},
        recovery::{self, RecoveryScheduler},
        terrain::TerrainDecodeQueue,
        world::{IngestStatus, IngestSummary, SessionWorld, viewer_world_limits},
    },
    world_reader::WorldReadHost,
};

const QUERY_STATE_MAINTENANCE_INTERVAL: Duration = Duration::from_millis(25);
const QUERY_STATE_MAINTENANCE_RETRY: Duration = Duration::from_millis(1);
const CAMERA_UPDATE_INTERVAL: Duration = Duration::from_millis(100);
const CAMERA_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

enum DrainInput {
    Cancelled,
    Event(Option<Event>),
    Media(Option<ObjectMediaResult>),
    QueryStateMaintenance,
}

fn next_query_state_maintenance_delay(elapsed: Duration, has_dirty_pressure: bool) -> Duration {
    if has_dirty_pressure {
        return QUERY_STATE_MAINTENANCE_RETRY;
    }
    let elapsed_in_interval = Duration::from_nanos((elapsed.as_nanos() % QUERY_STATE_MAINTENANCE_INTERVAL.as_nanos()) as u64);
    QUERY_STATE_MAINTENANCE_INTERVAL - elapsed_in_interval
}

async fn select_drain_input<E, M, C, T>(event: E, media: M, cancellation: C, maintenance: T) -> DrainInput
where
    E: std::future::Future<Output = Option<Event>>,
    M: std::future::Future<Output = Option<ObjectMediaResult>>,
    C: std::future::Future<Output = ()>,
    T: std::future::Future<Output = ()>,
{
    let event = event.fuse();
    let media = media.fuse();
    let cancellation = cancellation.fuse();
    let maintenance = maintenance.fuse();
    pin_mut!(event, media, cancellation, maintenance);
    futures::select! {
        _ = cancellation => DrainInput::Cancelled,
        event = event => DrainInput::Event(event),
        result = media => DrainInput::Media(result),
        _ = maintenance => DrainInput::QueryStateMaintenance,
    }
}

pub(crate) async fn drain_session(
    mut session: Session,
    session_handle: SessionHandle,
    runtime: Handle,
    mailbox: PresentationMailbox,
    cancellation: oneshot::Receiver<()>,
    grid: Grid,
    username: String,
    password: String,
    remember_login: bool,
    credential_write_lock: WriteLock,
) {
    use Event::{
        CapabilityError, CapabilityEvent, CapabilityEventBatch, CaptureFinished, CaptureInterrupted, CoverageLost, EventQueueCoverageLost,
        Failed, LoginAccepted, MfaRequired, SendFailed, SessionReady, SimulatorCapabilities, SimulatorConnectFailed, SimulatorConnected,
        SimulatorDisconnected, SimulatorFailed, SimulatorMessage, SimulatorPromoted,
    };

    let cancellation = async move {
        match cancellation.await {
            Ok(()) | Err(_) => (),
        }
    }
    .shared();
    let world_limits = viewer_world_limits();
    let mut session_world = SessionWorld::new(world_limits);
    let mut reads = WorldReadHost::new(session_world.world(), runtime.clone());
    let mut terrain = TerrainDecodeQueue::default();
    let mut environments = EnvironmentIntake::new(runtime.clone());
    let (environment_pushes, environment_inbox) = EnvironmentInbox::channel();
    let assets = AssetCoordinator::new(runtime.clone(), cancellation.clone());
    let mut terrain_received = false;
    let mut primary_simulator_id = None;
    let mut world_primary_simulator_id = None;
    let mut promoted_world_target = None;
    let mut handoff_observations = HashMap::new();
    let mut active_session: Option<AgentSession> = None;
    let mut active_login_data = None;
    let (camera, observations) = watch::channel(None::<Camera>);
    let mut last_camera = None;
    let mut camera_sent: Option<Instant> = None;
    let mut region_lookup = None;
    let query_state_maintenance_started = tokio::time::Instant::now().into_std();
    let mut query_state_maintenance = futures::future::ready(()).boxed().fuse();
    let mut media = ObjectMediaIntake::new(ObjectMediaFetcher::new(runtime.clone(), cancellation.clone()));
    let recovery_queue = Arc::new(Mutex::new(RecoveryScheduler::new()));
    let (mut recovery_wake, recovery_wake_receiver) = mpsc::channel(1);
    drop(runtime.clone().spawn(recovery::run_recovery_retries(
        recovery_wake_receiver,
        Arc::clone(&recovery_queue),
        session_handle.clone(),
        cancellation.clone(),
    )));

    loop {
        if cancellation.clone().now_or_never().is_some() {
            break;
        }
        reads.publish(session_world.world());
        media.flush(&session_world);
        let terrain_full = terrain.is_full();
        let event = async { if terrain_full { pending().await } else { session.recv_event().await } };
        let input = select_drain_input(event, media.next_result(), cancellation.clone(), &mut query_state_maintenance);
        let input = match reads.select(terrain.select(environments.select(input))).await {
            Ok(Err(decoded)) => {
                let applied = session_world.apply_terrain(decoded);
                if !terrain_received && applied > 0 {
                    terrain_received = true;
                    tracing::info!(patches = applied, "initial terrain patches received");
                }
                continue;
            }
            Ok(Ok(Ok(input))) => input,
            Ok(Ok(Err(completion))) => {
                if let Some(completion) = completion {
                    environments.complete(&mut session_world, *completion);
                }
                continue;
            }
            Err(request) => {
                request.serve(session_world.world());
                continue;
            }
        };
        let event = match input {
            DrainInput::Cancelled => break,
            DrainInput::QueryStateMaintenance => {
                if let (Some(simulator), Some(session), Some(view)) = (primary_simulator_id, &active_session, *observations.borrow())
                    && let Some(origin) = session_world
                        .region_for(simulator)
                        .and_then(|region| session_world.world().region(region))
                        .and_then(|info| info.resolved_origin())
                {
                    let state = (simulator, origin, view);
                    let interval = if last_camera == Some(state) { CAMERA_HEARTBEAT_INTERVAL } else { CAMERA_UPDATE_INTERVAL };
                    let now = tokio::time::Instant::now().into_std();
                    if camera_sent.is_none_or(|sent| now.saturating_duration_since(sent) >= interval)
                        && let Some(update) = agent::camera(session.agent_id, session.session_id, view, origin)
                    {
                        camera_sent = Some(now);
                        match session_handle.send(update, Delivery::Unreliable) {
                            Ok(()) => {
                                if last_camera.is_none_or(|(previous, _, camera): (SimulatorId, _, Camera)| {
                                    previous != simulator || camera.draw_distance() != view.draw_distance()
                                }) {
                                    tracing::debug!(draw_distance = view.draw_distance(), "simulator viewing distance updated");
                                }
                                last_camera = Some(state);
                            }
                            Err(error) => {
                                last_camera = None;
                                tracing::debug!(%error, "camera update deferred");
                            }
                        }
                    }
                }
                let has_dirty_pressure = session_world.maintain_query_state();
                let elapsed = tokio::time::Instant::now().into_std().duration_since(query_state_maintenance_started);
                query_state_maintenance =
                    tokio::time::sleep(next_query_state_maintenance_delay(elapsed, has_dirty_pressure)).boxed().fuse();
                continue;
            }
            DrainInput::Event(event) => event,
            DrainInput::Media(Some(result)) => {
                let retry = media.finish(&result);
                let requested_version = retry.map_or(result.target.version, |target| target.version);
                let response_version = result.update.as_ref().as_ref().ok().map(|update| update.version());
                if response_version.is_some_and(|version| response_is_stale(version, requested_version)) {
                    tracing::debug!(
                        simulator_id = ?result.target.simulator_id,
                        response_version,
                        requested_version,
                        "discarded superseded ObjectMedia response"
                    );
                } else {
                    match result.update {
                        Ok(update) => {
                            let address = result.target.binding.address();
                            if let Some(applied) = session_world.apply_object_media(result.target, update) {
                                let mut summary = IngestSummary::new(IngestStatus::Applied);
                                match applied {
                                    Ok(applied) => {
                                        if !applied.rejections.is_empty() {
                                            tracing::debug!(rejections = ?applied.rejections, "ObjectMedia update contained rejected fields");
                                        }
                                        summary.push_applied(applied, Some(address));
                                    }
                                    Err(error) => {
                                        tracing::debug!(?error, "ObjectMedia update was rejected by World");
                                        summary.push_error(error, Some(address));
                                    }
                                }
                                summary.log_incomplete_data(result.target.simulator_id);
                            }
                        }
                        Err(error) if session_world.object_media_target_is_current(result.target) => {
                            tracing::warn!(
                                simulator_id = ?result.target.simulator_id,
                                object_id = %result.target.binding.object_id(),
                                local_id = result.target.binding.address().local_id().get(),
                                failure = ?error,
                                "ObjectMedia could not be loaded"
                            );
                        }
                        Err(error) => {
                            tracing::debug!(simulator_id = ?result.target.simulator_id, failure = ?error, "discarded ObjectMedia failure for a stale object target");
                        }
                    }
                }
                if let Some(target) = retry {
                    media.queue(target, &session_world);
                }
                continue;
            }
            DrainInput::Media(None) => {
                if cancellation.clone().now_or_never().is_some() {
                    break;
                }
                tracing::warn!("ObjectMedia worker stopped; media fetching is disabled for this session");
                media.disable();
                continue;
            }
        };
        let Some(event) = event else {
            mailbox.publish(ViewerUpdate::Failed("The network session ended.".into()));
            break;
        };

        if let SimulatorDisconnected { simulator_id, .. } | SimulatorFailed { simulator_id, .. } = &event
            && let Some(region) = session_world.region_for(*simulator_id)
        {
            environments.observe(&event, &mut session_world, primary_simulator_id);
            assets.retire(region);
        }

        if let SimulatorMessage { simulator_id, message, packet_order: PacketOrder::Ordered(_), .. } = &event
            && Some(*simulator_id) == primary_simulator_id
            && let (Some(agent), Some(region)) = (active_session.as_ref(), session_world.region_for(*simulator_id))
        {
            match experience_environment::decode(message, agent.agent_id) {
                Ok(Some(push)) => match environment_pushes.try_send(EnvironmentCommand { region, push }) {
                    Ok(()) => {}
                    Err(error) => tracing::warn!(%error, "experience environment push could not be admitted"),
                },
                Ok(None) => {}
                Err(error) => tracing::warn!(?error, "invalid experience environment push"),
            }
        }

        let media_requests = match &event {
            SimulatorMessage { simulator_id, sequence, packet_order, message, .. } => session_world
                .region_for(*simulator_id)
                .map(|region| object_media_requests(region, *sequence, order_evidence(*packet_order), message))
                .unwrap_or_default(),
            _ => Vec::new(),
        };

        let event = match event {
            CoverageLost { loss } => {
                match loss {
                    CoverageLoss::Source(record) => {
                        let source = match record.watermark.source {
                            CoverageSource::Lludp => "lludp",
                            CoverageSource::EventQueue { .. } => "event_queue",
                        };
                        tracing::warn!(source, reason = ?record.reason, "World data may be incomplete");
                        session.acknowledge_coverage_loss(record.watermark);
                    }
                    CoverageLoss::SessionOverflow { generation } => {
                        tracing::warn!(generation, "World event queue overflowed; World data may be incomplete");
                        session.acknowledge_coverage_overflow(generation);
                    }
                };
                continue;
            }
            event => event,
        };

        if let SimulatorMessage { simulator_id, sequence, packet_order, message, .. } = &event
            && let Some(region) = session_world.region_for(*simulator_id)
        {
            terrain.enqueue(message, UpdateSource::new(region, *sequence, order_evidence(*packet_order), 0), &runtime);
        }

        let mut process_object_chunk = |_simulator_id,
                                        _summary: &IngestSummary,
                                        recovery_targets: Vec<(Option<SimulatorId>, LocalAddress)>,
                                        updated: Vec<LocalAddress>| {
            recovery::schedule_recovery_requests(recovery_targets, updated, active_session.as_ref(), &recovery_queue, &mut recovery_wake);
        };
        if let Some((simulator_id, summary)) = apply_world_event_cooperatively(
            &mut session_world,
            &event,
            &mut world_primary_simulator_id,
            &mut promoted_world_target,
            &mut handoff_observations,
            active_session.as_ref().map(|session: &AgentSession| session.agent_id),
            &mut process_object_chunk,
        )
        .await
        {
            summary.log_incomplete_data(simulator_id);
            recovery::schedule_recovery_requests(
                route_recovery_targets(&session_world, summary.recovery_targets()),
                summary.recovery_cancellation_addresses().collect(),
                active_session.as_ref(),
                &recovery_queue,
                &mut recovery_wake,
            );
        }
        if let SimulatorPromoted { simulator_id, .. } = &event
            && let Some(region) = session_world.region_for(*simulator_id)
        {
            let push = scene::EnvironmentPush::Clear { experience: None, transition: Duration::ZERO };
            if let Err(error) = environment_pushes.try_send(EnvironmentCommand { region, push }) {
                tracing::error!(%error, "experience environment crossing reset could not be admitted");
                mailbox.publish(ViewerUpdate::Failed("The environment update queue is unavailable.".into()));
                break;
            }
        }
        if !matches!(event, SimulatorDisconnected { .. } | SimulatorFailed { .. }) {
            environments.observe(&event, &mut session_world, primary_simulator_id);
        }
        if let SimulatorMessage { simulator_id, .. } = &event {
            for request in media_requests {
                if let Some(target) = session_world.prepare_object_media(*simulator_id, request) {
                    media.queue(target, &session_world);
                }
            }
        }

        if let SimulatorDisconnected { simulator_id, .. } | SimulatorFailed { simulator_id, .. } = &event {
            recovery_queue.lock().unwrap_or_else(PoisonError::into_inner).cancel_simulator(*simulator_id);
        }

        match event {
            MfaRequired { challenge } => {
                mailbox.publish(ViewerUpdate::Authentication(AuthenticationUpdate::MfaRequired(challenge)));
            }
            LoginAccepted { mfa_hash } => {
                let mailbox = mailbox.clone();
                let write_lock = credential_write_lock.clone();
                let username = username.clone();
                let password = password.clone();
                drop(runtime.spawn_blocking(move || {
                    let _guard = block_on(write_lock.lock());
                    let mut warning = None;
                    let remembered_login = if remember_login {
                        let saved = RememberedLogin { username, password, mfa_hash: mfa_hash.unwrap_or_default() };
                        match remembered_login::save(grid, &saved) {
                            Ok(()) => Some(saved),
                            Err(error) => {
                                warning = Some(error);
                                None
                            }
                        }
                    } else {
                        if let Err(error) = remembered_login::clear(grid) {
                            warning = Some(error);
                        }
                        None
                    };
                    mailbox.publish(ViewerUpdate::Authentication(AuthenticationUpdate::CredentialsUpdated { remembered_login, warning }));
                }));
            }
            SessionReady { simulator_id, session, capabilities, login_data } => {
                if let Some(region) = session_world.region_for(simulator_id) {
                    assets.register(region, capabilities.clone());
                }
                let session = *session;
                media.set_client(simulator_id, capabilities.clone());
                primary_simulator_id = Some(simulator_id);
                active_session = Some(session.clone());
                active_login_data = Some(login_data.clone());
                let lookup = RegionLookup::new(session_handle.clone(), simulator_id, &session, runtime.clone());
                region_lookup = Some(lookup.clone());
                mailbox.publish(ViewerUpdate::SessionReady(SessionReadyUpdate {
                    environments: environment_inbox.clone(),
                    camera: camera.clone(),
                    assets: assets.handle(),
                    session: session.clone(),
                    region_lookup: lookup.clone(),
                    region_name: session_world.region_name(simulator_id).map(str::to_owned),
                    world_reader: reads.reader(),
                }));

                let mailbox = mailbox.clone();
                let username = username.clone();
                let runtime = runtime.clone();
                let cancelled = cancellation.clone();
                drop(runtime.spawn(async move {
                    let load = load_favorites(&capabilities, &lookup, &session, &login_data).fuse();
                    pin_mut!(load);
                    let cancelled = cancelled.fuse();
                    pin_mut!(cancelled);
                    futures::select! {
                        result = load => mailbox.publish(ViewerUpdate::Favorites(FavoritesUpdate {
                            grid: session.grid,
                            agent_id: session.agent_id,
                            session_id: session.session_id,
                            username,
                            result,
                        })),
                        _ = cancelled => {}
                    }
                }));
            }
            SimulatorPromoted { previous_simulator_id, simulator_id, session, capabilities } => {
                let session = *session;
                media.set_client(simulator_id, capabilities.clone());
                primary_simulator_id = Some(simulator_id);
                active_session = Some(session.clone());
                let lookup = RegionLookup::new(session_handle.clone(), simulator_id, &session, runtime.clone());
                region_lookup = Some(lookup.clone());
                let previous_target = promoted_world_target.map(|handoff| handoff.target);
                if begin_world_handoff(&mut world_primary_simulator_id, &mut promoted_world_target, previous_simulator_id, simulator_id) {
                    if let Some(previous_target) = previous_target.filter(|target| *target != simulator_id) {
                        handoff_observations.remove(&previous_target);
                    }
                    if let Some(observation) = handoff_observations.get(&simulator_id).cloned() {
                        let (completed, summary) = complete_promoted_handoff(
                            &mut session_world,
                            &mut world_primary_simulator_id,
                            &mut promoted_world_target,
                            simulator_id,
                            observation.clone(),
                        );
                        if completed {
                            handoff_observations.remove(&simulator_id);
                        }
                        if let Some(summary) = summary {
                            recovery::schedule_recovery_requests(
                                route_recovery_targets(&session_world, summary.recovery_targets()),
                                summary.recovery_cancellation_addresses().collect(),
                                active_session.as_ref(),
                                &recovery_queue,
                                &mut recovery_wake,
                            );
                        }
                    }
                }
                let favorites_session = session.clone();
                let favorites_lookup = lookup.clone();
                mailbox.publish(ViewerUpdate::SessionReady(SessionReadyUpdate {
                    environments: environment_inbox.clone(),
                    camera: camera.clone(),
                    assets: assets.handle(),
                    session: session.clone(),
                    region_lookup: lookup,
                    region_name: session_world.region_name(simulator_id).map(str::to_owned),
                    world_reader: reads.reader(),
                }));
                if let Some(login_data) = active_login_data.clone() {
                    let mailbox = mailbox.clone();
                    let username = username.clone();
                    let cancelled = cancellation.clone();
                    drop(runtime.clone().spawn(async move {
                        let load = load_favorites(&capabilities, &favorites_lookup, &favorites_session, &login_data).fuse();
                        pin_mut!(load);
                        let cancelled = cancelled.fuse();
                        pin_mut!(cancelled);
                        futures::select! {
                            result = load => mailbox.publish(ViewerUpdate::Favorites(FavoritesUpdate {
                                grid: favorites_session.grid,
                                agent_id: favorites_session.agent_id,
                                session_id: favorites_session.session_id,
                                username,
                                result,
                            })),
                            _ = cancelled => {}
                        }
                    }));
                }
            }
            SimulatorConnected { simulator_id, region_handle, .. } => {
                if region_handle.is_none() {
                    primary_simulator_id = Some(simulator_id);
                }
            }
            SimulatorMessage { simulator_id, source, message, .. } => {
                if Some(simulator_id) == primary_simulator_id
                    && observations.borrow().is_none()
                    && let Message::AgentMovementComplete(movement) = &message
                    && let Some(session) = &active_session
                    && let Some(update) = agent::arrival(session.agent_id, session.session_id, movement)
                    && let Err(error) = session_handle.send(update, Delivery::Reliable)
                {
                    tracing::debug!(%error, "arrival camera update deferred");
                }
                let scope = if Some(simulator_id) == primary_simulator_id { SimulatorScope::Primary } else { SimulatorScope::Neighbor };
                let region_name = session_world.region_name(simulator_id);
                handle_simulator_message(scope, source, message, active_session.as_ref(), region_lookup.as_ref(), region_name, &mailbox);
            }
            CaptureFinished { .. } => {}
            CaptureInterrupted { error, .. } => {
                tracing::warn!(operation = "capture_stop", outcome = "interrupted", error = ?error, "Capture interrupted")
            }
            SendFailed(error) => tracing::warn!(operation = "simulator_send", outcome = "failed", %error, "Send failed"),
            EventQueueCoverageLost { reason, .. } => tracing::warn!(
                operation = "event_queue",
                outcome = "coverage_lost",
                reason = ?reason,
                "Simulator event delivery may have a gap; world data may be incomplete"
            ),
            Failed(error) => {
                mailbox.publish(ViewerUpdate::Failed(error));
                break;
            }
            SimulatorDisconnected { simulator_id, .. } => {
                media.forget_simulator(simulator_id);
                if Some(simulator_id) == primary_simulator_id {
                    primary_simulator_id = None;
                }
            }
            SimulatorFailed { simulator_id, .. } => {
                media.forget_simulator(simulator_id);
                if Some(simulator_id) == primary_simulator_id {
                    primary_simulator_id = None;
                }
            }
            CapabilityEvent { .. } | CapabilityEventBatch { .. } | CapabilityError { .. } | SimulatorConnectFailed { .. } => {}
            SimulatorCapabilities { simulator_id, capabilities, .. } => {
                if let Some(region) = session_world.region_for(simulator_id) {
                    assets.register(region, capabilities.clone());
                }
                media.set_client(simulator_id, capabilities);
            }
            CoverageLost { .. } => unreachable!("coverage loss events are reconciled before world intake"),
        }
    }

    reads.finish();
    drop(environments);
    drop(terrain);
    assets.stop().await;
    for (_, error) in session_world.retire_all() {
        tracing::warn!(?error, "could not retire simulator world region on session shutdown");
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        future::{pending, ready},
        num::NonZeroU32,
        time::Duration,
    };

    use futures::{FutureExt, StreamExt, stream};
    use math::Vec3;
    use network::{
        Event,
        Event::{SimulatorConnected, SimulatorMessage},
        PacketOrder,
        PacketOrder::Ordered,
        SimulatorId,
        testing::simulator_id,
    };
    use protocol::{
        Audible, ChatSourceType, ChatType,
        lludp::Message,
        lludp::Message::ObjectUpdate,
        messages::{
            ChatFromSimulator,
            chat::chat_from_simulator::ChatData,
            region::region_handshake::{RegionHandshake, RegionInfo1},
        },
    };
    use test_support::lludp::vectors;
    use tokio::runtime::Handle;
    use uuid::Uuid;
    use world::{Kind, ObjectObservation, ParentUpdate, Pose, UpdateSource};

    use super::*;
    use crate::session::coordinator::events::{apply_world_event_cooperatively, apply_world_event_with_handoff};
    use crate::session::coordinator::select_drain_input;
    use crate::session::world::{SessionWorld, viewer_world_limits};

    pub(super) fn region_handshake(name: &str) -> Message {
        Message::RegionHandshake(RegionHandshake {
            region_info1: RegionInfo1 { sim_name: name.into(), ..RegionInfo1::default() },
            region_info2: None,
            region_info3: None,
            region_info4: Vec::new(),
        })
    }

    #[tokio::test]
    async fn floods_preserve_reads_and_intake() {
        use crate::world_reader::{MAX_INTAKE_TURNS, WorldReadHost};
        use futures::poll;
        use std::{collections::VecDeque, time::Instant};

        const EVENTS: u32 = 512;
        let simulator = simulator_id();
        let events = (1..=EVENTS).map(|sequence| agent_update(Uuid::from_u128(u128::from(sequence)), sequence, 0)).collect::<Vec<_>>();
        let mut baseline = SessionWorld::new(viewer_world_limits());
        baseline.register_simulator(simulator, None).unwrap();
        let mut primary = Some(simulator);
        let mut promoted = None;
        let mut observations = HashMap::new();
        let started = Instant::now();
        for (index, message) in events.iter().enumerate() {
            let event = simulator_message(simulator, index as u32 + 1, message.clone());
            apply_world_event_cooperatively(
                &mut baseline,
                &event,
                &mut primary,
                &mut promoted,
                &mut observations,
                None,
                &mut |_, _, _, _| {},
            )
            .await;
        }
        let baseline_time = started.elapsed();

        let mut world = SessionWorld::new(viewer_world_limits());
        world.register_simulator(simulator, None).unwrap();
        let mut reads = WorldReadHost::new(world.world(), Handle::current());
        let reader = reads.reader();
        let mut queued_reads = VecDeque::new();
        for _ in 0..8 {
            let mut read = Box::pin(reader.objects(None, 256));
            assert!(poll!(read.as_mut()).is_pending());
            queued_reads.push_back((0, Instant::now(), read));
        }
        let mut completed = 0;
        let mut processed = 0;
        let mut since_read = 0;
        let mut latencies = Vec::new();
        let started = Instant::now();
        while processed < events.len() {
            reads.publish(world.world());
            let event = simulator_message(simulator, processed as u32 + 1, events[processed].clone());
            let input = super::select_drain_input(ready(Some(event)), pending(), pending(), pending());
            match reads.select(input).await {
                Ok(super::DrainInput::Event(Some(event))) => {
                    apply_world_event_cooperatively(
                        &mut world,
                        &event,
                        &mut primary,
                        &mut promoted,
                        &mut observations,
                        None,
                        &mut |_, _, _, _| {},
                    )
                    .await;
                    processed += 1;
                    since_read += 1;
                    assert!(since_read <= MAX_INTAKE_TURNS, "a ready read must receive its reserved turn");
                }
                Err(request) => {
                    assert_eq!(since_read, MAX_INTAKE_TURNS, "reader floods must not displace ready intake turns");
                    request.serve(world.world());
                    since_read = 0;
                    let (at_event, queued, read) = queued_reads.pop_front().unwrap();
                    let page = read.await.unwrap();
                    latencies.push(queued.elapsed());
                    assert!(processed - at_event <= 8 * MAX_INTAKE_TURNS, "each admitted FIFO request has bounded turn latency");
                    assert_eq!(page.hydration_revision, world.world().revision());
                    assert_eq!(page.objects.len(), processed.min(256));
                    assert_eq!(page.objects[0].id, Uuid::from_u128(1));
                    assert_eq!(page.objects.last().unwrap().id, Uuid::from_u128(processed.min(256) as u128));
                    assert_eq!(reader.revision().unwrap(), page.hydration_revision);
                    completed += 1;
                    let mut read = Box::pin(reader.objects(None, 256));
                    assert!(poll!(read.as_mut()).is_pending());
                    queued_reads.push_back((processed, Instant::now(), read));
                }
                _ => panic!("only simulator intake and reader requests are ready"),
            }
        }
        let flooded_time = started.elapsed();
        assert_eq!(completed, EVENTS as usize / MAX_INTAKE_TURNS - 1);
        assert_eq!(world.world().object_count(), EVENTS as usize);
        assert_eq!(world.world().object_count(), baseline.world().object_count());
        drop(queued_reads);
        reads.finish();
        latencies.sort();
        eprintln!(
            "World intake: {EVENTS} updates, baseline {baseline_time:?}, with {completed} reads {flooded_time:?}; read latency p50 {:?}, p95 {:?}, max {:?}",
            latencies[latencies.len() / 2],
            latencies[latencies.len() * 95 / 100],
            latencies.last().unwrap()
        );
    }

    #[test]
    fn dirty_queries_back_off_repairs() {
        let elapsed = Duration::from_millis(15);
        assert_eq!(super::next_query_state_maintenance_delay(elapsed, false), Duration::from_millis(10));
        assert_eq!(super::next_query_state_maintenance_delay(elapsed, true), Duration::from_millis(1));
    }

    #[tokio::test(start_paused = true)]
    async fn query_state_maintenance_keeps_phase() {
        let maintenance_started = tokio::time::Instant::now().into_std();
        let mut maintenance = ready(()).boxed().fuse();
        assert_eq!(super::next_query_state_maintenance_delay(Duration::ZERO, false), Duration::from_millis(25));

        let result = select_drain_input(std::future::pending(), std::future::pending(), std::future::pending(), &mut maintenance).await;
        assert!(matches!(result, super::DrainInput::QueryStateMaintenance));

        tokio::time::advance(Duration::from_millis(30)).await;
        let delay =
            super::next_query_state_maintenance_delay(tokio::time::Instant::now().into_std().duration_since(maintenance_started), false);
        assert_eq!(delay, Duration::from_millis(20));
        maintenance = tokio::time::sleep(delay).boxed().fuse();
        let simulator_id = simulator_id();
        let mut events = stream::iter([SimulatorConnected {
            simulator_id,
            region_handle: None,
            endpoint: "127.0.0.1:13000".parse().expect("endpoint"),
        }]);
        let result =
            select_drain_input(events.next(), stream::pending::<ObjectMediaResult>().next(), std::future::pending(), &mut maintenance)
                .await;
        assert!(matches!(result, super::DrainInput::Event(Some(_))));

        let result = select_drain_input(std::future::pending(), std::future::pending(), std::future::pending(), &mut maintenance).await;
        assert!(matches!(result, super::DrainInput::QueryStateMaintenance));

        tokio::time::advance(Duration::from_millis(45)).await;
        let delay =
            super::next_query_state_maintenance_delay(tokio::time::Instant::now().into_std().duration_since(maintenance_started), false);
        assert_eq!(delay, Duration::from_millis(5));
        maintenance = tokio::time::sleep(delay).boxed().fuse();
        let result = select_drain_input(std::future::pending(), std::future::pending(), std::future::pending(), &mut maintenance).await;

        assert!(matches!(result, super::DrainInput::QueryStateMaintenance));
    }

    pub(super) fn agent_update(agent_id: Uuid, local_id: u32, region_handle: u64) -> Message {
        let mut message = fixture_packet(vectors::OBJECT_UPDATE).message().clone();
        let ObjectUpdate(update) = &mut message else { panic!("expected full object update") };
        update.region_data.region_handle = region_handle;
        update.objects.truncate(1);
        update.objects[0].id = agent_id;
        update.objects[0].local_id = local_id;
        update.objects[0].pcode = 47;
        update.objects[0].parent_local_id = 0;
        message
    }

    pub(super) fn agent_observation(source: UpdateSource, local_id: u32, agent_id: Uuid, pose: Option<Pose>) -> ObjectObservation {
        ObjectObservation {
            pose,
            ..ObjectObservation::new(
                source,
                agent_id,
                NonZeroU32::new(local_id).expect("nonzero local ID"),
                Kind::Avatar,
                ParentUpdate::Root,
            )
        }
    }

    pub(super) fn simulator_message(simulator_id: SimulatorId, sequence: u32, message: Message) -> Event {
        simulator_message_at_order(simulator_id, sequence, Ordered(u64::from(sequence)), message)
    }

    pub(super) fn simulator_message_at_order(
        simulator_id: SimulatorId,
        sequence: u32,
        packet_order: PacketOrder,
        message: Message,
    ) -> Event {
        SimulatorMessage { simulator_id, source: "127.0.0.1:13000".parse().expect("simulator address"), sequence, packet_order, message }
    }

    pub(super) fn chat_message(from_name: &str) -> Message {
        Message::ChatFromSimulator(ChatFromSimulator {
            chat_data: ChatData {
                from_name: from_name.into(),
                source_id: Uuid::nil(),
                owner_id: Uuid::nil(),
                source_type: ChatSourceType::Agent,
                chat_type: ChatType::Normal,
                audible: Audible::Fully,
                position: Vec3::default(),
                message: "hello".into(),
            },
        })
    }

    pub(super) fn fixture_packet(fixture: &str) -> protocol::lludp::Packet {
        let bytes = test_support::parse_hex_fixture(fixture).expect("valid LLUDP fixture");
        protocol::lludp::Packet::decode(&bytes).expect("decode LLUDP fixture")
    }

    pub(super) fn apply_world_event(world: &mut SessionWorld, event: &Event) -> Option<(SimulatorId, IngestSummary)> {
        let mut world_primary = None;
        let mut promoted_target = None;
        let mut handoff_observations = HashMap::new();
        apply_world_event_with_handoff(world, event, &mut world_primary, &mut promoted_target, &mut handoff_observations, None)
    }
}
