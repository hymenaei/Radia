use std::{collections::HashMap, net::SocketAddr};

use network::{AgentSession, Event, PacketOrder, SimulatorId};
use protocol::lludp::Message;
use uuid::Uuid;
use world::{Applied, LocalAddress, ObjectObservation, OrderEvidence, OrderEvidence::Ordered, RegionHandle, UpdateSource};

use super::{PresentationMailbox, ViewerUpdate};
use crate::session::{
    RegionLookup, display_message,
    inbound::{
        AvatarAppearanceMessage, ObjectBatch, ObjectOperation, SitTargetUpdate, decode_avatar_appearance_update, decode_object_message,
        decode_sit_target_update,
    },
    world::{IngestStatus, IngestSummary, RegistrationError, SessionWorld},
};

const MAX_HANDOFF_OBSERVATIONS: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PendingWorldHandoff {
    pub(super) source: SimulatorId,
    pub(super) target: SimulatorId,
}

#[derive(Clone, Copy)]
pub(super) enum SimulatorScope {
    Primary,
    Neighbor,
}

pub(super) enum DecodedWorldUpdate {
    Object(ObjectBatch),
    AvatarAppearance { source: UpdateSource, update: AvatarAppearanceMessage },
    SitTarget { source: UpdateSource, update: SitTargetUpdate },
    Malformed { invalid_blocks: usize, invalid_fields: usize },
}

pub(super) fn route_recovery_targets(
    world: &SessionWorld,
    addresses: impl Iterator<Item = LocalAddress>,
) -> Vec<(Option<SimulatorId>, LocalAddress)> {
    addresses.map(|address| (world.simulator_for_region(address.region()), address)).collect()
}

pub(super) fn decode_simulator_world_update(
    world: &mut SessionWorld,
    simulator_id: SimulatorId,
    sequence: u32,
    packet_order: PacketOrder,
    message: &Message,
) -> Option<DecodedWorldUpdate> {
    let region = world.region_for(simulator_id)?;
    if let Message::RegionHandshake(handshake) = message {
        if let Err(error) = world
            .update_water_height(UpdateSource::new(region, sequence, order_evidence(packet_order), 0), handshake.region_info1.water_height)
        {
            tracing::debug!(?error, "could not update region water height");
        }
        let region_id = handshake.region_info2.as_ref().map(|info| info.region_id).filter(|id| !id.is_nil());
        if let Err(error) = world.update_handshake_metadata(
            UpdateSource::new(region, sequence, order_evidence(packet_order), 0),
            region_id,
            handshake.region_info1.sim_name.clone(),
        ) {
            tracing::warn!(?error, "could not update region handshake metadata");
        }
    }

    if let Some(update) = decode_sit_target_update(message) {
        let source = UpdateSource::new(region, sequence, order_evidence(packet_order), 0);
        return Some(DecodedWorldUpdate::SitTarget { source, update });
    }
    match decode_avatar_appearance_update(message) {
        Ok(Some(update)) => {
            let source = UpdateSource::new(region, sequence, order_evidence(packet_order), 0);
            return Some(DecodedWorldUpdate::AvatarAppearance { source, update });
        }
        Ok(None) => {}
        Err(_) => return Some(DecodedWorldUpdate::Malformed { invalid_blocks: 1, invalid_fields: 0 }),
    }
    decode_object_message(region, sequence, order_evidence(packet_order), message).map(DecodedWorldUpdate::Object)
}

