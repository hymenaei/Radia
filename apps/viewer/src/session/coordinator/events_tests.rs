use std::{collections::HashMap, num::NonZeroU32};

use math::{Quat, Vec3};
use network::{
    Event::{SimulatorConnected, SimulatorDisconnected},
    PacketOrder::{Ambiguous, Ordered},
};
use protocol::{
    lludp::Message,
    lludp::Message::{ImprovedTerseObjectUpdate, KillObject, ObjectUpdate, ObjectUpdateCached},
    lludp::Packet,
    messages::{CacheMissType::Full, CachedObjectReference, ObjectUpdateCached as CachedUpdate, RequestAgentData, object::kill_object},
};
use test_support::lludp::vectors;
use tokio::runtime::Handle;
use uuid::Uuid;
use world::{
    Applied,
    ApplyOutcome::{Deferred, Introduced, TombstoneRecorded, Unchanged},
    DeferredReason::AmbiguousOrder,
    Kind, LocalAddress, ObjectObservation, Parent, ParentUpdate, Pose, Region, RegionHandle, UpdateSource,
};

use crate::session::coordinator::events::{MovementCompleteOutcome, PendingWorldHandoff, WorldHandoff};
use crate::session::coordinator::tests::apply_world_event;
use crate::session::coordinator::tests::{
    agent_observation, agent_update, fixture_packet, region_handshake, simulator_message, simulator_message_at_order,
};
use crate::session::inbound::{ObjectBatch, ObjectMediaRequest, ObjectOperation};
use crate::session::{
    object_cache::{ObjectCacheEvent, ObjectCacheSession},
    recovery::recovery_request,
    world::{IngestStatus, SessionWorld, viewer_world_limits},
};
use crate::test_helpers::simulator_id;

fn apply_world_event_with_handoff(
    world: &mut SessionWorld,
    event: &network::Event,
    world_primary: &mut Option<network::SimulatorId>,
    promoted_target: &mut Option<PendingWorldHandoff>,
    observations: &mut HashMap<network::SimulatorId, ObjectObservation>,
    agent_id: Option<Uuid>,
) -> Option<(network::SimulatorId, super::IngestSummary)> {
    let mut handoff = WorldHandoff {
        world_primary: *world_primary,
        promoted_target: *promoted_target,
        observations: std::mem::take(observations),
        pending_movement_completes: HashMap::new(),
    };
    let result = handoff.apply_event(world, event, agent_id);
    *world_primary = handoff.world_primary;
    *promoted_target = handoff.promoted_target;
    *observations = handoff.observations;
    result
}

fn begin_world_handoff(
    world_primary: &mut Option<network::SimulatorId>,
    promoted_target: &mut Option<PendingWorldHandoff>,
    source: network::SimulatorId,
    target: network::SimulatorId,
) -> bool {
    let mut handoff = WorldHandoff {
        world_primary: *world_primary,
        promoted_target: *promoted_target,
        observations: HashMap::new(),
        pending_movement_completes: HashMap::new(),
    };
    let result = handoff.begin(source, target);
    *world_primary = handoff.world_primary;
    *promoted_target = handoff.promoted_target;
    result
}

fn assert_public_world_invariants(world: &SessionWorld, region: Region) {
    let state = world.world();
    let mut object_views = state.objects();
    let objects = object_views.iter().collect::<Vec<_>>();
    assert_eq!(objects.len(), state.object_count());
    for object in objects {
        let entity = state.entity_at(object.address()).expect("live object address remains indexed");
        assert_eq!(state.object(entity).map(|indexed| indexed.id()), Some(object.id()));
        if let Parent::Resolved(parent) = object.parent() {
            assert!(state.object(parent).is_some(), "resolved parent remains live");
        }
    }
    assert_eq!(state.objects_in_region(region).count(), state.object_count());
}

async fn apply_with_cache(
    world: &mut SessionWorld,
    handoff: &mut WorldHandoff,
    cache: &mut ObjectCacheSession,
    simulator: network::SimulatorId,
    sequence: u32,
    message: &Message,
) -> (Vec<(Option<network::SimulatorId>, LocalAddress, protocol::messages::CacheMissType)>, Vec<ObjectMediaRequest>) {
    let mut recovery = Vec::new();
    let mut media_requests = Vec::new();
    let _ = handoff
        .apply_message_cooperatively(
            world,
            simulator,
            sequence,
            Ordered(u64::from(sequence)),
            std::time::Instant::now(),
            None,
            message,
            None,
            cache,
            &mut |targets, _| recovery.extend(targets),
            &mut media_requests,
        )
        .await;
    (recovery, media_requests)
}

async fn load_object_cache(cache: &mut ObjectCacheSession, simulator: network::SimulatorId) {
    await_cache_write(cache).await;
    if cache.begin_region_load(simulator, &Handle::current()).is_some() {
        return;
    }
    loop {
        let event = cache.next_event().await.expect("region cache load completes");
        if cache.handle_event(event).is_some() {
            return;
        }
    }
}

async fn await_cache_write(cache: &mut ObjectCacheSession) {
    loop {
        let event = cache.next_event().await.expect("cache worker returns the write result");
        let is_write = matches!(&event, ObjectCacheEvent::WriteApplied { .. } | ObjectCacheEvent::WriteDropped { .. });
        let _ = cache.handle_event(event);
        if is_write {
            return;
        }
    }
}

