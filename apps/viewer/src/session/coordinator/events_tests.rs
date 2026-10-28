use std::{collections::HashMap, num::NonZeroU32, sync::Arc};

use math::{Quat, Vec3};
use network::{
    Event::{SimulatorConnected, SimulatorDisconnected},
    HandoffEventOutcome,
    PacketOrder::{Ambiguous, Ordered},
};
use protocol::{
    lludp::Message,
    lludp::Message::{ImprovedTerseObjectUpdate, KillObject, ObjectUpdate, ObjectUpdateCached},
    messages::{CacheMissType::Full, RequestAgentData, object::kill_object},
};
use test_support::lludp::vectors;
use uuid::Uuid;
use world::{
    Applied,
    ApplyOutcome::{Deferred, Introduced, TombstoneRecorded, Unchanged},
    DeferredReason::AmbiguousOrder,
    LocalAddress, ObjectObservation, OrderEvidence, PARCEL_BITMAP_BYTES, PARCEL_OVERLAY_PART_BYTES, PARCEL_OVERLAY_PARTS,
    ParcelAccessSettings, ParcelBitmap, ParcelFlags, ParcelLocalId, ParcelOverlayObservation, ParcelPropertiesData,
    ParcelPropertiesObservation, Parent, Pose, Region, RegionHandle, RegionOutcome, UpdateSource,
};

use crate::session::coordinator::events::{PendingWorldHandoff, WorldHandoff};
use crate::session::coordinator::tests::apply_world_event;
use crate::session::coordinator::tests::{
    agent_observation, agent_update, fixture_packet, region_handshake, simulator_message, simulator_message_at_order,
};
use crate::session::inbound::{ObjectBatch, ObjectOperation};
use crate::session::{
    recovery::recovery_request,
    teleport::{Destination, TeleportController, TeleportStatus},
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
    let mut handoff =
        WorldHandoff { world_primary: *world_primary, promoted_target: *promoted_target, observations: std::mem::take(observations) };
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
    let mut handoff = WorldHandoff { world_primary: *world_primary, promoted_target: *promoted_target, observations: HashMap::new() };
    let result = handoff.begin(source, target);
    *world_primary = handoff.world_primary;
    *promoted_target = handoff.promoted_target;
    result
}

fn complete_promoted_handoff(
    world: &mut SessionWorld,
    world_primary: &mut Option<network::SimulatorId>,
    promoted_target: &mut Option<PendingWorldHandoff>,
    target: network::SimulatorId,
    observation: ObjectObservation,
) -> (bool, Option<super::IngestSummary>) {
    let mut handoff = WorldHandoff { world_primary: *world_primary, promoted_target: *promoted_target, observations: HashMap::new() };
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
    let mut handoff = WorldHandoff { world_primary: *world_primary, promoted_target: *promoted_target, observations: HashMap::new() };
    let result = handoff.complete_world_handoff(world, target, observation);
    *world_primary = handoff.world_primary;
    *promoted_target = handoff.promoted_target;
    result
}

fn parcel_properties(local_id: i32) -> ParcelPropertiesData {
    let bitmap = ParcelBitmap::from_bytes(&[0; PARCEL_BITMAP_BYTES]).expect("parcel bitmap");
    ParcelPropertiesData {
        local_id: ParcelLocalId::new(local_id).expect("parcel local ID"),
        name: Arc::from("parcel"),
        description: Arc::from("parcel description"),
        owner_id: None,
        group_id: None,
        group_owned: false,
        area: 1024,
        flags: ParcelFlags::from_bits(0),
        bitmap,
        landing_point: None,
        landing_look_at: None,
        landing_type: None,
        music_url: Arc::from(""),
        media_url: Arc::from(""),
        access: ParcelAccessSettings::default(),
    }
}

