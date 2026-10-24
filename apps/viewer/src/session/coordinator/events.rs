use std::{collections::HashMap, net::SocketAddr};

#[cfg(test)]
use network::Event;
use network::{AgentSession, PacketOrder, SimulatorId};
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

#[derive(Default)]
pub(super) struct WorldHandoff {
    world_primary: Option<SimulatorId>,
    promoted_target: Option<PendingWorldHandoff>,
    observations: HashMap<SimulatorId, ObjectObservation>,
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
            tracing::debug!(simulator = simulator_id.get(), region = region.get(), %error, "could not update region water height");
        }
        let region_id = handshake.region_info2.as_ref().map(|info| info.region_id).filter(|id| !id.is_nil());
        if let Err(error) = world.update_handshake_metadata(
            UpdateSource::new(region, sequence, order_evidence(packet_order), 0),
            region_id,
            handshake.region_info1.sim_name.clone(),
        ) {
            tracing::warn!(simulator = simulator_id.get(), region = region.get(), %error, "could not update region handshake metadata");
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

impl WorldHandoff {
    pub(super) fn session_ready(&mut self, world: &mut SessionWorld, simulator_id: SimulatorId, agent_id: Uuid) {
        let region = world.region_for(simulator_id).map(|region| region.get());
        world.set_self_agent(simulator_id, agent_id);
        self.world_primary = Some(simulator_id);
        match world.register_simulator(simulator_id, None) {
            Ok(_) => {}
            Err(RegistrationError::HandleChanged) => {
                tracing::error!(simulator = simulator_id.get(), region, "primary simulator handle changed during one connection lifetime");
            }
            Err(RegistrationError::Apply(error)) => {
                tracing::error!(simulator = simulator_id.get(), region, %error, "could not register primary world region")
            }
        }
    }

    pub(super) fn simulator_connected(&mut self, world: &mut SessionWorld, simulator_id: SimulatorId, region_handle: Option<u64>) {
        let region = world.region_for(simulator_id).map(|region| region.get());
        let handle = region_handle.map(RegionHandle::new);
        match world.register_simulator(simulator_id, handle) {
            Ok(_) => {}
            Err(RegistrationError::HandleChanged) => {
                tracing::warn!(simulator = simulator_id.get(), region, "simulator handle changed during one connection lifetime");
            }
            Err(RegistrationError::Apply(error)) => {
                tracing::warn!(simulator = simulator_id.get(), region, %error, "could not register simulator world region")
            }
        }
    }

    pub(super) fn simulator_disconnected(&mut self, world: &mut SessionWorld, simulator_id: SimulatorId) {
        let region = world.region_for(simulator_id).map(|region| region.get());
        if let Err(error) = world.retire_simulator(simulator_id) {
            tracing::warn!(simulator = simulator_id.get(), region, %error, "could not retire simulator world region");
        }
        self.observations.remove(&simulator_id);
        if self.promoted_target.is_some_and(|handoff| handoff.target == simulator_id) {
            self.promoted_target = None;
        }
    }

    #[cfg(test)]
    pub(super) fn apply_event(
        &mut self,
        world: &mut SessionWorld,
        event: &Event,
        agent_id: Option<Uuid>,
    ) -> Option<(SimulatorId, IngestSummary)> {
        use Event::{SessionReady, SimulatorConnected, SimulatorDisconnected, SimulatorFailed, SimulatorMessage};

        match event {
            SessionReady { simulator_id, session, .. } => {
                self.session_ready(world, *simulator_id, session.agent_id);
                None
            }
            SimulatorConnected { simulator_id, region_handle, .. } => {
                self.simulator_connected(world, *simulator_id, *region_handle);
                None
            }
            SimulatorMessage { simulator_id, sequence, packet_order, message, .. } => {
                let update = decode_simulator_world_update(world, *simulator_id, *sequence, *packet_order, message)?;
                self.apply_update(world, *simulator_id, update, agent_id)
            }
            SimulatorDisconnected { simulator_id, .. } | SimulatorFailed { simulator_id, .. } => {
                self.simulator_disconnected(world, *simulator_id);
                None
            }
            _ => None,
        }
    }

    pub(super) async fn apply_message_cooperatively(
        &mut self,
        world: &mut SessionWorld,
        simulator_id: SimulatorId,
        sequence: u32,
        packet_order: PacketOrder,
        message: &Message,
        agent_id: Option<Uuid>,
        on_object_chunk: &mut impl FnMut(Vec<(Option<SimulatorId>, LocalAddress)>, Vec<LocalAddress>),
    ) -> Option<(SimulatorId, IngestSummary)> {
        let update = decode_simulator_world_update(world, simulator_id, sequence, packet_order, message)?;
        match update {
            DecodedWorldUpdate::Object(batch) => {
                let batch = self.filter_batch(world, simulator_id, agent_id, batch)?;
                let _summary = world
                    .apply_object_batch_cooperatively(simulator_id, batch, |_summary, targets, cancellations| {
                        on_object_chunk(targets, cancellations);
                    })
                    .await?;
                self.complete_observation(world, simulator_id).map(|summary| (simulator_id, summary))
            }
            update => self.apply_update(world, simulator_id, update, agent_id),
        }
    }

    fn apply_update(
        &mut self,
        world: &mut SessionWorld,
        simulator_id: SimulatorId,
        update: DecodedWorldUpdate,
        agent_id: Option<Uuid>,
    ) -> Option<(SimulatorId, IngestSummary)> {
        match update {
            DecodedWorldUpdate::Object(batch) => {
                let batch = self.filter_batch(world, simulator_id, agent_id, batch)?;
                let summary = world.apply_object_batch(simulator_id, batch);
                self.complete_batch(world, simulator_id, summary)
            }
            DecodedWorldUpdate::SitTarget { source, update } => {
                let object = update.object_id;
                let address = world.object_address(simulator_id, object);
                let local_id = address.map(|address| address.local_id().get());
                let result = world.apply_sit_target_update(simulator_id, source, update);
                match result {
                    Some(Ok(applied)) => {
                        if !applied.rejections.is_empty() {
                            tracing::debug!(simulator = simulator_id.get(), region = source.region().get(), object = %object, local_id, rejections = ?applied.rejections, "avatar sit response contained rejected metadata");
                        }
                        let mut summary = IngestSummary::new(IngestStatus::Applied);
                        summary.push_applied(applied, address);
                        Some((simulator_id, summary))
                    }
                    Some(Err(error)) => {
                        tracing::debug!(simulator = simulator_id.get(), region = source.region().get(), object = %object, local_id, %error, "could not apply avatar sit target");
                        let mut summary = IngestSummary::new(IngestStatus::Applied);
                        summary.push_error(error, address);
                        Some((simulator_id, summary))
                    }
                    None => None,
                }
            }
            DecodedWorldUpdate::AvatarAppearance { source, update } => {
                let object = update.object_id;
                let invalid_fields = update.invalid_fields;
                let address = world.object_address(simulator_id, object);
                let local_id = address.map(|address| address.local_id().get());
                match world.apply_avatar_appearance(simulator_id, source, update) {
                    Some(Ok(applied)) => {
                        if !applied.rejections.is_empty() {
                            tracing::debug!(simulator = simulator_id.get(), region = source.region().get(), object = %object, local_id, rejections = ?applied.rejections, "avatar appearance contained rejected fields");
                        }
                        let mut summary = IngestSummary::new(IngestStatus::Applied);
                        summary.invalid_fields = invalid_fields;
                        summary.push_applied(applied, address);
                        Some((simulator_id, summary))
                    }
                    Some(Err(error)) => {
                        tracing::debug!(simulator = simulator_id.get(), region = source.region().get(), object = %object, local_id, %error, "could not apply avatar appearance");
                        let mut summary = IngestSummary::new(IngestStatus::Applied);
                        summary.invalid_fields = invalid_fields;
                        summary.push_error(error, address);
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

    fn filter_batch(
        &mut self,
        world: &SessionWorld,
        simulator_id: SimulatorId,
        agent_id: Option<Uuid>,
        mut batch: ObjectBatch,
    ) -> Option<ObjectBatch> {
        if self.world_primary != Some(simulator_id)
            && let Some(agent_id) = agent_id
        {
            let agent_region = world.region_for(simulator_id)?;
            let mut retained = Vec::with_capacity(batch.blocks.len());
            for operation in batch.blocks.drain(..) {
                match operation {
                    Some(ObjectOperation::Observe(observation)) if observation.id == agent_id => {
                        let replace = self
                            .observations
                            .get(&simulator_id)
                            .is_none_or(|current| should_replace_handoff_observation(current, &observation));
                        if replace {
                            self.observations.insert(simulator_id, observation);
                        }
                    }
                    Some(operation) => match self.observations.get(&simulator_id) {
                        Some(observation)
                            if operation.address() == LocalAddress::new(agent_region, observation.local_id)
                                && conflicts_with_handoff_observation(&operation, observation) =>
                        {
                            if operation_is_newer(operation.source(), observation.source) {
                                self.observations.remove(&simulator_id);
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
            while self.observations.len() > MAX_HANDOFF_OBSERVATIONS {
                if let Some(stale) = self.observations.keys().next().copied() {
                    self.observations.remove(&stale);
                }
            }
        }
        Some(batch)
    }

    fn complete_batch(
        &mut self,
        world: &mut SessionWorld,
        simulator_id: SimulatorId,
        summary: Option<IngestSummary>,
    ) -> Option<(SimulatorId, IngestSummary)> {
        let mut summary = summary.map(|summary| (simulator_id, summary));
        if let Some(extra) = self.complete_observation(world, simulator_id) {
            let summary = summary.get_or_insert_with(|| (simulator_id, IngestSummary::new(IngestStatus::Applied)));
            summary.1.append(extra);
        }
        summary
    }

    fn complete_observation(&mut self, world: &mut SessionWorld, simulator_id: SimulatorId) -> Option<IngestSummary> {
        if let Some(observation) = self.observations.get(&simulator_id).cloned() {
            let (completed, applied) = self.complete_world_handoff(world, simulator_id, observation.clone());
            if completed {
                self.observations.remove(&simulator_id);
            }
            if let Some(applied) = applied {
                let mut summary = IngestSummary::new(IngestStatus::Applied);
                summary.push_applied(applied, Some(LocalAddress::new(observation.source.region(), observation.local_id)));
                return Some(summary);
            }
        }
        None
    }

    pub(super) fn begin(&mut self, source: SimulatorId, target: SimulatorId) -> bool {
        if source == target {
            return false;
        }
        if let Some(current) = self.promoted_target
            && current.target == source
            && self.world_primary == Some(current.source)
        {
            self.promoted_target = Some(PendingWorldHandoff { source: current.source, target });
            return true;
        }
        if self.world_primary != Some(source) {
            return false;
        }
        self.promoted_target = Some(PendingWorldHandoff { source, target });
        true
    }

    pub(super) fn promote(&mut self, world: &mut SessionWorld, source: SimulatorId, target: SimulatorId) -> Option<IngestSummary> {
        let previous_target = self.promoted_target.map(|handoff| handoff.target);
        if !self.begin(source, target) {
            return None;
        }
        if let Some(previous_target) = previous_target.filter(|previous_target| *previous_target != target) {
            self.observations.remove(&previous_target);
        }
        self.observations.get(&target).cloned().and_then(|observation| self.complete_promoted(world, target, observation).1)
    }

    pub(super) fn complete_promoted(
        &mut self,
        world: &mut SessionWorld,
        target: SimulatorId,
        observation: ObjectObservation,
    ) -> (bool, Option<IngestSummary>) {
        let address = LocalAddress::new(observation.source.region(), observation.local_id);
        let (completed, applied) = self.complete_world_handoff(world, target, observation);
        let summary = applied.map(|applied| {
            let mut summary = IngestSummary::new(IngestStatus::Applied);
            summary.push_applied(applied, Some(address));
            summary.log_incomplete_data(target, world.region_for(target));
            summary
        });
        (completed, summary)
    }

    fn complete_world_handoff(
        &mut self,
        world: &mut SessionWorld,
        target: SimulatorId,
        observation: ObjectObservation,
    ) -> (bool, Option<Applied>) {
        let Some(handoff) = self.promoted_target else { return (false, None) };
        if handoff.target != target || self.world_primary != Some(handoff.source) {
            return (false, None);
        }
        let (completed, applied) = world.complete_agent_handoff(handoff.source, target, observation);
        if completed {
            self.world_primary = Some(target);
            self.promoted_target = None;
        }
        (completed, applied)
    }
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
#[path = "events_tests.rs"]
mod tests;