#[tokio::test]
async fn interleaved_cached_full_and_terse_updates_preserve_world_indexes() {
    let directory = tempfile::tempdir().expect("create isolated cache directory");
    let simulator = simulator_id();
    let mut cache = crate::session::object_cache::open_for_test(directory.path().join("objects.sqlite"), network::Grid::Agni);
    let Message::RegionHandshake(mut handshake) = fixture_packet(vectors::REGION_HANDSHAKE).into_message() else {
        panic!("region handshake fixture should decode as a handshake");
    };
    handshake.region_info1.cache_id = Uuid::from_u128(0x9876);
    let Some(info) = &mut handshake.region_info2 else { panic!("test handshake includes region identity") };
    info.region_id = Uuid::from_u128(0x5432);
    cache.set_region_identity(simulator, &handshake);

    let region_handle = 0x2222_0000_0000_0002;
    let mut world = SessionWorld::new(viewer_world_limits());
    world.register_simulator(simulator, Some(RegionHandle::new(region_handle))).expect("register region");
    let region = world.region_for(simulator).expect("region exists");
    let mut handoff = WorldHandoff::default();
    handoff.session_ready(&mut world, simulator, Uuid::nil());
    let mut full = fixture_packet(vectors::OBJECT_UPDATE).into_message();
    let ObjectUpdate(update) = &mut full else { panic!("full object fixture should decode as an update") };
    update.region_data.region_handle = region_handle;
    update.objects[1].parent_local_id = update.objects[0].local_id;
    apply_with_cache(&mut world, &mut handoff, &mut cache, simulator, 1, &full).await;
    cache.record_update(simulator, full.clone());
    load_object_cache(&mut cache, simulator).await;
    assert_public_world_invariants(&world, region);

    let make_cached = |message: &Message| {
        let Message::ObjectUpdate(update) = message else { panic!("cached source is a full update") };
        Message::ObjectUpdateCached(CachedUpdate {
            region_data: update.region_data,
            objects: update
                .objects
                .iter()
                .map(|object| CachedObjectReference { local_id: object.local_id, crc: object.crc, update_flags: object.update_flags })
                .collect(),
        })
    };
    let cached = make_cached(&full);
    let before_live_hit = world.world().revision();
    assert!(apply_with_cache(&mut world, &mut handoff, &mut cache, simulator, 2, &cached).await.0.is_empty());
    assert_eq!(world.world().revision(), before_live_hit);
    assert_public_world_invariants(&world, region);

    let parent_local_id = match &full {
        Message::ObjectUpdate(update) => update.objects[0].local_id,
        _ => unreachable!(),
    };
    let parent_id = match &full {
        Message::ObjectUpdate(update) => update.objects[0].id,
        _ => unreachable!(),
    };
    let mut terse = fixture_packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE).into_message();
    let ImprovedTerseObjectUpdate(update) = &mut terse else { panic!("terse fixture should decode as an update") };
    update.region_data.region_handle = region_handle;
    let object = update.objects[0].as_decoded_mut().expect("terse fixture object is valid");
    object.local_id = parent_local_id;
    object.motion.position = Vec3::new(31.0, 42.0, 53.0);
    object.motion.rotation = Quat::IDENTITY;
    apply_with_cache(&mut world, &mut handoff, &mut cache, simulator, 3, &terse).await;
    cache.record_update(simulator, terse.clone());
    assert_public_world_invariants(&world, region);

    let before_probe = world.world().revision();
    let (recovery, _) = apply_with_cache(&mut world, &mut handoff, &mut cache, simulator, 4, &make_cached(&full)).await;
    assert!(recovery.is_empty());
    assert_eq!(world.world().revision(), before_probe);
    let parent = world.world().object(world.world().entity(parent_id).expect("parent object remains live")).expect("parent object exists");
    assert_eq!(parent.transform().position(), Some(Vec3::new(31.0, 42.0, 53.0)));

    let Message::ObjectUpdate(update) = &mut full else { unreachable!() };
    update.objects[0].crc = update.objects[0].crc.wrapping_add(1);
    apply_with_cache(&mut world, &mut handoff, &mut cache, simulator, 5, &full).await;
    cache.record_update(simulator, full.clone());
    await_cache_write(&mut cache).await;
    assert_public_world_invariants(&world, region);

    let cached = make_cached(&full);
    assert!(apply_with_cache(&mut world, &mut handoff, &mut cache, simulator, 6, &cached).await.0.is_empty());
    assert_public_world_invariants(&world, region);

    let child_local_id = match &full {
        Message::ObjectUpdate(update) => update.objects[1].local_id,
        _ => unreachable!(),
    };
    let mut terse = fixture_packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE).into_message();
    let ImprovedTerseObjectUpdate(update) = &mut terse else { unreachable!() };
    update.region_data.region_handle = region_handle;
    let object = update.objects[0].as_decoded_mut().expect("terse fixture object is valid");
    object.local_id = child_local_id;
    object.motion.position = Vec3::new(-5.0, 7.0, 11.0);
    object.motion.rotation = Quat::IDENTITY;
    apply_with_cache(&mut world, &mut handoff, &mut cache, simulator, 7, &terse).await;
    cache.record_update(simulator, terse);
    assert_public_world_invariants(&world, region);
    cache.close().await;
}

