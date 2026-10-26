mod driver;
mod events;
mod input;
mod inventory_bootstrap;
mod inventory_intake;
mod media;
mod persistence;
mod presentation;

pub(crate) use self::driver::{CredentialPersistence, drain_session};
#[cfg(test)]
use self::input::select_drain_input;
pub(crate) use self::persistence::{WriteLock, save_favorite_store, write_lock};
pub(crate) use self::presentation::{
    AuthenticationUpdate, FavoritesUpdate, PresentationMailbox, PresentationSnapshot, SessionReadyUpdate, ViewerUpdate,
};

#[cfg(test)]
mod tests {
    use std::{
        future::{pending, ready},
        num::NonZeroU32,
        time::Duration,
    };

    use math::Vec3;
    use network::{Event, Event::SimulatorMessage, PacketOrder, PacketOrder::Ordered, SimulatorId};
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

    use crate::session::coordinator::events::WorldHandoff;
    use crate::session::coordinator::input::Input;
    use crate::session::world::{IngestSummary, SessionWorld, viewer_world_limits};
    use crate::test_helpers::simulator_id;
    use crate::world_reader::WorldReadHost;

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
        use crate::world_reader::MAX_INTAKE_TURNS;
        use futures::poll;
        use std::{collections::VecDeque, time::Instant};

        const EVENTS: u32 = 512;
        let simulator = simulator_id();
        let events = (1..=EVENTS).map(|sequence| agent_update(Uuid::from_u128(u128::from(sequence)), sequence, 0)).collect::<Vec<_>>();
        let mut baseline = SessionWorld::new(viewer_world_limits());
        baseline.register_simulator(simulator, None).unwrap();
        let mut baseline_handoff = WorldHandoff::default();
        let started = Instant::now();
        for (index, message) in events.iter().enumerate() {
            let event = simulator_message(simulator, index as u32 + 1, message.clone());
            apply_world_cooperatively(&mut baseline, &mut baseline_handoff, &event).await;
        }
        let baseline_time = started.elapsed();

        let mut world = SessionWorld::new(viewer_world_limits());
        world.register_simulator(simulator, None).unwrap();
        let mut reads = WorldReadHost::new(world.world(), Handle::current());
        let mut handoff = WorldHandoff::default();
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
            let input = super::select_drain_input(
                &mut reads,
                ready(Some(event)),
                pending(),
                pending(),
                pending(),
                pending(),
                pending(),
                pending(),
                pending(),
            )
            .await;
            match input {
                Input::Event(Some(event)) => {
                    apply_world_cooperatively(&mut world, &mut handoff, &event).await;
                    processed += 1;
                    since_read += 1;
                    assert!(since_read <= MAX_INTAKE_TURNS, "a ready read must receive its reserved turn");
                }
                Input::WorldRead(request) => {
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

    #[tokio::test]
    async fn camera_tick_is_selected_when_ready() {
        let mut camera_tick = tokio::time::interval(Duration::from_millis(100));
        camera_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let world = SessionWorld::new(viewer_world_limits());
        let mut reads = WorldReadHost::new(world.world(), Handle::current());

        let result = super::select_drain_input(
            &mut reads,
            pending(),
            pending(),
            pending(),
            pending(),
            pending(),
            camera_tick.tick(),
            pending(),
            pending(),
        )
        .await;

        let Input::CameraTick = result else { panic!("ready camera tick was not selected") };
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
        WorldHandoff::default().apply_event(world, event, None)
    }

    async fn apply_world_cooperatively(world: &mut SessionWorld, handoff: &mut WorldHandoff, event: &Event) {
        if let Event::SimulatorMessage { simulator_id, sequence, packet_order, message, .. } = event {
            let _ =
                handoff.apply_message_cooperatively(world, *simulator_id, *sequence, *packet_order, message, None, &mut |_, _| {}).await;
        } else {
            let _ = handoff.apply_event(world, event, None);
        }
    }
}
