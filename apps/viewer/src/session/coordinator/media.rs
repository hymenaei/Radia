use std::{
    collections::{HashMap, VecDeque},
    future::pending,
};

use network::{SimulatorId, capabilities::Client};
use world::Entity;

use crate::session::{
    object_media::{ObjectMediaFetcher, ObjectMediaResult, ObjectMediaTarget, response_is_stale},
    world::{MAX_WORLD_ENTITIES, SessionWorld},
};

const MAX_WAITING_OBJECT_MEDIA: usize = MAX_WORLD_ENTITIES;
const MAX_MEDIA_FLUSH_BATCH: usize = 64;

#[derive(Default)]
pub(super) struct WaitingObjectMedia {
    order: VecDeque<Entity>,
    targets: HashMap<Entity, ObjectMediaTarget>,
}

impl WaitingObjectMedia {
    fn len(&self) -> usize {
        self.targets.len()
    }

    fn update(&mut self, target: ObjectMediaTarget) -> bool {
        let Some(current) = self.targets.get_mut(&target.binding.entity()) else { return false };
        let same_binding = current.simulator_id == target.simulator_id && current.binding == target.binding;
        if !same_binding || target.is_newer_than(*current) {
            *current = target;
        }
        true
    }

    fn insert(&mut self, target: ObjectMediaTarget) -> bool {
        if self.update(target) {
            return true;
        }
        if self.len() >= MAX_WAITING_OBJECT_MEDIA {
            return false;
        }
        self.order.push_back(target.binding.entity());
        self.targets.insert(target.binding.entity(), target);
        true
    }

    fn forget_simulator(&mut self, simulator_id: SimulatorId) {
        self.targets.retain(|_, target| target.simulator_id != simulator_id);
        self.order.retain(|entity| self.targets.contains_key(entity));
    }

    fn pop_front(&mut self) -> Option<ObjectMediaTarget> {
        while let Some(entity) = self.order.pop_front() {
            if let Some(target) = self.targets.remove(&entity) {
                return Some(target);
            }
        }
        None
    }

    fn push_front(&mut self, target: ObjectMediaTarget) {
        self.order.push_front(target.binding.entity());
        self.targets.insert(target.binding.entity(), target);
    }

    fn push_back(&mut self, target: ObjectMediaTarget) {
        self.order.push_back(target.binding.entity());
        self.targets.insert(target.binding.entity(), target);
    }
}

pub(super) enum ObjectMediaIntake {
    Enabled { fetcher: ObjectMediaFetcher, clients: HashMap<SimulatorId, Client>, waiting: WaitingObjectMedia },
    Disabled,
}

impl ObjectMediaIntake {
    pub(super) fn new(fetcher: ObjectMediaFetcher) -> Self {
        Self::Enabled { fetcher, clients: HashMap::new(), waiting: WaitingObjectMedia::default() }
    }

    pub(super) async fn next_result(&mut self) -> Option<ObjectMediaResult> {
        match self {
            Self::Enabled { fetcher, .. } => fetcher.next_result().await,
            Self::Disabled => pending().await,
        }
    }

    pub(super) fn disable(&mut self) {
        *self = Self::Disabled;
    }

    pub(super) fn set_client(&mut self, simulator_id: SimulatorId, client: Client) {
        if let Self::Enabled { clients, .. } = self {
            clients.insert(simulator_id, client);
        }
    }

    pub(super) fn finish(&mut self, result: &ObjectMediaResult) -> Option<ObjectMediaTarget> {
        match self {
            Self::Enabled { fetcher, .. } => fetcher.finish(result),
            Self::Disabled => None,
        }
    }

    pub(super) fn forget_simulator(&mut self, simulator_id: SimulatorId) {
        let Self::Enabled { fetcher, clients, waiting } = self else { return };
        clients.remove(&simulator_id);
        fetcher.forget_simulator(simulator_id);
        waiting.forget_simulator(simulator_id);
    }

    pub(super) fn queue(&mut self, target: ObjectMediaTarget, world: &SessionWorld) {
        let Self::Enabled { fetcher, clients, waiting } = self else { return };
        if !world.object_media_target_is_current(target) {
            return;
        }
        if waiting.update(target) {
            return;
        }
        if let Some(client) = clients.get(&target.simulator_id)
            && fetcher.enqueue(target, client.clone())
        {
            return;
        }
        if !waiting.insert(target) {
            tracing::debug!(
                simulator = target.simulator_id.get(),
                region = target.binding.address().region().get(),
                object = %target.binding.object_id(),
                local_id = target.binding.address().local_id().get(),
                "object media queue reached the live object limit"
            );
        }
    }