#[tokio::test]
async fn recovered_cached_object_media_reaches_session_handoff() {
    let directory = tempfile::tempdir().expect("create isolated cache directory");
    let simulator = simulator_id();
    let mut cache = crate::session::object_cache::open_for_test(directory.path().join("objects.sqlite"), network::Grid::Agni);
    let Message::RegionHandshake(mut handshake) = fixture_packet(vectors::REGION_HANDSHAKE).into_message() else {
        panic!("region handshake fixture should decode as a handshake");
    };
    handshake.region_info1.cache_id = Uuid::from_u128(0x9876);
    let Some(info) = &mut handshake.region_info2 else { panic!("test handshake includes region identity") };
    info.region_id = Uuid::from_u128(0x5432);
    cache.set_region_identity(simulator, &handshake);

    let region_handle = 0x2222_0000_0000_0002;
    let mut world = SessionWorld::new(viewer_world_limits());
    world.register_simulator(simulator, Some(RegionHandle::new(region_handle))).expect("register region");
    let region = world.region_for(simulator).expect("region exists");
    let mut handoff = WorldHandoff::default();
    handoff.session_ready(&mut world, simulator, Uuid::nil());

    let mut full = fixture_packet(vectors::OBJECT_UPDATE).into_message();
    let ObjectUpdate(update) = &mut full else { panic!("full object fixture should decode as an update") };
    update.region_data.region_handle = region_handle;
    update.objects.truncate(1);
    update.objects[0].parent_local_id = 0;
    let object_id = Uuid::from_u128(0xabcde);
    update.objects[0].pcode = 9;
    update.objects[0].id = object_id;
    update.objects[0].media_url = "x-mv:12/00000000-0000-0000-0000-000000000001".into();
    cache.record_update(simulator, full.clone());
    load_object_cache(&mut cache, simulator).await;

    let ObjectUpdate(update) = &full else { unreachable!() };
    let cached = Message::ObjectUpdateCached(CachedUpdate {
        region_data: update.region_data,
        objects: update
            .objects
            .iter()
            .map(|object| CachedObjectReference { local_id: object.local_id, crc: object.crc, update_flags: object.update_flags })
            .collect(),
    });
    let (recovery, media_requests) = apply_with_cache(&mut world, &mut handoff, &mut cache, simulator, 2, &cached).await;

    assert!(recovery.is_empty());
    assert_eq!(media_requests.len(), 1);
    assert_eq!(media_requests[0].object_id, object_id);
    assert_eq!(media_requests[0].version, 12);
    assert_public_world_invariants(&world, region);
    cache.close().await;
}

fn complete_promoted_handoff(
    world: &mut SessionWorld,
    world_primary: &mut Option<network::SimulatorId>,
    promoted_target: &mut Option<PendingWorldHandoff>,
    target: network::SimulatorId,
    observation: ObjectObservation,
) -> (bool, Option<super::IngestSummary>) {
    let mut handoff = WorldHandoff {
        world_primary: *world_primary,
        promoted_target: *promoted_target,
        observations: HashMap::new(),
        pending_movement_completes: HashMap::new(),
    };
    let result = handoff.complete_promoted(world, target, observation);
    *world_primary = handoff.world_primary;
    *promoted_target = handoff.promoted_target;
    result
}

fn complete_promoted_world_handoff(
    world: &mut SessionWorld,
    world_primary: &mut Option<network::SimulatorId>,
    promoted_target: &mut Option<PendingWorldHandoff>,
    target: network::SimulatorId,
    observation: ObjectObservation,
) -> (bool, Option<Applied>) {
    let mut handoff = WorldHandoff {
        world_primary: *world_primary,
        promoted_target: *promoted_target,
        observations: HashMap::new(),
        pending_movement_completes: HashMap::new(),
    };
    let result = handoff.complete_world_handoff(world, target, observation);
    *world_primary = handoff.world_primary;
    *promoted_target = handoff.promoted_target;
    result
}

#[test]
fn region_connection_lifecycle() {
    let simulator_id = simulator_id();
    let handle = 0x1111_0000_0000_0001;
    let endpoint = "127.0.0.1:13000".parse().expect("endpoint");
    let mut intake = SessionWorld::new(viewer_world_limits());

    apply_world_event(&mut intake, &SimulatorConnected { simulator_id, region_handle: Some(handle), endpoint });
    let region = intake.region_for(simulator_id).expect("registered region");
    assert_eq!(region, Region::new(simulator_id.get()));
    assert_eq!(intake.world().region(region).unwrap().handle, Some(RegionHandle::new(handle)));

    apply_world_event(&mut intake, &SimulatorDisconnected { simulator_id, region_handle: Some(handle), endpoint });
    assert!(intake.region_for(simulator_id).is_none());
    assert!(intake.world().region(region).is_none());
    assert!(apply_world_event(&mut intake, &simulator_message(simulator_id, 1, region_handshake("late"))).is_none());
}