fn seed_parcel_region(world: &mut SessionWorld, region: Region, order: u64, local_id: i32, overlay_value: u8) {
    let source =
        |order: u64| UpdateSource::new(region, u32::try_from(order).expect("test update sequence"), OrderEvidence::Ordered(order), 0);
    let _properties_applied = world
        .observe_parcel_properties(
            source(order),
            ParcelPropertiesObservation {
                request_result: 0,
                sequence_id: i32::try_from(order).expect("test parcel sequence"),
                properties: Some(parcel_properties(local_id)),
            },
        )
        .expect("store parcel properties");
    for part in 0..PARCEL_OVERLAY_PARTS {
        let _overlay_applied = world
            .observe_parcel_overlay(
                source(order + 1 + part as u64),
                ParcelOverlayObservation {
                    sequence_id: i32::try_from(part).expect("overlay part index"),
                    data: vec![overlay_value; PARCEL_OVERLAY_PART_BYTES],
                },
            )
            .expect("store parcel overlay part");
    }
    let _flags_applied = world.observe_region_flags(source(order + 5), Some(1), Some(2)).expect("store region flags");
}

#[test]
fn region_connection_lifecycle() {
    let simulator_id = simulator_id();
    let handle = 0x1111_0000_0000_0001;
    let endpoint = "127.0.0.1:13000".parse().expect("endpoint");
    let mut intake = SessionWorld::new(viewer_world_limits());

    apply_world_event(&mut intake, &SimulatorConnected { simulator_id, region_handle: Some(handle), endpoint });
    let region = intake.region_for(simulator_id).expect("registered region");
    assert_eq!(intake.world().region(region).unwrap().handle, Some(RegionHandle::new(handle)));

    apply_world_event(&mut intake, &SimulatorDisconnected { simulator_id, region_handle: Some(handle), endpoint });
    assert!(intake.region_for(simulator_id).is_none());
    assert!(intake.world().region(region).is_none());
    assert!(apply_world_event(&mut intake, &simulator_message(simulator_id, 1, region_handshake("late"))).is_none());
}

#[test]
fn local_teleport_changes_only_the_authenticated_agents_position() {
    let simulator = simulator_id();
    let agent_id = Uuid::from_u128(0x1234);
    let handle = RegionHandle::new(0x1111_0000_0000_0001);
    let mut world = SessionWorld::new(viewer_world_limits());
    world.set_self_agent(simulator, agent_id);
    let region = world.register_simulator(simulator, Some(handle)).expect("register primary region");
    assert!(apply_world_event(&mut world, &simulator_message(simulator, 1, agent_update(agent_id, 10, handle.get()))).is_some());

    let entity = world.world().entity(agent_id).expect("self avatar is present");
    let before = world.world().object(entity).expect("self avatar object").local_transform().expect("avatar transform");
    let position = Vec3::new(34.0, 56.0, 78.0);
    let source = UpdateSource::new(region, 2, OrderEvidence::Ordered(2), 0);
    assert!(world.apply_teleport_local(simulator, agent_id, source, position).expect("self avatar address").is_ok());

    let after = world.world().object(entity).expect("self avatar remains present").local_transform().expect("updated avatar transform");
    assert_eq!(after.position, position);
    assert_eq!(after.rotation, before.rotation);
}

#[test]
fn teleport_retirement_removes_old_primary_and_neighbor_world_state() {
    let primary = simulator_id();
    let neighbor = simulator_id();
    let destination = simulator_id();
    let agent_id = Uuid::from_u128(0x1234);
    let primary_handle = RegionHandle::new(0x1111_0000_0000_0001);
    let neighbor_handle = RegionHandle::new(0x2222_0000_0000_0002);
    let destination_handle = RegionHandle::new(0x3333_0000_0000_0003);
    let mut world = SessionWorld::new(viewer_world_limits());
    let mut handoff = WorldHandoff::default();
    handoff.session_ready(&mut world, primary, agent_id);
    world.register_simulator(neighbor, Some(neighbor_handle)).expect("register neighbor region");
    world.register_simulator(destination, Some(destination_handle)).expect("register destination region");
    let neighbor_object = Uuid::from_u128(0x2222);
    let destination_object = Uuid::from_u128(0x3333);
    assert!(apply_world_event(&mut world, &simulator_message(primary, 1, agent_update(agent_id, 10, primary_handle.get()))).is_some());
    assert!(
        apply_world_event(&mut world, &simulator_message(neighbor, 2, agent_update(neighbor_object, 20, neighbor_handle.get()))).is_some()
    );
    assert!(
        apply_world_event(&mut world, &simulator_message(destination, 3, agent_update(destination_object, 30, destination_handle.get())))
            .is_some()
    );
    assert!(world.world().entity(agent_id).is_some());
    assert!(world.world().entity(neighbor_object).is_some());
    assert!(world.world().entity(destination_object).is_some());

    handoff.teleport_completed(&mut world, destination);

    assert!(world.region_for(primary).is_none());
    assert!(world.region_for(neighbor).is_none());
    assert!(world.region_for(destination).is_some());
    assert!(world.world().entity(agent_id).is_none());
    assert!(world.world().entity(neighbor_object).is_none());
    assert!(world.world().entity(destination_object).is_some());
}