    pub(super) fn apply_result(&mut self, result: ObjectMediaResult, world: &mut SessionWorld) {
        let retry = self.finish(&result);
        let requested_version = retry.map_or(result.target.version, |target| target.version);
        let response_version = result.update.as_ref().as_ref().ok().map(|update| update.version());
        if response_version.is_some_and(|version| response_is_stale(version, requested_version)) {
            tracing::debug!(
                simulator = result.target.simulator_id.get(),
                region = result.target.binding.address().region().get(),
                object = %result.target.binding.object_id(),
                local_id = result.target.binding.address().local_id().get(),
                response_version,
                requested_version,
                "discarded superseded object media response"
            );
        } else {
            match result.update {
                Ok(update) => {
                    let address = result.target.binding.address();
                    if let Some(applied) = world.apply_object_media(result.target, update) {
                        match applied {
                            Ok(applied) => {
                                if !applied.rejections.is_empty() {
                                    tracing::debug!(
                                        simulator = result.target.simulator_id.get(),
                                        region = address.region().get(),
                                        object = %result.target.binding.object_id(),
                                        local_id = address.local_id().get(),
                                        rejections = ?applied.rejections,
                                        "object media update contained rejected fields"
                                    );
                                }
                            }
                            Err(error) => {
                                tracing::debug!(
                                    simulator = result.target.simulator_id.get(),
                                    region = address.region().get(),
                                    object = %result.target.binding.object_id(),
                                    local_id = address.local_id().get(),
                                    %error,
                                    "object media update was rejected by world"
                                );
                            }
                        }
                    }
                }
                Err(error) if world.object_media_target_is_current(result.target) => {
                    tracing::debug!(
                        simulator = result.target.simulator_id.get(),
                        region = result.target.binding.address().region().get(),
                        object = %result.target.binding.object_id(),
                        local_id = result.target.binding.address().local_id().get(),
                        ?error,
                        "object media could not be loaded"
                    );
                }
                Err(error) => {
                    tracing::debug!(
                        simulator = result.target.simulator_id.get(),
                        region = result.target.binding.address().region().get(),
                        object = %result.target.binding.object_id(),
                        local_id = result.target.binding.address().local_id().get(),
                        ?error,
                        "discarded object media failure for a stale object target"
                    );
                }
            }
        }
        if let Some(target) = retry {
            self.queue(target, world);
        }
    }