#[tokio::test]
async fn seat_removal_preserves_parenting() {
    let simulator_id = simulator_id();
    let simulator = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("bind loopback simulator");
    let viewer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("bind loopback viewer");
    let seat_id = Uuid::from_u128(0x51);
    let agent_id = Uuid::from_u128(0x52);
    let seat_local_id = NonZeroU32::new(10).expect("seat local id");
    let agent_local_id = NonZeroU32::new(11).expect("agent local id");
    let mut intake = SessionWorld::new(viewer_world_limits());
    let region = intake.register_simulator(simulator_id, None).expect("register simulator");
    intake.set_self_agent(simulator_id, agent_id);
    let seat = ObjectObservation::new(
        UpdateSource::new(region, 1, world::OrderEvidence::Ordered(1), 0),
        seat_id,
        seat_local_id,
        Kind::Object,
        ParentUpdate::Root,
    );
    let mut agent = ObjectObservation::new(
        UpdateSource::new(region, 2, world::OrderEvidence::Ordered(2), 0),
        agent_id,
        agent_local_id,
        Kind::Avatar,
        ParentUpdate::Local(seat_local_id),
    );
    agent.pose = Some(Pose { position: Some(Vec3::new(2.0, 3.0, 4.0)), rotation: Some(Quat::IDENTITY) });
    intake
        .apply_object_batch(
            simulator_id,
            ObjectBatch {
                region,
                region_handle: None,
                blocks: vec![Some(ObjectOperation::Observe(seat)), Some(ObjectOperation::Observe(agent))],
                invalid_blocks: 0,
                invalid_fields: 0,
            },
        )
        .expect("apply seat and avatar");
    let seat_entity = intake.world().entity(seat_id).expect("seat entity");
    let agent_entity = intake.world().entity(agent_id).expect("avatar entity");
    assert_eq!(intake.world().object(agent_entity).expect("avatar").parent(), Parent::Resolved(seat_entity));

    let removal =
        Message::KillObject(kill_object::KillObject { object_data: vec![kill_object::ObjectData { local_id: seat_local_id.get() }] });
    let bytes = Packet::new(3, removal).encode().expect("encode seat removal");
    simulator.send_to(&bytes, viewer.local_addr().expect("viewer address")).await.expect("send seat removal");
    let mut bytes = [0; 2048];
    let (length, _) = viewer.recv_from(&mut bytes).await.expect("receive seat removal");
    let message = Packet::decode(&bytes[..length]).expect("decode seat removal").message().clone();
    std::assert_matches!(message, Message::KillObject(_));
    apply_world_event(&mut intake, &simulator_message(simulator_id, 3, message)).expect("apply simulator seat removal");

    let parent = intake.world().object(agent_entity).expect("avatar remains in world").parent();
    std::assert_matches!(parent, Parent::Pending { expected_uuid: Some(id), .. } if id == seat_id);
}

#[test]
fn authenticated_primary_movement_complete_resolves_region_origin() {
    let primary_simulator = simulator_id();
    let other_simulator = simulator_id();
    let packet = fixture_packet(vectors::AGENT_MOVEMENT_COMPLETE);
    let Message::AgentMovementComplete(movement) = packet.message().clone() else { panic!("movement-complete fixture") };
    let agent_id = movement.agent_data.agent_id;
    let session_id = movement.agent_data.session_id;
    let handle = RegionHandle::new(movement.data.region_handle);
    let mut intake = SessionWorld::new(viewer_world_limits());
    let region = intake.register_simulator(primary_simulator, None).expect("register simulator without an origin");
    let mut handoff = WorldHandoff::default();
    handoff.session_ready(&mut intake, primary_simulator, agent_id);

    assert_eq!(intake.world().region(region).and_then(|info| info.resolved_origin()), None);
    let received_at = std::time::Instant::now();
    assert_eq!(
        handoff.movement_complete(&mut intake, primary_simulator, Some((agent_id, Uuid::nil())), &movement, received_at),
        MovementCompleteOutcome::Rejected
    );
    assert_eq!(
        handoff.movement_complete(&mut intake, other_simulator, Some((agent_id, session_id)), &movement, received_at),
        MovementCompleteOutcome::Rejected
    );
    assert_eq!(intake.world().region(region).and_then(|info| info.resolved_origin()), None);
    assert_eq!(
        handoff.movement_complete(&mut intake, primary_simulator, Some((agent_id, session_id)), &movement, received_at),
        MovementCompleteOutcome::Applied
    );
    assert_eq!(intake.world().region(region).and_then(|info| info.resolved_origin()), Some(handle.origin()));
}

#[test]
fn destination_movement_complete_waits_for_promotion() {
    let packet = fixture_packet(vectors::AGENT_MOVEMENT_COMPLETE);
    let Message::AgentMovementComplete(mut movement) = packet.message().clone() else { panic!("movement-complete fixture") };
    let agent_id = movement.agent_data.agent_id;
    let session_id = movement.agent_data.session_id;
    let source = simulator_id();
    let destination = simulator_id();
    let source_handle = RegionHandle::new(0x1111_0000_0000_0001);
    let destination_handle = RegionHandle::new(0x2222_0000_0000_0002);
    movement.data.region_handle = destination_handle.get();
    let mut world = SessionWorld::new(viewer_world_limits());
    world.register_simulator(source, Some(source_handle)).expect("source region");
    let destination_region = world.register_simulator(destination, None).expect("destination region");
    let mut handoff = WorldHandoff::default();
    handoff.session_ready(&mut world, source, agent_id);
    let received_at = std::time::Instant::now();

    assert_eq!(
        handoff.movement_complete(&mut world, destination, Some((agent_id, session_id)), &movement, received_at),
        MovementCompleteOutcome::Held
    );
    assert_eq!(world.world().region(destination_region).and_then(|info| info.resolved_origin()), None);

    let _ = handoff.promote(&mut world, source, destination);
    let (_, applied_at) = handoff
        .apply_promoted_movement_complete(&mut world, destination, Some((agent_id, session_id)))
        .expect("held movement completion applies after promotion");
    assert_eq!(applied_at, received_at);
    assert_eq!(world.world().region(destination_region).and_then(|info| info.resolved_origin()), Some(destination_handle.origin()));
}

