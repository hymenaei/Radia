use std::time::Duration;

use math::{DVec3, Quat, Vec3};
use network::{AgentSession, Delivery, Error, SessionHandle, SimulatorId};
use protocol::messages::{AgentUpdate, SetAlwaysRun};
use render::scene::Camera;
use tokio::{
    sync::watch,
    time::{Instant, sleep_until},
};
use uuid::Uuid;

use super::agent;
use crate::agent_movement::{BODY_ROTATION_DOT_THRESHOLD, MovementObservation};

const MIN_UPDATE_INTERVAL: Duration = Duration::from_millis(8);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(1);
const CAMERA_POSITION_THRESHOLD: f64 = 0.01;
const CAMERA_AXIS_THRESHOLD: f32 = 0.1f32.to_radians();

pub(crate) struct CameraUplink {
    camera: watch::Sender<Option<Camera>>,
    observations: watch::Receiver<Option<Camera>>,
    movement: watch::Sender<Option<MovementObservation>>,
    movements: watch::Receiver<Option<MovementObservation>>,
    last_sent: Option<SentState>,
    last_camera: Option<LastCamera>,
    availability: Option<MovementAvailability>,
    last_sent_at: Option<Instant>,
    pending_at: Option<Instant>,
    followup_at: Option<Instant>,
    last_always_run: Option<(SimulatorId, Uuid, Uuid, bool)>,
    always_run_retry_at: Option<Instant>,
}

#[derive(Clone, Copy)]
struct SentState {
    simulator: SimulatorId,
    agent_id: Uuid,
    session_id: Uuid,
    origin: Option<DVec3>,
    camera: Option<Camera>,
    movement: MovementObservation,
}