    pub(super) fn flush(&mut self, world: &SessionWorld) {
        let Self::Enabled { fetcher, clients, waiting } = self else { return };
        let attempts = waiting.len().min(MAX_MEDIA_FLUSH_BATCH);
        for _ in 0..attempts {
            let Some(target) = waiting.pop_front() else { break };
            if !world.object_media_target_is_current(target) {
                continue;
            }
            let Some(client) = clients.get(&target.simulator_id) else {
                waiting.push_back(target);
                continue;
            };
            if !fetcher.enqueue(target, client.clone()) {
                waiting.push_front(target);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::{pending, ready},
        num::NonZeroU32,
    };

    use futures::{StreamExt, channel::oneshot, stream};
    use network::{Event, SimulatorId};
    use protocol::{lludp::Message::KillObject, messages::object::kill_object};
    use tokio::runtime::Handle;

    use crate::test_helpers::simulator_id;
    use uuid::Uuid;
    use world::{Appearance, ApplyOutcome::Introduced, FieldUpdate, Kind, ObjectObservation, OrderEvidence, ParentUpdate, UpdateSource};

    use super::*;
    use crate::session::coordinator::input::{self, select_drain_input};
    use crate::session::coordinator::tests::apply_world_event;
    use crate::session::coordinator::tests::simulator_message;
    use crate::session::world::{SessionWorld, viewer_world_limits};
    use crate::session::{
        inbound::{ObjectBatch, ObjectOperation},
        object_media::{FetchError, ObjectMediaFetcher, ObjectMediaTarget},
    };
    use crate::world_reader::WorldReadHost;

    fn object_media_target(world: &mut SessionWorld, simulator_id: SimulatorId, order: u32, version: u32) -> ObjectMediaTarget {
        let region = world.register_simulator(simulator_id, None).expect("register simulator");
        let local_id = NonZeroU32::new(7).expect("nonzero local ID");
        let source = UpdateSource::new(region, order, OrderEvidence::Ordered(u64::from(order)), 0);
        let observation = ObjectObservation {
            appearance: FieldUpdate::Set(Appearance::Surface),
            ..ObjectObservation::new(source, Uuid::from_u128(7), local_id, Kind::Object, ParentUpdate::Root)
        };
        let summary = world
            .apply_object_batch(
                simulator_id,
                ObjectBatch {
                    region,
                    region_handle: None,
                    blocks: vec![Some(ObjectOperation::Observe(observation))],
                    invalid_blocks: 0,
                    invalid_fields: 0,
                },
            )
            .expect("object batch applies");
        let result = summary.blocks[0].applied().expect("object is valid");
        let Introduced(entity) = &result.outcome else { panic!("object is introduced") };
        let binding = world.world().object_binding_target(*entity).expect("current object binding");
        ObjectMediaTarget { simulator_id, binding, source, version }
    }

    #[tokio::test]
    async fn media_exit_preserves_intake() {
        let simulator = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        let mut reads = WorldReadHost::new(world.world(), Handle::current());
        let target = object_media_target(&mut world, simulator, 1, 1);
        let mut media = ObjectMediaIntake::new(ObjectMediaFetcher::new(Handle::current(), ready(())));
        media.queue(target, &world);
        match &media {
            ObjectMediaIntake::Enabled { waiting, .. } if waiting.len() == 1 => {}
            _ => panic!("expected one waiting media target"),
        }
        assert!(media.next_result().await.is_none());

        media.disable();
        media.queue(target, &world);
        match &media {
            ObjectMediaIntake::Disabled => {}
            _ => panic!("disabled fetching releases queued work and rejects new work"),
        }
        let event = simulator_message(
            simulator,
            2,
            KillObject(kill_object::KillObject {
                object_data: vec![kill_object::ObjectData { local_id: target.binding.address().local_id().get() }],
            }),
        );
        let input =
            select_drain_input(&mut reads, ready(Some(event)), media.next_result(), pending(), pending(), pending(), pending(), pending())
                .await;
        let input::Input::Event(Some(event)) = input else { panic!("network intake continues after media failure") };
        let _ = apply_world_event(&mut world, &event).expect("object removal is applied");
        assert!(world.world().object(target.binding.entity()).is_none());

        let input = select_drain_input(
            &mut reads,
            pending(),
            media.next_result(),
            pending(),
            pending(),
            pending(),
            ready(tokio::time::Instant::now()),
            pending(),
        )
        .await;
        let input::Input::CameraTick = input else { panic!("ready camera tick was not selected") };
        let input =
            select_drain_input(&mut reads, pending(), media.next_result(), pending(), pending(), ready(()), pending(), pending()).await;
        let input::Input::Cancelled = input else { panic!("cancellation was not selected") };
    }

    #[test]
    fn waiting_media_coalesces_by_entity() {
        let simulator_id = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        let old = object_media_target(&mut world, simulator_id, 1, 1);
        let newer =
            ObjectMediaTarget { source: UpdateSource::new(old.source.region(), 2, OrderEvidence::Ordered(2), 0), version: 2, ..old };
        let later_marker_with_lower_version =
            ObjectMediaTarget { source: UpdateSource::new(old.source.region(), 3, OrderEvidence::Ordered(3), 0), version: 1, ..old };
        let mut waiting = WaitingObjectMedia::default();

        assert!(waiting.insert(old));
        assert!(waiting.insert(newer));
        assert!(waiting.insert(later_marker_with_lower_version));
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting.pop_front(), Some(newer));
        assert_eq!(waiting.pop_front(), None);
    }

    #[test]
    fn rebound_objects_ignore_media_failures() {
        let simulator_id = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        let target = object_media_target(&mut world, simulator_id, 1, 1);
        assert!(world.object_media_target_is_current(target));

        let source = UpdateSource::new(target.source.region(), 2, OrderEvidence::Ordered(2), 0);
        world
            .apply_object_batch(
                simulator_id,
                ObjectBatch {
                    region: target.source.region(),
                    region_handle: None,
                    blocks: vec![Some(ObjectOperation::Remove { source, local_id: target.binding.address().local_id() })],
                    invalid_blocks: 0,
                    invalid_fields: 0,
                },
            )
            .expect("remove object");

        assert!(!world.object_media_target_is_current(target));
    }

    #[tokio::test]
    async fn media_progresses_during_continuous_events() {
        let simulator_id = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        let mut reads = WorldReadHost::new(world.world(), Handle::current());
        let target = object_media_target(&mut world, simulator_id, 1, 1);
        let expected_entity = target.binding.entity();
        let (media_ready, media_gate) = oneshot::channel();
        let media = stream::once(async move {
            media_gate.await.expect("first event releases the queued media result");
            ObjectMediaResult { target, update: Err(FetchError::Transport) }
        })
        .chain(stream::pending());
        futures::pin_mut!(media);
        let endpoint = "127.0.0.1:13000".parse().expect("endpoint");
        let mut media_ready = Some(media_ready);
        let mut event_count = 0;

        let media_serviced = async {
            while event_count < 128 {
                match select_drain_input(
                    &mut reads,
                    std::future::ready(Some(Event::SimulatorConnected { simulator_id, region_handle: None, endpoint })),
                    media.next(),
                    std::future::pending(),
                    std::future::pending(),
                    std::future::pending(),
                    std::future::pending(),
                    std::future::pending(),
                )
                .await
                {
                    input::Input::Media(Some(result)) => {
                        assert_eq!(result.target.binding.entity(), expected_entity);
                        return true;
                    }
                    input::Input::Event(Some(_)) => {
                        event_count += 1;
                        if event_count == 1 {
                            media_ready.take().expect("media gate is released once").send(()).expect("media receiver is waiting");
                        }
                    }
                    _ => panic!("unexpected intake result"),
                }
            }
            false
        }
        .await;

        assert!(media_serviced, "a queued media response should progress while simulator events remain ready");
        assert!(event_count < 128, "the event stream must remain ready until media is serviced");
    }
}