#[test]
fn agent_waits_for_primary_promotion() {
    let agent_id = Uuid::from_u128(0x1234);
    let source_id = simulator_id();
    let destination_id = simulator_id();
    let source_handle = RegionHandle::new(0x1111_0000_0000_0001);
    let destination_handle = RegionHandle::new(0x2222_0000_0000_0002);
    let mut intake = SessionWorld::new(viewer_world_limits());
    intake.set_self_agent(source_id, agent_id);
    let source_region = intake.register_simulator(source_id, Some(source_handle)).expect("register source region");
    let destination_region = intake.register_simulator(destination_id, Some(destination_handle)).expect("register destination region");
    let mut world_primary = Some(source_id);
    let mut promoted_target = None;
    let mut observations = HashMap::new();

    let source = agent_update(agent_id, 10, source_handle.get());
    apply_world_event_with_handoff(
        &mut intake,
        &simulator_message(source_id, 1, source),
        &mut world_primary,
        &mut promoted_target,
        &mut observations,
        Some(agent_id),
    )
    .expect("apply source agent observation");

    let destination = agent_update(agent_id, 20, destination_handle.get());
    apply_world_event_with_handoff(
        &mut intake,
        &simulator_message_at_order(destination_id, 2, Ordered(20), destination),
        &mut world_primary,
        &mut promoted_target,
        &mut observations,
        Some(agent_id),
    )
    .expect("ingest destination packet");

    apply_world_event_with_handoff(
        &mut intake,
        &simulator_message_at_order(destination_id, 3, Ordered(19), agent_update(agent_id, 21, destination_handle.get())),
        &mut world_primary,
        &mut promoted_target,
        &mut observations,
        Some(agent_id),
    )
    .expect("ingest late destination packet");
    apply_world_event_with_handoff(
        &mut intake,
        &simulator_message_at_order(destination_id, 4, Ambiguous, agent_update(agent_id, 22, destination_handle.get())),
        &mut world_primary,
        &mut promoted_target,
        &mut observations,
        Some(agent_id),
    )
    .expect("ingest ambiguous destination packet");
    assert_eq!(observations.get(&destination_id).unwrap().source.order(), ::world::OrderEvidence::Ordered(20));

    let state = intake.world();
    let agent = state.object(state.entity(agent_id).expect("source agent remains present")).expect("agent object");
    assert_eq!(agent.address().region(), source_region);
    assert!(observations.contains_key(&destination_id));
    assert!(begin_world_handoff(&mut world_primary, &mut promoted_target, source_id, destination_id));
    let destination = agent_update(agent_id, 20, destination_handle.get());
    apply_world_event_with_handoff(
        &mut intake,
        &simulator_message(destination_id, 3, destination),
        &mut world_primary,
        &mut promoted_target,
        &mut observations,
        Some(agent_id),
    )
    .expect("complete explicit destination handoff");

    let state = intake.world();
    let agent = state.object(state.entity(agent_id).expect("agent survives handoff")).expect("agent object");
    assert_eq!(agent.address().region(), destination_region);
    assert_eq!(world_primary, Some(destination_id));
    assert_eq!(promoted_target, None);
    assert!(!observations.contains_key(&destination_id));
    apply_world_event_with_handoff(
        &mut intake,
        &SimulatorDisconnected {
            simulator_id: source_id,
            region_handle: Some(source_handle.get()),
            endpoint: "127.0.0.1:13000".parse().expect("simulator address"),
        },
        &mut world_primary,
        &mut promoted_target,
        &mut observations,
        Some(agent_id),
    );
    assert_eq!(world_primary, Some(destination_id));
    assert!(intake.region_for(destination_id).is_some());
    let state = intake.world();
    let agent = state.object(state.entity(agent_id).expect("promoted agent remains present")).expect("agent object");
    assert_eq!(agent.address().region(), destination_region);
}

#[test]
fn promoted_handoff_preserves_field_rejections() {
    let agent_id = Uuid::from_u128(0x1234);
    let source_id = simulator_id();
    let destination_id = simulator_id();
    let mut intake = SessionWorld::new(viewer_world_limits());
    intake.set_self_agent(source_id, agent_id);
    let source_region = intake.register_simulator(source_id, None).expect("register source region");
    let destination_region = intake.register_simulator(destination_id, None).expect("register destination region");
    let source_observation =
        agent_observation(UpdateSource::new(source_region, 1, ::world::OrderEvidence::Ordered(1), 0), 10, agent_id, None);
    intake
        .apply_object_batch(
            source_id,
            ObjectBatch {
                region: source_region,
                region_handle: None,
                blocks: vec![Some(ObjectOperation::Observe(source_observation))],
                invalid_blocks: 0,
                invalid_fields: 0,
            },
        )
        .expect("observe source avatar");

    let mut world_primary = Some(source_id);
    let mut pending = None;
    assert!(begin_world_handoff(&mut world_primary, &mut pending, source_id, destination_id));
    let observation = agent_observation(
        UpdateSource::new(destination_region, 2, ::world::OrderEvidence::Ordered(2), 0),
        20,
        agent_id,
        Some(Pose { position: None, rotation: Some(Quat::from_xyzw(f32::NAN, 0.0, 0.0, 1.0)) }),
    );

    let (completed, summary) = complete_promoted_handoff(&mut intake, &mut world_primary, &mut pending, destination_id, observation);

    assert!(completed);
    let summary = summary.expect("promoted handoff produces an ingest summary");
    let applied = summary.blocks[0].applied().expect("handoff applies");
    assert!(applied.rejections.iter().any(|rejection| matches!(rejection.reason, ::world::FieldRejectionReason::InvalidRotation)));
}