pub(super) fn apply_world_event_with_handoff(
    world: &mut SessionWorld,
    event: &Event,
    world_primary: &mut Option<SimulatorId>,
    promoted_target: &mut Option<PendingWorldHandoff>,
    handoff_observations: &mut HashMap<SimulatorId, ObjectObservation>,
    agent_id: Option<Uuid>,
) -> Option<(SimulatorId, IngestSummary)> {
    use Event::{SessionReady, SimulatorConnected, SimulatorDisconnected, SimulatorFailed, SimulatorMessage};

    match event {
        SessionReady { simulator_id, session, .. } => {
            world.set_self_agent(session.agent_id);
            *world_primary = Some(*simulator_id);
            match world.register_simulator(*simulator_id, None) {
                Ok(_) => {}
                Err(RegistrationError::HandleChanged) => {
                    tracing::error!("primary simulator handle changed during one connection lifetime");
                }
                Err(RegistrationError::Apply(error)) => tracing::error!(?error, "could not register primary world region"),
            }
            None
        }
        SimulatorConnected { simulator_id, region_handle, .. } => {
            let handle = region_handle.map(RegionHandle::new);
            match world.register_simulator(*simulator_id, handle) {
                Ok(_) => {}
                Err(RegistrationError::HandleChanged) => {
                    tracing::warn!("simulator handle changed during one connection lifetime");
                }
                Err(RegistrationError::Apply(error)) => tracing::warn!(?error, "could not register simulator world region"),
            }
            None
        }
        SimulatorMessage { simulator_id, sequence, packet_order, message, .. } => {
            let update = decode_simulator_world_update(world, *simulator_id, *sequence, *packet_order, message)?;
            apply_decoded_world_update(world, *simulator_id, update, world_primary, promoted_target, handoff_observations, agent_id)
        }
        SimulatorDisconnected { simulator_id, .. } | SimulatorFailed { simulator_id, .. } => {
            if let Err(error) = world.retire_simulator(*simulator_id) {
                tracing::warn!(?error, "could not retire simulator world region");
            }
            handoff_observations.remove(simulator_id);
            if promoted_target.is_some_and(|handoff| handoff.target == *simulator_id) {
                *promoted_target = None;
            }
            None
        }
        _ => None,
    }
}

pub(super) async fn apply_world_event_cooperatively(
    world: &mut SessionWorld,
    event: &Event,
    world_primary: &mut Option<SimulatorId>,
    promoted_target: &mut Option<PendingWorldHandoff>,
    handoff_observations: &mut HashMap<SimulatorId, ObjectObservation>,
    agent_id: Option<Uuid>,
    on_object_chunk: &mut impl FnMut(SimulatorId, &IngestSummary, Vec<(Option<SimulatorId>, LocalAddress)>, Vec<LocalAddress>),
) -> Option<(SimulatorId, IngestSummary)> {
    if let Event::SimulatorMessage { simulator_id, sequence, packet_order, message, .. } = event {
        let update = decode_simulator_world_update(world, *simulator_id, *sequence, *packet_order, message)?;
        return match update {
            DecodedWorldUpdate::Object(batch) => {
                let batch = filter_handoff_batch(world, *simulator_id, *world_primary, agent_id, batch, handoff_observations)?;
                let _summary = world
                    .apply_object_batch_cooperatively(*simulator_id, batch, |summary, targets, cancellations| {
                        on_object_chunk(*simulator_id, summary, targets, cancellations);
                    })
                    .await?;
                complete_handoff_observation(world, *simulator_id, world_primary, promoted_target, handoff_observations)
                    .map(|summary| (*simulator_id, summary))
            }
            update => {
                apply_decoded_world_update(world, *simulator_id, update, world_primary, promoted_target, handoff_observations, agent_id)
            }
        };
    }
    apply_world_event_with_handoff(world, event, world_primary, promoted_target, handoff_observations, agent_id)
}