#[test]
fn return_teleport_restores_reused_region_route_and_keeps_both_retired_scenes_empty() {
    let returning_simulator = simulator_id();
    let current_simulator = simulator_id();
    let agent_id = Uuid::from_u128(0x1234);
    let old_return_object = Uuid::from_u128(0x1111);
    let current_object = Uuid::from_u128(0x2222);
    let returned_object = Uuid::from_u128(0x3333);
    let returning_handle = RegionHandle::new(0x1111_0000_0000_0001);
    let current_handle = RegionHandle::new(0x2222_0000_0000_0002);
    let endpoint = "127.0.0.1:13001".parse().expect("return simulator endpoint");
    let mut world = SessionWorld::new(viewer_world_limits());
    let mut handoff = WorldHandoff::default();
    handoff.session_ready(&mut world, returning_simulator, agent_id);
    handoff.simulator_connected(&mut world, returning_simulator, Some(returning_handle.get()));
    handoff.simulator_connected(&mut world, current_simulator, Some(current_handle.get()));
    let original_return_region = world.region_for(returning_simulator).expect("original return region");
    let current_region = world.region_for(current_simulator).expect("current region");
    seed_parcel_region(&mut world, original_return_region, 10, 31, 7);
    seed_parcel_region(&mut world, current_region, 20, 42, 8);
    assert!(
        apply_world_event(
            &mut world,
            &simulator_message(returning_simulator, 1, agent_update(old_return_object, 11, returning_handle.get()))
        )
        .is_some()
    );
    assert!(
        apply_world_event(&mut world, &simulator_message(current_simulator, 2, agent_update(current_object, 22, current_handle.get())))
            .is_some()
    );

    handoff.teleport_completed(&mut world, current_simulator);
    assert!(world.region_for(returning_simulator).is_none());
    assert!(world.world().entity(old_return_object).is_none());
    assert!(world.world().parcel_state(original_return_region).is_none());

    let mut teleport = TeleportController::new();
    assert!(teleport.accept_lifecycle_packet(current_simulator, Ordered(1)));
    teleport.on_start(&protocol::messages::TeleportStart { teleport_flags: 0 }, Some(current_handle.get()));
    let destination = Destination { region_handle: returning_handle.get(), endpoint };
    assert!(teleport.accept_lifecycle_packet(current_simulator, Ordered(2)));
    assert!(!teleport.on_finish(destination, 0, Some(13), HandoffEventOutcome::Accepted));
    let stale_local = protocol::messages::TeleportLocal {
        agent_id,
        location_id: 0,
        position: Vec3::new(5.0, 6.0, 7.0),
        look_at: Vec3::Y,
        teleport_flags: 0,
    };
    if teleport.accept_local_packet(current_simulator, Ordered(1)) {
        teleport.on_local(&stale_local, Some(current_handle.get()), agent_id);
    }
    assert_eq!(
        teleport.handle().state().status,
        TeleportStatus::Connecting { region_handle: returning_handle.get() },
        "an older local notice must not replace the accepted cross-region finish"
    );
    let region_handle = teleport.region_handle_for_promotion(endpoint).expect("accepted return destination");
    assert!(handoff.restore_promoted_region(&mut world, returning_simulator, region_handle).is_some());
    assert!(teleport.on_simulator_promoted(Some(region_handle), endpoint));
    handoff.teleport_completed(&mut world, returning_simulator);

    let returned_region = world.region_for(returning_simulator).expect("restored return region");
    assert_ne!(returned_region, original_return_region);
    assert_eq!(world.world().region(returned_region).expect("restored World region").handle, Some(returning_handle));
    let returned_parcels = world.world().parcel_state(returned_region).expect("fresh parcel state for returned region");
    assert!(returned_parcels.current.is_none());
    assert!(returned_parcels.overlay.is_none());
    assert!(world.region_for(current_simulator).is_none());
    assert!(world.world().entity(old_return_object).is_none());
    assert!(world.world().entity(current_object).is_none());
    assert!(
        apply_world_event(
            &mut world,
            &simulator_message(returning_simulator, 3, agent_update(returned_object, 33, returning_handle.get()))
        )
        .is_some()
    );
    assert!(world.world().entity(returned_object).is_some());
}