#[test]
fn newer_handoff_conflicts_invalidate_observation() {
    let agent_id = Uuid::from_u128(0x1234);
    let replacement_id = Uuid::from_u128(0x5678);
    let source_id = simulator_id();
    let destination_id = simulator_id();
    let source_handle = RegionHandle::new(0x1111_0000_0000_0001);
    let destination_handle = RegionHandle::new(0x2222_0000_0000_0002);
    let mut intake = SessionWorld::new(viewer_world_limits());
    intake.set_self_agent(source_id, agent_id);
    let source_region = intake.register_simulator(source_id, Some(source_handle)).expect("register source");
    let destination_region = intake.register_simulator(destination_id, Some(destination_handle)).expect("register destination");
    let mut world_primary = Some(source_id);
    let mut promoted_target = None;
    let mut observations = HashMap::new();

    apply_world_event_with_handoff(
        &mut intake,
        &simulator_message_at_order(source_id, 1, Ordered(1), agent_update(agent_id, 10, source_handle.get())),
        &mut world_primary,
        &mut promoted_target,
        &mut observations,
        Some(agent_id),
    )
    .expect("apply source agent");
    apply_world_event_with_handoff(
        &mut intake,
        &simulator_message_at_order(destination_id, 2, Ordered(20), agent_update(agent_id, 20, destination_handle.get())),
        &mut world_primary,
        &mut promoted_target,
        &mut observations,
        Some(agent_id),
    )
    .expect("buffer destination agent");

    let removed_address = LocalAddress::new(destination_region, NonZeroU32::new(20).expect("nonzero local ID"));
    let removal = Message::KillObject(kill_object::KillObject { object_data: vec![kill_object::ObjectData { local_id: 20 }] });
    let (_, summary) = apply_world_event_with_handoff(
        &mut intake,
        &simulator_message_at_order(destination_id, 3, Ordered(19), removal.clone()),
        &mut world_primary,
        &mut promoted_target,
        &mut observations,
        Some(agent_id),
    )
    .expect("apply stale kill");
    let result = summary.blocks[0].applied().expect("recorded kill");
    std::assert_matches!(&result.outcome, TombstoneRecorded(address) if *address == removed_address);
    assert!(observations.contains_key(&destination_id));

    apply_world_event_with_handoff(
        &mut intake,
        &simulator_message_at_order(destination_id, 4, Ordered(21), removal),
        &mut world_primary,
        &mut promoted_target,
        &mut observations,
        Some(agent_id),
    )
    .expect("apply newer kill");
    assert!(!observations.contains_key(&destination_id));

    apply_world_event_with_handoff(
        &mut intake,
        &simulator_message_at_order(destination_id, 5, Ordered(22), agent_update(agent_id, 21, destination_handle.get())),
        &mut world_primary,
        &mut promoted_target,
        &mut observations,
        Some(agent_id),
    )
    .expect("buffer replacement-address agent");
    let replacement = agent_update(replacement_id, 21, destination_handle.get());
    let (_, summary) = apply_world_event_with_handoff(
        &mut intake,
        &simulator_message_at_order(destination_id, 6, Ordered(23), replacement),
        &mut world_primary,
        &mut promoted_target,
        &mut observations,
        Some(agent_id),
    )
    .expect("apply newer replacement");
    assert!(summary.blocks[0].applied().is_some());
    assert!(!observations.contains_key(&destination_id));

    assert!(begin_world_handoff(&mut world_primary, &mut promoted_target, source_id, destination_id));
    assert_eq!(world_primary, Some(source_id));
    assert_eq!(promoted_target, Some(PendingWorldHandoff { source: source_id, target: destination_id }));
    let state = intake.world();
    let agent = state.object(state.entity(agent_id).expect("source agent remains present")).expect("agent object");
    assert_eq!(agent.address().region(), source_region);
    assert_eq!(
        state.object(state.entity(replacement_id).expect("replacement was applied")).unwrap().address(),
        LocalAddress::new(destination_region, NonZeroU32::new(21).unwrap())
    );
}

#[test]
fn deferred_handoff_keeps_source_primary() {
    let agent_id = Uuid::from_u128(0x1234);
    let source_id = simulator_id();
    let destination_id = simulator_id();
    let source_handle = RegionHandle::new(0x1111_0000_0000_0001);
    let destination_handle = RegionHandle::new(0x2222_0000_0000_0002);
    let mut intake = SessionWorld::new(viewer_world_limits());
    intake.set_self_agent(source_id, agent_id);
    let source_region = intake.register_simulator(source_id, Some(source_handle)).expect("register source region");
    intake.register_simulator(destination_id, Some(destination_handle)).expect("register destination region");
    let mut world_primary = Some(source_id);
    let mut promoted_target = None;
    let mut observations = HashMap::new();

    apply_world_event_with_handoff(
        &mut intake,
        &simulator_message(source_id, 1, agent_update(agent_id, 10, source_handle.get())),
        &mut world_primary,
        &mut promoted_target,
        &mut observations,
        Some(agent_id),
    )
    .expect("apply source agent observation");
    apply_world_event_with_handoff(
        &mut intake,
        &simulator_message_at_order(destination_id, 2, Ambiguous, agent_update(agent_id, 20, destination_handle.get())),
        &mut world_primary,
        &mut promoted_target,
        &mut observations,
        Some(agent_id),
    )
    .expect("buffer ambiguous destination observation");
    assert!(begin_world_handoff(&mut world_primary, &mut promoted_target, source_id, destination_id));

    let observation = observations.get(&destination_id).cloned().expect("buffered destination observation");
    assert!(!complete_promoted_world_handoff(&mut intake, &mut world_primary, &mut promoted_target, destination_id, observation).0);
    assert_eq!(world_primary, Some(source_id));
    assert!(promoted_target.is_some());

    let state = intake.world();
    let agent = state.object(state.entity(agent_id).expect("source agent remains present")).expect("agent object");
    assert_eq!(agent.address().region(), source_region);
}