#[derive(Clone, Copy)]
struct LastCamera {
    simulator: SimulatorId,
    agent_id: Uuid,
    session_id: Uuid,
    origin: DVec3,
    camera: Camera,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MovementAvailability {
    Available,
    Blocked(&'static str),
}

enum UplinkMessage {
    AgentUpdate(AgentUpdate),
    SetAlwaysRun(SetAlwaysRun),
}

impl CameraUplink {
    pub(crate) fn new() -> Self {
        let (camera, observations) = watch::channel(None);
        let (movement, movements) = watch::channel(None);
        Self {
            camera,
            observations,
            movement,
            movements,
            last_sent: None,
            last_camera: None,
            availability: None,
            last_sent_at: None,
            pending_at: None,
            followup_at: None,
            last_always_run: None,
            always_run_retry_at: None,
        }
    }

    pub(crate) fn sender(&self) -> watch::Sender<Option<Camera>> {
        self.camera.clone()
    }

    pub(crate) fn movement_sender(&self) -> watch::Sender<Option<MovementObservation>> {
        self.movement.clone()
    }

    pub(crate) fn request_update(&self) {
        self.movement.send_modify(|_| {});
    }

    pub(crate) fn region_origin_changed(&self, previous: Option<DVec3>, current: Option<DVec3>) {
        if previous != current {
            self.request_update();
        }
    }

    pub(crate) async fn next_tick(&mut self) -> Instant {
        let deadline = self.next_deadline();
        tokio::select! {
            _ = self.observations.changed() => Instant::now(),
            _ = self.movements.changed() => Instant::now(),
            _ = async {
                if let Some(deadline) = deadline {
                    sleep_until(deadline).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => Instant::now(),
        }
    }

    pub(crate) fn tick(
        &mut self,
        session_handle: &SessionHandle,
        simulator: Option<SimulatorId>,
        session: Option<&AgentSession>,
        origin: Option<DVec3>,
    ) {
        let camera = *self.observations.borrow_and_update();
        let movement = *self.movements.borrow_and_update();
        let destination = simulator
            .zip(session.map(|session| (session.agent_id, session.session_id)))
            .map(|(simulator, (agent_id, session_id))| (simulator, agent_id, session_id));
        self.tick_with(destination, origin, camera, movement, |message, delivery| match message {
            UplinkMessage::AgentUpdate(update) => session_handle.send(update, delivery),
            UplinkMessage::SetAlwaysRun(update) => session_handle.send(update, delivery),
        });
    }

    fn next_deadline(&self) -> Option<Instant> {
        let keepalive = self.last_sent_at.map(|sent| sent + KEEPALIVE_INTERVAL);
        [self.pending_at, self.followup_at, self.always_run_retry_at, keepalive].into_iter().flatten().min()
    }

    fn tick_with(
        &mut self,
        destination: Option<(SimulatorId, Uuid, Uuid)>,
        origin: Option<DVec3>,
        camera: Option<Camera>,
        movement: Option<MovementObservation>,
        mut send: impl FnMut(UplinkMessage, Delivery) -> Result<(), Error>,
    ) {
        let Some((simulator, agent_id, session_id)) = destination else {
            self.report_availability(MovementAvailability::Blocked("no simulator"));
            self.clear_destination();
            return;
        };
        let now = Instant::now();
        if let Some(movement) = movement {
            let state = (simulator, agent_id, session_id, movement.always_run);
            let retry_ready = self.always_run_retry_at.is_none_or(|deadline| now >= deadline);
            if self.last_always_run != Some(state) && retry_ready {
                match send(UplinkMessage::SetAlwaysRun(agent::always_run(agent_id, session_id, movement.always_run)), Delivery::Reliable) {
                    Ok(()) => {
                        self.last_always_run = Some(state);
                        self.always_run_retry_at = None;
                    }
                    Err(error) => {
                        self.always_run_retry_at = Some(now + MIN_UPDATE_INTERVAL);
                        tracing::debug!(%error, "always-run update deferred");
                    }
                }
            }
        }

        let movement = movement.unwrap_or(MovementObservation {
            control_flags: 0,
            body_rotation: Quat::IDENTITY,
            always_run: false,
            stop_sequence: 0,
        });
        if let (Some(camera), Some(origin)) = (camera, origin) {
            self.last_camera = Some(LastCamera { simulator, agent_id, session_id, origin, camera });
        }
        let previous = self.last_sent;
        let last_camera =
            self.last_camera.filter(|last| last.simulator == simulator && last.agent_id == agent_id && last.session_id == session_id);
        let camera = camera.or_else(|| last_camera.map(|last| last.camera));
        let origin = origin.or_else(|| last_camera.map(|last| last.origin));
        let (camera, origin) = match (camera, origin) {
            (Some(camera), Some(origin)) => (camera, origin),
            _ => {
                let reason = if camera.is_none() { "no agent object" } else { "no region" };
                self.report_availability(MovementAvailability::Blocked(reason));
                self.last_sent_at = None;
                self.pending_at = None;
                self.followup_at = None;
                return;
            }
        };
        self.report_availability(MovementAvailability::Available);
        let current = SentState { simulator, agent_id, session_id, origin: Some(origin), camera: Some(camera), movement };
        let context_changed = previous.is_none_or(|previous| {
            previous.simulator != simulator
                || previous.agent_id != agent_id
                || previous.session_id != session_id
                || previous.origin != Some(origin)
        });
        let control_changed = previous.is_none_or(|previous| {
            previous.movement.control_flags != movement.control_flags || previous.movement.stop_sequence != movement.stop_sequence
        });
        let camera_changed = previous.is_none_or(|previous| previous.camera.is_none_or(|previous| camera_changed(previous, camera)));
        let body_changed = previous.is_none_or(|previous| rotation_changed(previous.movement.body_rotation, movement.body_rotation));
        let any_change = context_changed || camera_changed || body_changed;
        let keepalive_due = self.last_sent_at.is_none_or(|sent| now.saturating_duration_since(sent) >= KEEPALIVE_INTERVAL);
        let followup_due = self.followup_at.is_some_and(|deadline| now >= deadline);
        let forced = previous.is_none() || context_changed || control_changed;
        let min_interval_elapsed = self.last_sent_at.is_none_or(|sent| now.saturating_duration_since(sent) >= MIN_UPDATE_INTERVAL);
        if !forced && any_change && !min_interval_elapsed {
            self.pending_at = self.last_sent_at.map(|sent| sent + MIN_UPDATE_INTERVAL);
            return;
        }
        if !forced && !any_change && !keepalive_due && !followup_due {
            self.pending_at = None;
            return;
        }
        self.pending_at = None;
        let update = agent::movement_camera(agent_id, session_id, camera, origin, movement);
        if let Some(update) = update {
            match send(UplinkMessage::AgentUpdate(update), Delivery::Unreliable) {
                Ok(()) => {
                    self.last_sent = Some(current);
                    self.last_sent_at = Some(now);
                    self.followup_at = control_changed.then_some(now + MIN_UPDATE_INTERVAL);
                    if previous.is_none_or(|previous| {
                        previous.simulator != simulator
                            || previous.camera.is_none_or(|previous| previous.draw_distance() != camera.draw_distance())
                    }) {
                        tracing::debug!(draw_distance = camera.draw_distance(), "simulator viewing distance updated");
                    }
                }
                Err(error) => {
                    self.last_sent = None;
                    self.last_sent_at = None;
                    self.pending_at = Some(now + MIN_UPDATE_INTERVAL);
                    tracing::debug!(%error, "agent update deferred");
                }
            }
        }
    }

    fn report_availability(&mut self, availability: MovementAvailability) {
        if self.availability != Some(availability) {
            self.availability = Some(availability);
            match availability {
                MovementAvailability::Available => tracing::info!(available = true, "movement controls available"),
                MovementAvailability::Blocked(reason) => tracing::info!(available = false, reason, "movement controls unavailable"),
            }
        }
    }

    fn clear_destination(&mut self) {
        self.last_sent = None;
        self.last_camera = None;
        self.last_sent_at = None;
        self.pending_at = None;
        self.followup_at = None;
        self.last_always_run = None;
        self.always_run_retry_at = None;
    }

    pub(crate) fn simulator_disconnected(&mut self, simulator: SimulatorId) {
        if self.last_camera.is_some_and(|camera| camera.simulator == simulator) {
            self.last_camera = None;
        }
        if self.last_sent.is_some_and(|sent| sent.simulator == simulator) {
            self.last_sent = None;
            self.last_sent_at = None;
            self.pending_at = None;
            self.followup_at = None;
        }
        if self.last_always_run.is_some_and(|(sent_simulator, ..)| sent_simulator == simulator) {
            self.last_always_run = None;
            self.always_run_retry_at = None;
        }
    }
}

fn camera_changed(previous: Camera, current: Camera) -> bool {
    let (previous_left, previous_up) = camera_axes(previous.direction());
    let (current_left, current_up) = camera_axes(current.direction());
    previous.position().distance(current.position()) > CAMERA_POSITION_THRESHOLD
        || previous.direction().distance(current.direction()) > CAMERA_AXIS_THRESHOLD
        || previous_left.distance(current_left) > CAMERA_AXIS_THRESHOLD
        || previous_up.distance(current_up) > CAMERA_AXIS_THRESHOLD
        || previous.draw_distance() != current.draw_distance()
}

fn camera_axes(direction: Vec3) -> (Vec3, Vec3) {
    let left = Vec3::Z.cross(direction).try_normalize().unwrap_or(Vec3::Y);
    (left, direction.cross(left))
}

fn rotation_changed(previous: Quat, current: Quat) -> bool {
    previous.dot(current).abs() < BODY_ROTATION_DOT_THRESHOLD
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, time::Duration};

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    use std::net::UdpSocket;

    use crate::test_helpers::simulator_id;
    use futures::FutureExt;
    use math::{DVec3, Quat, Vec3};
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    use network::Delivery;
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    use protocol::lludp::{Message, Packet};
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    use protocol::messages::SetAlwaysRun;
    use render::scene::Camera;
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    use tokio::net::UdpSocket as TokioUdpSocket;
    use tokio::time::{Instant, advance};
    use uuid::Uuid;
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    use winit::{
        event::ElementState,
        keyboard::{Key, NamedKey},
    };

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    use world::RegionHandle;

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    use crate::{
        agent_movement::{HeldControls, KeyBinding, MovementKey},
        camera::FollowCamera,
        session::world::{SessionWorld, viewer_world_limits},
    };

    use super::{CameraUplink, KEEPALIVE_INTERVAL, MIN_UPDATE_INTERVAL, UplinkMessage, camera_changed};

    fn view(position: DVec3, direction: Vec3) -> Camera {
        Camera::new(position, direction, 1.0, [720, 720]).expect("valid camera").with_quality(64.0, 1.0).expect("valid quality")
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn send_as_packet(
        socket: &UdpSocket,
        destination: std::net::SocketAddr,
        sequence: u32,
        message: UplinkMessage,
    ) -> Result<(), network::Error> {
        let message = match message {
            UplinkMessage::AgentUpdate(update) => Message::AgentUpdate(update),
            UplinkMessage::SetAlwaysRun(update) => Message::SetAlwaysRun(update),
        };
        let bytes = Packet::new(sequence, message).encode().map_err(|_| network::Error::SessionClosed)?;
        socket.send_to(&bytes, destination).map(|_| ()).map_err(|_| network::Error::SessionClosed)
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    async fn receive_packet(socket: &TokioUdpSocket) -> Message {
        let mut bytes = [0; 2048];
        let (length, _) = socket.recv_from(&mut bytes).await.expect("loopback update");
        Packet::decode(&bytes[..length]).expect("valid LLUDP packet").message().clone()
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn send_loopback(
        uplink: &mut CameraUplink,
        simulator: network::SimulatorId,
        agent_id: Uuid,
        session_id: Uuid,
        camera: impl Into<Option<Camera>>,
        movement: crate::agent_movement::MovementObservation,
        socket: &UdpSocket,
        destination: std::net::SocketAddr,
        sequence: &mut u32,
    ) {
        uplink.tick_with(Some((simulator, agent_id, session_id)), Some(DVec3::ZERO), camera.into(), Some(movement), |message, delivery| {
            let current = *sequence;
            *sequence += 1;
            let expected_delivery = match &message {
                UplinkMessage::AgentUpdate(_) => Delivery::Unreliable,
                UplinkMessage::SetAlwaysRun(_) => Delivery::Reliable,
            };
            assert_eq!(delivery, expected_delivery);
            send_as_packet(socket, destination, current, message)
        });
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[tokio::test]
    async fn loopback_simulator_receives_action_flags_camera_fields_and_focus_stop() {
        let receiver = TokioUdpSocket::bind("127.0.0.1:0").await.expect("bind loopback simulator");
        let sender = UdpSocket::bind("127.0.0.1:0").expect("bind loopback viewer");
        let destination = receiver.local_addr().expect("simulator address");
        let simulator = simulator_id();
        let (agent_id, session_id) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let mut sequence = 1;
        let mut uplink = CameraUplink::new();
        let actions = [
            (KeyBinding::Movement(MovementKey::Forward), false, 0x0000_0401),
            (KeyBinding::Movement(MovementKey::Backward), false, 0x0000_0402),
            (KeyBinding::Movement(MovementKey::TurnLeft), false, 0x0000_0100),
            (KeyBinding::Movement(MovementKey::TurnRight), false, 0x0000_0200),
            (KeyBinding::Movement(MovementKey::TurnLeft), true, 0x0000_0804),
            (KeyBinding::Movement(MovementKey::TurnRightArrow), true, 0x0000_0808),
            (KeyBinding::Movement(MovementKey::Jump), false, 0x0000_1010),
            (KeyBinding::Movement(MovementKey::Down), false, 0x0000_1020),
        ];

        for (index, (binding, shift, flags)) in actions.into_iter().enumerate() {
            let mut controls = HeldControls::default();
            let KeyBinding::Movement(key) = binding else { panic!("movement action") };
            controls.set_pressed(key, true);
            let movement = controls.observation(shift, Quat::IDENTITY);
            let position = DVec3::new(index as f64 + 1.0, 4.0, 8.0);
            let direction = if index.is_multiple_of(2) { Vec3::X } else { Vec3::Y };
            send_loopback(
                &mut uplink,
                simulator,
                agent_id,
                session_id,
                view(position, direction),
                movement,
                &sender,
                destination,
                &mut sequence,
            );
            let first = receive_packet(&receiver).await;
            let update = match first {
                Message::SetAlwaysRun(run) => {
                    assert_eq!(run, SetAlwaysRun { agent_id, session_id, always_run: false });
                    receive_packet(&receiver).await
                }
                message => message,
            };
            let Message::AgentUpdate(update) = update else { panic!("AgentUpdate for movement action") };
            assert_eq!(update.agent_data.control_flags, flags);
            assert_eq!(update.agent_data.camera_center, Vec3::new(position.x as f32, 4.0, 8.0));
            assert_eq!(update.agent_data.camera_at_axis, direction);
            assert_eq!(update.agent_data.camera_left_axis, Vec3::Z.cross(direction));
            assert_eq!(update.agent_data.camera_up_axis, direction.cross(Vec3::Z.cross(direction)));
            assert_eq!(update.agent_data.far, 64.0);
        }

        let mut controls = HeldControls::default();
        controls.toggle_flying();
        controls.toggle_run();
        let movement = controls.observation(false, Quat::IDENTITY);
        send_loopback(&mut uplink, simulator, agent_id, session_id, None::<Camera>, movement, &sender, destination, &mut sequence);
        let Message::SetAlwaysRun(run) = receive_packet(&receiver).await else { panic!("always-run toggle") };
        assert_eq!(run, SetAlwaysRun { agent_id, session_id, always_run: true });
        let Message::AgentUpdate(update) = receive_packet(&receiver).await else { panic!("AgentUpdate after toggles") };
        assert_eq!(update.agent_data.control_flags, 0x0000_2000);
        assert_eq!(update.agent_data.camera_center, Vec3::new(8.0, 4.0, 8.0));
        assert_eq!(update.agent_data.camera_at_axis, Vec3::Y);
        assert_eq!(update.agent_data.camera_left_axis, -Vec3::X);
        assert_eq!(update.agent_data.camera_up_axis, Vec3::Z);

        controls.set_pressed(MovementKey::Forward, true);
        let movement = controls.observation(false, Quat::IDENTITY);
        send_loopback(&mut uplink, simulator, agent_id, session_id, None::<Camera>, movement, &sender, destination, &mut sequence);
        let Message::AgentUpdate(_) = receive_packet(&receiver).await else { panic!("AgentUpdate before focus loss") };
        let movement = controls.release_and_observe(Quat::IDENTITY);
        send_loopback(&mut uplink, simulator, agent_id, session_id, None::<Camera>, movement, &sender, destination, &mut sequence);
        let Message::AgentUpdate(update) = receive_packet(&receiver).await else { panic!("focus-loss AgentUpdate") };
        assert_eq!(update.agent_data.control_flags, 0x0000_6000);
        let movement = controls.observation(false, Quat::IDENTITY);
        send_loopback(&mut uplink, simulator, agent_id, session_id, None::<Camera>, movement, &sender, destination, &mut sequence);
        let Message::AgentUpdate(update) = receive_packet(&receiver).await else { panic!("released controls update") };
        assert_eq!(update.agent_data.control_flags, 0x0000_2000);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn space_stop_reaches_agent_update() {
        let simulator = simulator_id();
        let agent_id = Uuid::from_u128(1);
        let session_id = Uuid::from_u128(2);
        let camera = view(DVec3::ZERO, Vec3::X);
        let mut controls = HeldControls::default();
        assert_eq!(
            controls.keyboard_event(&Key::Named(NamedKey::Space), ElementState::Pressed, false, false),
            Some((KeyBinding::Stop, true))
        );
        let mut uplink = CameraUplink::new();
        uplink.movement_sender().send_replace(Some(controls.observation(false, Quat::IDENTITY)));
        let movement = *uplink.movements.borrow_and_update();
        let mut sent = Vec::new();

        uplink.tick_with(Some((simulator, agent_id, session_id)), Some(DVec3::ZERO), Some(camera), movement, |message, _| {
            if let UplinkMessage::AgentUpdate(update) = message {
                sent.push(update);
            }
            Ok(())
        });

        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].agent_data.control_flags & crate::agent_movement::STOP, crate::agent_movement::STOP);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn redundant_wake_does_not_send_a_duplicate_agent_update() {
        let simulator = simulator_id();
        let (agent_id, session_id) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let camera = view(DVec3::ZERO, Vec3::X);
        let movement = crate::agent_movement::MovementObservation {
            control_flags: 0,
            body_rotation: Quat::IDENTITY,
            always_run: false,
            stop_sequence: 0,
        };
        let mut uplink = CameraUplink::new();
        let mut sent = Vec::new();
        uplink.tick_with(Some((simulator, agent_id, session_id)), Some(DVec3::ZERO), Some(camera), Some(movement), |message, _| {
            if let UplinkMessage::AgentUpdate(update) = message {
                sent.push(update);
            }
            Ok(())
        });
        assert_eq!(sent.len(), 1);

        uplink.request_update();
        uplink.tick_with(Some((simulator, agent_id, session_id)), Some(DVec3::ZERO), Some(camera), Some(movement), |message, _| {
            if let UplinkMessage::AgentUpdate(update) = message {
                sent.push(update);
            }
            Ok(())
        });
        assert_eq!(sent.len(), 1, "a redundant wake must not duplicate the current state");
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[tokio::test(start_paused = true)]
    async fn visible_region_origin_wakes_live_camera_and_forwards_held_key() {
        let receiver = TokioUdpSocket::bind("127.0.0.1:0").await.expect("bind loopback simulator");
        let sender = UdpSocket::bind("127.0.0.1:0").expect("bind loopback viewer");
        let destination = receiver.local_addr().expect("simulator address");
        let simulator = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        let region = world.register_simulator(simulator, None).expect("register primary simulator region");
        assert_eq!(world.world().region(region).and_then(|info| info.resolved_origin()), None);

        let (agent_id, session_id) = (Uuid::from_u128(1), Uuid::from_u128(2));
        assert_eq!(world.world().entity(agent_id), None, "the self avatar has not resolved in World");

        let mut controls = HeldControls::default();
        let (binding, changed) =
            controls.keyboard_event(&Key::Character("w".into()), ElementState::Pressed, false, false).expect("forward key binding");
        assert_eq!(binding, KeyBinding::Movement(MovementKey::Forward));
        assert!(changed);
        let movement = controls.observation(false, Quat::IDENTITY);
        let mut uplink = CameraUplink::new();
        let mut sequence = 1;
        uplink.tick_with(Some((simulator, agent_id, session_id)), None, None, Some(movement), |message, delivery| {
            let current = sequence;
            sequence += 1;
            assert_eq!(
                delivery,
                match &message {
                    UplinkMessage::AgentUpdate(_) => Delivery::Unreliable,
                    UplinkMessage::SetAlwaysRun(_) => Delivery::Reliable,
                }
            );
            send_as_packet(&sender, destination, current, message)
        });
        let Message::SetAlwaysRun(_) = receive_packet(&receiver).await else { panic!("initial always-run state") };

        let handle = RegionHandle::new((256_u64 << 32) | 512);
        world.register_simulator(simulator, Some(handle)).expect("resolve visible region handle");
        let resolved_origin = world.world().region(region).and_then(|info| info.resolved_origin()).expect("visible region origin");
        let origin = DVec3::new(256.0, 512.0, 0.0);
        assert_eq!(resolved_origin, origin);
        uplink.region_origin_changed(None, Some(resolved_origin));
        uplink.tick_with(Some((simulator, agent_id, session_id)), Some(resolved_origin), None, Some(movement), |message, _| {
            if let UplinkMessage::AgentUpdate(update) = message {
                panic!("movement cannot be sent before the viewport consumes its camera seed: {update:?}");
            }
            Ok(())
        });

        assert_eq!(world.world().entity(agent_id), None, "the visible region still has no self-avatar World pose");
        let pose = world::WorldPose { position: origin + DVec3::new(128.0, 128.0, 24.0), rotation: Quat::IDENTITY };
        let camera = FollowCamera::scene_camera(pose, [720, 720], 64.0, 1.0).expect("seeded follow camera");
        uplink.camera.send_replace(Some(camera));

        let mut sent = Vec::new();
        let camera = *uplink.observations.borrow_and_update();
        uplink.tick_with(Some((simulator, agent_id, session_id)), Some(resolved_origin), camera, Some(movement), |message, delivery| {
            if let UplinkMessage::AgentUpdate(update) = &message {
                assert_eq!(delivery, Delivery::Unreliable);
                sent.push(update.clone());
            }
            let current = sequence;
            sequence += 1;
            send_as_packet(&sender, destination, current, message)
        });
        let Message::AgentUpdate(update) = receive_packet(&receiver).await else { panic!("held-forward AgentUpdate") };
        assert_eq!(update.agent_data.control_flags & 0x0000_0001, 0x0000_0001);
        assert_eq!(update.agent_data.camera_center, (camera.expect("published follow camera").position() - resolved_origin).as_vec3());
        assert_eq!(update.agent_data.camera_at_axis, camera.expect("published follow camera").direction());
        assert_eq!(sent.len(), 1);

        advance(MIN_UPDATE_INTERVAL).await;
        uplink.tick_with(Some((simulator, agent_id, session_id)), Some(resolved_origin), camera, Some(movement), |message, delivery| {
            if let UplinkMessage::AgentUpdate(update) = &message {
                assert_eq!(delivery, Delivery::Unreliable);
                sent.push(update.clone());
            }
            let current = sequence;
            sequence += 1;
            send_as_packet(&sender, destination, current, message)
        });
        let Message::AgentUpdate(follow_up) = receive_packet(&receiver).await else { panic!("cadence follow-up AgentUpdate") };
        assert_eq!(follow_up.agent_data.control_flags & 0x0000_0001, 0x0000_0001);
        assert_eq!(sent.len(), 2);

        advance(Duration::from_millis(50)).await;
        uplink.tick_with(Some((simulator, agent_id, session_id)), Some(resolved_origin), camera, Some(movement), |message, delivery| {
            if let UplinkMessage::AgentUpdate(update) = &message {
                assert_eq!(delivery, Delivery::Unreliable);
                sent.push(update.clone());
            }
            let current = sequence;
            sequence += 1;
            send_as_packet(&sender, destination, current, message)
        });
        assert_eq!(sent.len(), 2, "held controls do not repeat before the one-second keepalive");

        let (_, changed) =
            controls.keyboard_event(&Key::Character("w".into()), ElementState::Released, false, false).expect("forward key release");
        assert!(changed);
        let released = controls.observation(false, Quat::IDENTITY);
        assert_eq!(released.control_flags, 0);
        uplink.tick_with(Some((simulator, agent_id, session_id)), Some(resolved_origin), camera, Some(released), |message, delivery| {
            if let UplinkMessage::AgentUpdate(update) = &message {
                assert_eq!(delivery, Delivery::Unreliable);
                sent.push(update.clone());
            }
            let current = sequence;
            sequence += 1;
            send_as_packet(&sender, destination, current, message)
        });
        let Message::AgentUpdate(release) = receive_packet(&receiver).await else { panic!("neutral release AgentUpdate") };
        assert_eq!(release.agent_data.control_flags, 0);
        assert_eq!(sent.len(), 3);

        advance(MIN_UPDATE_INTERVAL).await;
        uplink.tick_with(Some((simulator, agent_id, session_id)), Some(resolved_origin), camera, Some(released), |message, delivery| {
            if let UplinkMessage::AgentUpdate(update) = &message {
                assert_eq!(delivery, Delivery::Unreliable);
                sent.push(update.clone());
            }
            let current = sequence;
            sequence += 1;
            send_as_packet(&sender, destination, current, message)
        });
        let Message::AgentUpdate(release_follow_up) = receive_packet(&receiver).await else { panic!("neutral release follow-up") };
        assert_eq!(release_follow_up.agent_data.control_flags, 0);
        assert_eq!(sent.len(), 4);
    }
    #[tokio::test(start_paused = true)]
    async fn controls_send_on_change_and_keepalive_after_one_second() {
        let simulator = simulator_id();
        let mut uplink = CameraUplink::new();
        let camera = view(DVec3::ZERO, Vec3::X);
        let mut sent = Vec::new();
        let movement = crate::agent_movement::MovementObservation {
            control_flags: 0,
            body_rotation: Quat::IDENTITY,
            always_run: false,
            stop_sequence: 0,
        };
        uplink.tick_with(Some((simulator, Uuid::nil(), Uuid::nil())), Some(DVec3::ZERO), Some(camera), Some(movement), |message, _| {
            if let UplinkMessage::AgentUpdate(update) = message {
                sent.push(update);
            }
            Ok(())
        });
        let mut changed = movement;
        changed.control_flags = 0x0000_0401;
        uplink.tick_with(Some((simulator, Uuid::nil(), Uuid::nil())), Some(DVec3::ZERO), Some(camera), Some(changed), |message, _| {
            if let UplinkMessage::AgentUpdate(update) = message {
                sent.push(update);
            }
            Ok(())
        });
        assert_eq!(sent.len(), 2, "a control change bypasses the regular minimum period");
        advance(MIN_UPDATE_INTERVAL).await;
        uplink.tick_with(Some((simulator, Uuid::nil(), Uuid::nil())), Some(DVec3::ZERO), Some(camera), Some(changed), |message, _| {
            if let UplinkMessage::AgentUpdate(update) = message {
                sent.push(update);
            }
            Ok(())
        });
        assert_eq!(sent.len(), 3, "a control change schedules its protocol follow-up");
        advance(KEEPALIVE_INTERVAL - Duration::from_millis(1)).await;
        uplink.tick_with(Some((simulator, Uuid::nil(), Uuid::nil())), Some(DVec3::ZERO), Some(camera), Some(changed), |message, _| {
            if let UplinkMessage::AgentUpdate(update) = message {
                sent.push(update);
            }
            Ok(())
        });
        assert_eq!(sent.len(), 3);
        advance(Duration::from_millis(1)).await;
        uplink.tick_with(Some((simulator, Uuid::nil(), Uuid::nil())), Some(DVec3::ZERO), Some(camera), Some(changed), |message, _| {
            if let UplinkMessage::AgentUpdate(update) = message {
                sent.push(update);
            }
            Ok(())
        });
        assert_eq!(sent.len(), 4);

        let mut released = changed;
        released.control_flags = 0;
        uplink.tick_with(Some((simulator, Uuid::nil(), Uuid::nil())), Some(DVec3::ZERO), Some(camera), Some(released), |message, _| {
            if let UplinkMessage::AgentUpdate(update) = message {
                sent.push(update);
            }
            Ok(())
        });
        assert_eq!(sent.len(), 5, "releasing movement sends neutral controls immediately");
        advance(MIN_UPDATE_INTERVAL).await;
        uplink.tick_with(Some((simulator, Uuid::nil(), Uuid::nil())), Some(DVec3::ZERO), Some(camera), Some(released), |message, _| {
            if let UplinkMessage::AgentUpdate(update) = message {
                sent.push(update);
            }
            Ok(())
        });
        assert_eq!(sent.len(), 6, "neutral controls keep their protocol follow-up");
        advance(KEEPALIVE_INTERVAL - Duration::from_millis(1)).await;
        uplink.tick_with(Some((simulator, Uuid::nil(), Uuid::nil())), Some(DVec3::ZERO), Some(camera), Some(released), |message, _| {
            if let UplinkMessage::AgentUpdate(update) = message {
                sent.push(update);
            }
            Ok(())
        });
        assert_eq!(sent.len(), 6, "idle controls keep the one-second interval");
        advance(Duration::from_millis(1)).await;
        uplink.tick_with(Some((simulator, Uuid::nil(), Uuid::nil())), Some(DVec3::ZERO), Some(camera), Some(released), |message, _| {
            if let UplinkMessage::AgentUpdate(update) = message {
                sent.push(update);
            }
            Ok(())
        });
        assert_eq!(sent.len(), 7);
    }

    #[tokio::test(start_paused = true)]
    async fn disconnected_uplink_suspends_keepalive_until_reconnect() {
        let simulator = simulator_id();
        let destination = Some((simulator, Uuid::nil(), Uuid::nil()));
        let camera = view(DVec3::ZERO, Vec3::X);
        let movement = crate::agent_movement::MovementObservation {
            control_flags: 0,
            body_rotation: Quat::IDENTITY,
            always_run: false,
            stop_sequence: 0,
        };
        let mut uplink = CameraUplink::new();
        let sent = Cell::new(0);
        let mut send = |_, _| {
            sent.set(sent.get() + 1);
            Ok(())
        };

        uplink.tick_with(destination, Some(DVec3::ZERO), Some(camera), Some(movement), &mut send);
        assert_eq!(sent.get(), 2, "initial destination sends run state and movement");
        assert!(uplink.next_deadline().is_some());
        advance(KEEPALIVE_INTERVAL).await;
        assert!(uplink.next_tick().now_or_never().is_some(), "the keepalive deadline is ready");

        uplink.tick_with(None, Some(DVec3::ZERO), Some(camera), Some(movement), &mut send);
        assert_eq!(uplink.next_deadline(), None);
        assert!(uplink.next_tick().now_or_never().is_none(), "disconnect must not leave a ready timer ahead of session events");

        uplink.request_update();
        assert!(uplink.next_tick().now_or_never().is_some(), "a newly connected primary wakes the uplink");
        uplink.tick_with(destination, Some(DVec3::ZERO), Some(camera), Some(movement), &mut send);
        assert_eq!(sent.get(), 4, "the current state is resent when a destination returns");
    }

    #[tokio::test(start_paused = true)]
    async fn camera_change_waits_for_eight_millisecond_limit() {
        let simulator = simulator_id();
        let mut uplink = CameraUplink::new();
        let movement = crate::agent_movement::MovementObservation {
            control_flags: 0,
            body_rotation: Quat::IDENTITY,
            always_run: false,
            stop_sequence: 0,
        };
        let mut sent = Vec::new();
        uplink.tick_with(
            Some((simulator, Uuid::nil(), Uuid::nil())),
            Some(DVec3::ZERO),
            Some(view(DVec3::ZERO, Vec3::X)),
            Some(movement),
            |message, _| {
                if let UplinkMessage::AgentUpdate(update) = message {
                    sent.push(update);
                }
                Ok(())
            },
        );
        uplink.tick_with(
            Some((simulator, Uuid::nil(), Uuid::nil())),
            Some(DVec3::ZERO),
            Some(view(DVec3::new(1.0, 0.0, 0.0), Vec3::X)),
            Some(movement),
            |message, _| {
                if let UplinkMessage::AgentUpdate(update) = message {
                    sent.push(update);
                }
                Ok(())
            },
        );
        assert_eq!(sent.len(), 1);
        advance(MIN_UPDATE_INTERVAL - Duration::from_millis(1)).await;
        assert_eq!(uplink.next_deadline(), Some(Instant::now() + Duration::from_millis(1)));
        advance(Duration::from_millis(1)).await;
        uplink.tick_with(
            Some((simulator, Uuid::nil(), Uuid::nil())),
            Some(DVec3::ZERO),
            Some(view(DVec3::new(1.0, 0.0, 0.0), Vec3::X)),
            Some(movement),
            |message, _| {
                if let UplinkMessage::AgentUpdate(update) = message {
                    sent.push(update);
                }
                Ok(())
            },
        );
        assert_eq!(sent.len(), 2);
    }

    #[test]
    fn camera_direction_uses_official_axis_threshold() {
        let first = view(DVec3::ZERO, Vec3::X);
        let below_threshold = view(DVec3::ZERO, Vec3::new(1.0, 0.0015, 0.0));
        let above_threshold = view(DVec3::ZERO, Vec3::new(1.0, 0.002, 0.0));
        assert!(!camera_changed(first, below_threshold));
        assert!(camera_changed(first, above_threshold));
    }
}