pub(super) fn apply_decoded_world_update(
    world: &mut SessionWorld,
    simulator_id: SimulatorId,
    update: DecodedWorldUpdate,
    world_primary: &mut Option<SimulatorId>,
    promoted_target: &mut Option<PendingWorldHandoff>,
    handoff_observations: &mut HashMap<SimulatorId, ObjectObservation>,
    agent_id: Option<Uuid>,
) -> Option<(SimulatorId, IngestSummary)> {
    match update {
        DecodedWorldUpdate::Object(batch) => {
            let batch = filter_handoff_batch(world, simulator_id, *world_primary, agent_id, batch, handoff_observations)?;
            let summary = world.apply_object_batch(simulator_id, batch);
            complete_batch_handoff(world, simulator_id, world_primary, promoted_target, handoff_observations, summary)
        }
        DecodedWorldUpdate::SitTarget { source, update } => {
            let address = world.object_address(simulator_id, update.object_id);
            let result = world.apply_sit_target_update(simulator_id, source, update);
            match result {
                Some(Ok(applied)) => {
                    if !applied.rejections.is_empty() {
                        tracing::debug!(rejections = ?applied.rejections, "avatar sit response contained rejected metadata");
                    }
                    let mut summary = IngestSummary::new(IngestStatus::Applied);
                    summary.push_applied(applied, address);
                    Some((simulator_id, summary))
                }
                Some(Err(error)) => {
                    tracing::debug!(?error, "could not apply avatar sit target");
                    let mut summary = IngestSummary::new(IngestStatus::Applied);
                    summary.push_error(error, address);
                    Some((simulator_id, summary))
                }
                None => None,
            }
        }
        DecodedWorldUpdate::AvatarAppearance { source, update, .. } => {
            let object_id = update.object_id;
            let invalid_fields = update.invalid_fields;
            match world.apply_avatar_appearance(simulator_id, source, update) {
                Some(Ok(applied)) => {
                    if !applied.rejections.is_empty() {
                        tracing::debug!(rejections = ?applied.rejections, "avatar appearance contained rejected fields");
                    }
                    let mut summary = IngestSummary::new(IngestStatus::Applied);
                    summary.invalid_fields = invalid_fields;
                    summary.push_applied(applied, world.object_address(simulator_id, object_id));
                    Some((simulator_id, summary))
                }
                Some(Err(error)) => {
                    tracing::debug!(?error, "could not apply avatar appearance");
                    let mut summary = IngestSummary::new(IngestStatus::Applied);
                    summary.invalid_fields = invalid_fields;
                    summary.push_error(error, world.object_address(simulator_id, object_id));
                    Some((simulator_id, summary))
                }
                None => None,
            }
        }
        DecodedWorldUpdate::Malformed { invalid_blocks, invalid_fields } => {
            let mut summary = IngestSummary::new(IngestStatus::Applied);
            summary.invalid_blocks = invalid_blocks;
            summary.invalid_fields = invalid_fields;
            Some((simulator_id, summary))
        }
    }
}

pub(super) fn filter_handoff_batch(
    world: &SessionWorld,
    simulator_id: SimulatorId,
    world_primary: Option<SimulatorId>,
    agent_id: Option<Uuid>,
    mut batch: ObjectBatch,
    handoff_observations: &mut HashMap<SimulatorId, ObjectObservation>,
) -> Option<ObjectBatch> {
    if world_primary != Some(simulator_id)
        && let Some(agent_id) = agent_id
    {
        let agent_region = world.region_for(simulator_id)?;
        let mut retained = Vec::with_capacity(batch.blocks.len());
        for operation in batch.blocks.drain(..) {
            match operation {
                Some(ObjectOperation::Observe(observation)) if observation.id == agent_id => {
                    let replace = handoff_observations
                        .get(&simulator_id)
                        .is_none_or(|current| should_replace_handoff_observation(current, &observation));
                    if replace {
                        handoff_observations.insert(simulator_id, observation);
                    }
                }
                Some(operation) => match handoff_observations.get(&simulator_id) {
                    Some(observation)
                        if operation.address() == LocalAddress::new(agent_region, observation.local_id)
                            && conflicts_with_handoff_observation(&operation, observation) =>
                    {
                        if operation_is_newer(operation.source(), observation.source) {
                            handoff_observations.remove(&simulator_id);
                        }
                        retained.push(Some(operation));
                    }
                    Some(observation) if operation.address() == LocalAddress::new(agent_region, observation.local_id) => {}
                    _ => retained.push(Some(operation)),
                },
                operation => retained.push(operation),
            }
        }
        batch.blocks = retained;
        while handoff_observations.len() > MAX_HANDOFF_OBSERVATIONS {
            if let Some(stale) = handoff_observations.keys().next().copied() {
                handoff_observations.remove(&stale);
            }
        }
    }
    Some(batch)
}

pub(super) fn complete_batch_handoff(
    world: &mut SessionWorld,
    simulator_id: SimulatorId,
    world_primary: &mut Option<SimulatorId>,
    promoted_target: &mut Option<PendingWorldHandoff>,
    handoff_observations: &mut HashMap<SimulatorId, ObjectObservation>,
    summary: Option<IngestSummary>,
) -> Option<(SimulatorId, IngestSummary)> {
    let mut summary = summary.map(|summary| (simulator_id, summary));
    if let Some(extra) = complete_handoff_observation(world, simulator_id, world_primary, promoted_target, handoff_observations) {
        let summary = summary.get_or_insert_with(|| (simulator_id, IngestSummary::new(IngestStatus::Applied)));
        summary.1.append(extra);
    }
    summary
}