#[test]
fn region_crossing_clears_agent_parcels_and_keeps_region_overlay_and_flags() {
    let source_simulator = simulator_id();
    let target_simulator = simulator_id();
    let agent_id = Uuid::from_u128(0x1234);
    let source_handle = RegionHandle::new(0x1111_0000_0000_0001);
    let target_handle = RegionHandle::new(0x2222_0000_0000_0002);
    let mut world = SessionWorld::new(viewer_world_limits());
    let mut handoff = WorldHandoff::default();
    handoff.session_ready(&mut world, source_simulator, agent_id);
    handoff.simulator_connected(&mut world, target_simulator, Some(target_handle.get()));
    let source_region = world.region_for(source_simulator).expect("source region");
    let target_region = world.region_for(target_simulator).expect("target region");
    seed_parcel_region(&mut world, source_region, 10, 31, 7);
    seed_parcel_region(&mut world, target_region, 20, 42, 8);

    assert!(
        handoff
            .apply_event(
                &mut world,
                &simulator_message(source_simulator, 1, agent_update(agent_id, 11, source_handle.get())),
                Some(agent_id),
            )
            .is_some()
    );
    let destination = agent_observation(UpdateSource::new(target_region, 30, OrderEvidence::Ordered(30), 0), 22, agent_id, None);
    assert!(handoff.begin(source_simulator, target_simulator));
    let (completed, _) = handoff.complete_promoted(&mut world, target_simulator, destination);
    assert!(completed);

    for region in [source_region, target_region] {
        let parcel = world.world().parcel_state(region).expect("active region parcel state");
        assert!(parcel.current.is_none());
        assert_eq!(parcel.overlay.as_ref().and_then(|grid| grid.raw_cell(0, 0)), Some(if region == source_region { 7 } else { 8 }));
        assert_eq!(parcel.region_flags.low, Some(1));
        assert_eq!(parcel.region_flags.extended, Some(2));
        assert!(world.world().environment(region).expect("active region environment").parcel.is_none());
    }

    let applied = world
        .observe_parcel_properties(
            UpdateSource::new(target_region, 40, OrderEvidence::Ordered(40), 0),
            ParcelPropertiesObservation { request_result: 0, sequence_id: 41, properties: Some(parcel_properties(43)) },
        )
        .expect("new primary parcel properties are accepted");
    std::assert_matches!(applied.outcome, RegionOutcome::Patched { .. });
    assert_eq!(
        world.world().parcel_state(target_region).unwrap().current.as_ref().unwrap().properties.local_id,
        ParcelLocalId::new(43).expect("new current parcel")
    );
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
    assert!(matches!(&result.outcome, TombstoneRecorded(address) if *address == removed_address));
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
        assert!(matches!(&result.outcome, TombstoneRecorded(recorded) if recorded == address));
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
    assert!(matches!(&first_result.outcome, TombstoneRecorded(_)));

    let (_, repeated) = apply_world_event(&mut intake, &simulator_message(simulator_id, 1, message)).expect("registered simulator");
    let repeated_result = repeated.blocks[0].applied().expect("unchanged removal");

    assert_eq!(repeated_result.revision, None);
    assert!(matches!(&repeated_result.outcome, Unchanged));
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
        assert!(matches!(
            child.parent(),
            Parent::Resolved(parent) if state.object(parent).unwrap().id() == parent_id
        ));
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