#[test]
fn stale_promotion_is_ignored() {
    let source = simulator_id();
    let stale_source = simulator_id();
    let target = simulator_id();
    let mut world_primary = Some(source);
    let mut pending = None;

    assert!(!begin_world_handoff(&mut world_primary, &mut pending, stale_source, target));
    assert_eq!(world_primary, Some(source));
    assert_eq!(pending, None);
}

#[test]
fn chained_promotion_retargets_handoff() {
    let agent_id = Uuid::from_u128(0x1234);
    let source_id = simulator_id();
    let middle_id = simulator_id();
    let target_id = simulator_id();
    let source_handle = RegionHandle::new(0x1111_0000_0000_0001);
    let middle_handle = RegionHandle::new(0x2222_0000_0000_0002);
    let target_handle = RegionHandle::new(0x3333_0000_0000_0003);
    let mut intake = SessionWorld::new(viewer_world_limits());
    intake.set_self_agent(source_id, agent_id);
    intake.register_simulator(source_id, Some(source_handle)).expect("register source");
    intake.register_simulator(middle_id, Some(middle_handle)).expect("register first destination");
    intake.register_simulator(target_id, Some(target_handle)).expect("register latest destination");
    let mut world_primary = Some(source_id);
    let mut pending = None;
    let mut observations = HashMap::new();

    apply_world_event_with_handoff(
        &mut intake,
        &simulator_message_at_order(source_id, 1, Ordered(1), agent_update(agent_id, 10, source_handle.get())),
        &mut world_primary,
        &mut pending,
        &mut observations,
        Some(agent_id),
    )
    .expect("apply source agent");
    assert!(begin_world_handoff(&mut world_primary, &mut pending, source_id, middle_id));
    assert!(begin_world_handoff(&mut world_primary, &mut pending, middle_id, target_id));

    apply_world_event_with_handoff(
        &mut intake,
        &simulator_message_at_order(middle_id, 2, Ordered(2), agent_update(agent_id, 20, middle_handle.get())),
        &mut world_primary,
        &mut pending,
        &mut observations,
        Some(agent_id),
    )
    .expect("ignore stale intermediate avatar observation");
    assert_eq!(world_primary, Some(source_id));

    apply_world_event_with_handoff(
        &mut intake,
        &simulator_message_at_order(target_id, 3, Ordered(3), agent_update(agent_id, 30, target_handle.get())),
        &mut world_primary,
        &mut pending,
        &mut observations,
        Some(agent_id),
    )
    .expect("apply latest destination avatar observation");

    assert_eq!(world_primary, Some(target_id));
    assert_eq!(pending, None);
    let state = intake.world();
    assert_eq!(state.object(state.entity(agent_id).unwrap()).unwrap().address().region(), intake.region_for(target_id).unwrap());
}

#[test]
fn session_exit_retires_world_regions() {
    let simulator_id = simulator_id();
    let endpoint = "127.0.0.1:13000".parse().expect("endpoint");
    let mut intake = SessionWorld::new(viewer_world_limits());
    apply_world_event(&mut intake, &SimulatorConnected { simulator_id, region_handle: None, endpoint });
    let region = intake.region_for(simulator_id).expect("registered region");

    assert!(intake.retire_all().is_empty());

    let state = intake.world();
    assert!(state.regions().next().is_none());
    assert!(state.objects().iter().next().is_none());
    assert!(state.region(region).is_none());
}

#[test]
fn unknown_removal_cancels_recovery() {
    let simulator_id = simulator_id();
    let endpoint = "127.0.0.1:13000".parse().expect("endpoint");
    let mut intake = SessionWorld::new(viewer_world_limits());
    apply_world_event(&mut intake, &SimulatorConnected { simulator_id, region_handle: None, endpoint });
    let region = intake.region_for(simulator_id).expect("registered region");
    let message = fixture_packet(vectors::KILL_OBJECT).message().clone();
    let KillObject(kill) = &message else { panic!("expected object removal fixture") };
    let addresses = kill
        .object_data
        .iter()
        .map(|object| LocalAddress::new(region, NonZeroU32::new(object.local_id).expect("valid local ID")))
        .collect::<Vec<_>>();

    let (_, summary) = apply_world_event(&mut intake, &simulator_message(simulator_id, 1, message)).expect("registered simulator");
    assert_eq!(summary.blocks.len(), addresses.len());
    for (block, address) in summary.blocks.iter().zip(&addresses) {
        let result = block.applied().expect("recorded tombstone");
        assert_eq!(result.revision, None);
        std::assert_matches!(&result.outcome, TombstoneRecorded(recorded) if recorded == address);
    }
    assert_eq!(summary.recovery_cancellation_addresses().collect::<Vec<_>>(), addresses);
}