pub(super) fn complete_handoff_observation(
    world: &mut SessionWorld,
    simulator_id: SimulatorId,
    world_primary: &mut Option<SimulatorId>,
    promoted_target: &mut Option<PendingWorldHandoff>,
    handoff_observations: &mut HashMap<SimulatorId, ObjectObservation>,
) -> Option<IngestSummary> {
    if let Some(observation) = handoff_observations.get(&simulator_id).cloned() {
        let (completed, applied) =
            complete_promoted_world_handoff(world, world_primary, promoted_target, simulator_id, observation.clone());
        if completed {
            handoff_observations.remove(&simulator_id);
        }
        if let Some(applied) = applied {
            let mut summary = IngestSummary::new(IngestStatus::Applied);
            summary.push_applied(applied, Some(LocalAddress::new(observation.source.region(), observation.local_id)));
            return Some(summary);
        }
    }
    None
}

pub(super) fn conflicts_with_handoff_observation(operation: &ObjectOperation, observation: &ObjectObservation) -> bool {
    match operation {
        ObjectOperation::Observe(candidate) => candidate.id != observation.id,
        ObjectOperation::Remove { .. } => true,
        ObjectOperation::Patch(_) | ObjectOperation::Cached { .. } => false,
    }
}

pub(super) fn operation_is_newer(candidate: UpdateSource, current: UpdateSource) -> bool {
    match (candidate.order(), current.order()) {
        (Ordered(candidate_order), Ordered(current_order)) => {
            (candidate_order, candidate.block_ordinal()) > (current_order, current.block_ordinal())
        }
        _ => false,
    }
}

pub(super) fn should_replace_handoff_observation(current: &ObjectObservation, candidate: &ObjectObservation) -> bool {
    if current.source.region() != candidate.source.region() {
        return true;
    }
    match (current.source.order(), candidate.source.order()) {
        (OrderEvidence::Ordered(current_order), OrderEvidence::Ordered(candidate_order)) => {
            (candidate_order, candidate.source.block_ordinal()) > (current_order, current.source.block_ordinal())
        }
        (OrderEvidence::Ambiguous | OrderEvidence::Degraded, OrderEvidence::Ordered(_)) => true,
        (OrderEvidence::Ordered(_), OrderEvidence::Ambiguous | OrderEvidence::Degraded)
        | (OrderEvidence::Ambiguous | OrderEvidence::Degraded, OrderEvidence::Ambiguous | OrderEvidence::Degraded) => false,
    }
}

pub(super) fn begin_world_handoff(
    world_primary: &mut Option<SimulatorId>,
    pending: &mut Option<PendingWorldHandoff>,
    source: SimulatorId,
    target: SimulatorId,
) -> bool {
    if source == target {
        return false;
    }
    if let Some(current) = *pending
        && current.target == source
        && *world_primary == Some(current.source)
    {
        *pending = Some(PendingWorldHandoff { source: current.source, target });
        return true;
    }
    if *world_primary != Some(source) {
        return false;
    }
    *pending = Some(PendingWorldHandoff { source, target });
    true
}

pub(super) fn complete_promoted_world_handoff(
    world: &mut SessionWorld,
    world_primary: &mut Option<SimulatorId>,
    pending: &mut Option<PendingWorldHandoff>,
    target: SimulatorId,
    observation: ObjectObservation,
) -> (bool, Option<Applied>) {
    let Some(handoff) = *pending else { return (false, None) };
    if handoff.target != target || *world_primary != Some(handoff.source) {
        return (false, None);
    }
    let (completed, applied) = world.complete_agent_handoff(handoff.source, target, observation);
    if !completed {
        return (false, applied);
    }
    *world_primary = Some(target);
    *pending = None;
    (true, applied)
}