#[test]
fn repeated_removal_keeps_recovery() {
    let simulator_id = simulator_id();
    let endpoint = "127.0.0.1:13000".parse().expect("endpoint");
    let mut intake = SessionWorld::new(viewer_world_limits());
    apply_world_event(&mut intake, &SimulatorConnected { simulator_id, region_handle: None, endpoint });
    let message = fixture_packet(vectors::KILL_OBJECT).message().clone();

    let (_, first) = apply_world_event(&mut intake, &simulator_message(simulator_id, 1, message.clone())).expect("registered simulator");
    let first_result = first.blocks[0].applied().expect("recorded tombstone");
    std::assert_matches!(&first_result.outcome, TombstoneRecorded(_));

    let (_, repeated) = apply_world_event(&mut intake, &simulator_message(simulator_id, 1, message)).expect("registered simulator");
    let repeated_result = repeated.blocks[0].applied().expect("unchanged removal");

    assert_eq!(repeated_result.revision, None);
    std::assert_matches!(&repeated_result.outcome, Unchanged);
    assert_eq!(repeated.recovery_cancellation_addresses().count(), 0);
}

#[test]
fn object_update_order_schedules_recovery() {
    let simulator_id = simulator_id();
    let region_handle = 0x2222_0000_0000_0002;
    let endpoint = "127.0.0.1:13000".parse().expect("endpoint");
    let mut intake = SessionWorld::new(viewer_world_limits());
    apply_world_event(&mut intake, &SimulatorConnected { simulator_id, region_handle: Some(region_handle), endpoint });

    let mut full = fixture_packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
    update.region_data.region_handle = region_handle;
    let parent_local_id = update.objects[0].local_id;
    let parent_id = update.objects[0].id;
    let child_id = update.objects[1].id;
    update.objects[1].parent_local_id = parent_local_id;

    let (_, full_summary) =
        apply_world_event(&mut intake, &simulator_message(simulator_id, 1, full.clone())).expect("registered simulator");
    assert_eq!(full_summary.status, IngestStatus::Applied);
    assert_eq!(full_summary.blocks.len(), 2);
    assert!(full_summary.blocks.iter().all(|block| matches!(&block.applied().unwrap().outcome, Introduced(_))));
    let region = intake.region_for(simulator_id).unwrap();
    {
        let state = intake.world();
        let child = state.object(state.entity(child_id).unwrap()).unwrap();
        std::assert_matches!(
            child.parent(),
            Parent::Resolved(parent) if state.object(parent).unwrap().id() == parent_id
        );
    }

    let mut terse = fixture_packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE).message().clone();
    let ImprovedTerseObjectUpdate(update) = &mut terse else { panic!("expected terse fixture") };
    update.region_data.region_handle = region_handle;
    let object = update.objects[0].as_decoded_mut().expect("fixture block is structurally valid");
    object.local_id = parent_local_id;
    object.motion.position = Vec3::new(31.0, 42.0, 53.0);
    object.motion.rotation = Quat::IDENTITY;
    apply_world_event(&mut intake, &simulator_message(simulator_id, 2, terse.clone())).expect("registered simulator");
    let state = intake.world();
    let parent = state.object(state.entity(parent_id).unwrap()).unwrap();
    assert_eq!(parent.transform().position(), Some(Vec3::new(31.0, 42.0, 53.0)));
    let before_cached_revision = state.revision();
    let mut cached = fixture_packet(vectors::OBJECT_UPDATE_CACHED).message().clone();
    let ObjectUpdateCached(update) = &mut cached else { panic!("expected cached object fixture") };
    update.region_data.region_handle = region_handle;
    let (_, cached_summary) = apply_world_event(&mut intake, &simulator_message(simulator_id, 3, cached)).expect("registered simulator");
    let targets = cached_summary.recovery_targets().collect::<Vec<_>>();
    assert!(!targets.is_empty());
    assert!(targets.iter().all(|target| target.region() == region));
    assert_eq!(intake.world().revision(), before_cached_revision);
    let request = recovery_request(RequestAgentData { agent_id: Uuid::from_u128(1), session_id: Uuid::from_u128(2) }, &targets);
    assert!(request.objects.iter().all(|object| object.cache_miss_type == Full));

    let mut unknown = fixture_packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE).message().clone();
    let ImprovedTerseObjectUpdate(update) = &mut unknown else { unreachable!() };
    update.region_data.region_handle = region_handle;
    update.objects[0].as_decoded_mut().expect("fixture block is structurally valid").local_id = 0x7654_3210;
    let (_, ambiguous) =
        apply_world_event(&mut intake, &simulator_message_at_order(simulator_id, 4, Ambiguous, unknown)).expect("registered simulator");
    assert_eq!(&ambiguous.blocks[0].applied().unwrap().outcome, &Deferred(AmbiguousOrder));
    assert_eq!(ambiguous.recovery_targets().count(), 1);
}

#[test]
fn invalid_update_leaves_region_unbound() {
    let simulator_id = simulator_id();
    let endpoint = "127.0.0.1:13000".parse().expect("endpoint");
    let mut intake = SessionWorld::new(viewer_world_limits());
    apply_world_event(&mut intake, &SimulatorConnected { simulator_id, region_handle: None, endpoint });

    let mut invalid = fixture_packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut invalid else { panic!("expected full update fixture") };
    update.region_data.region_handle = 0x3333_0000_0000_0003;
    update.objects.truncate(1);
    update.objects[0].id = Uuid::nil();
    let (_, summary) = apply_world_event(&mut intake, &simulator_message(simulator_id, 10, invalid)).expect("registered simulator");

    assert_eq!(summary.invalid_blocks, 1);
    assert_eq!(summary.accepted_region_handle, None);
    let region = intake.region_for(simulator_id).unwrap();
    assert_eq!(intake.world().region(region).unwrap().handle, None);
}