pub(super) fn complete_promoted_handoff(
    world: &mut SessionWorld,
    world_primary: &mut Option<SimulatorId>,
    pending: &mut Option<PendingWorldHandoff>,
    target: SimulatorId,
    observation: ObjectObservation,
) -> (bool, Option<IngestSummary>) {
    let address = LocalAddress::new(observation.source.region(), observation.local_id);
    let (completed, applied) = complete_promoted_world_handoff(world, world_primary, pending, target, observation);
    let summary = applied.map(|applied| {
        let mut summary = IngestSummary::new(IngestStatus::Applied);
        summary.push_applied(applied, Some(address));
        summary.log_incomplete_data(target);
        summary
    });
    (completed, summary)
}

pub(super) fn order_evidence(packet_order: PacketOrder) -> OrderEvidence {
    match packet_order {
        PacketOrder::Ordered(order) => OrderEvidence::Ordered(order),
        PacketOrder::Ambiguous => OrderEvidence::Ambiguous,
        PacketOrder::Degraded => OrderEvidence::Degraded,
    }
}

pub(super) fn handle_simulator_message(
    scope: SimulatorScope,
    source: SocketAddr,
    message: Message,
    session: Option<&AgentSession>,
    region_lookup: Option<&RegionLookup>,
    region_name: Option<&str>,
    mailbox: &PresentationMailbox,
) {
    use Message::{ChatFromSimulator, RegionHandshake};

    if matches!(scope, SimulatorScope::Primary) {
        if let Some(region_lookup) = region_lookup {
            region_lookup.handle_message(source, &message);
        }
        if matches!(&message, RegionHandshake(_))
            && let Some(name) = region_name
        {
            mailbox.publish(ViewerUpdate::RegionName(name.to_owned()));
        }
    }
    if let ChatFromSimulator(message) = message
        && let Some(chat_message) = display_message(message.chat_data, session)
    {
        mailbox.publish(ViewerUpdate::ChatMessage(chat_message));
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, num::NonZeroU32};

    use math::{Quat, Vec3};
    use network::{
        Event::{SimulatorConnected, SimulatorDisconnected},
        PacketOrder::{Ambiguous, Ordered},
        testing::simulator_id,
    };
    use protocol::{
        lludp::Message,
        lludp::Message::{ImprovedTerseObjectUpdate, KillObject, ObjectUpdate, ObjectUpdateCached},
        messages::{CacheMissType::Full, RequestAgentData, object::kill_object},
    };
    use test_support::lludp::vectors;
    use uuid::Uuid;
    use world::{
        ApplyOutcome::{Deferred, Introduced, TombstoneRecorded, Unchanged},
        DeferredReason::AmbiguousOrder,
        LocalAddress, Parent, Pose, Region, RegionHandle, UpdateSource,
    };

    use crate::session::coordinator::events::{
        PendingWorldHandoff, apply_world_event_with_handoff, begin_world_handoff, complete_promoted_handoff,
        complete_promoted_world_handoff,
    };
    use crate::session::coordinator::tests::apply_world_event;
    use crate::session::coordinator::tests::{
        agent_observation, agent_update, fixture_packet, region_handshake, simulator_message, simulator_message_at_order,
    };
    use crate::session::inbound::{ObjectBatch, ObjectOperation};
    use crate::session::{
        recovery::recovery_request,
        world::{IngestStatus, SessionWorld, viewer_world_limits},
    };

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

    #[test]
    fn agent_waits_for_primary_promotion() {
        let agent_id = Uuid::from_u128(0x1234);
        let source_id = simulator_id();
        let destination_id = simulator_id();
        let source_handle = RegionHandle::new(0x1111_0000_0000_0001);
        let destination_handle = RegionHandle::new(0x2222_0000_0000_0002);
        let mut intake = SessionWorld::new(viewer_world_limits());
        intake.set_self_agent(agent_id);
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
        intake.set_self_agent(agent_id);
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
        intake.set_self_agent(agent_id);
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
        intake.set_self_agent(agent_id);
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
        intake.set_self_agent(agent_id);
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

        let (_, first) =
            apply_world_event(&mut intake, &simulator_message(simulator_id, 1, message.clone())).expect("registered simulator");
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
        let (_, cached_summary) =
            apply_world_event(&mut intake, &simulator_message(simulator_id, 3, cached)).expect("registered simulator");
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
}
