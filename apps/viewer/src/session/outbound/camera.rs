use std::time::Duration;

use math::{DVec3, Quat, Vec3};
use network::{AgentSession, Delivery, Error, SessionHandle, SimulatorId};
use protocol::messages::{AgentMovementComplete, AgentUpdate, SetAlwaysRun};
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
    origin: DVec3,
    camera: Camera,
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

    pub(crate) fn on_movement_complete(
        &self,
        session_handle: &SessionHandle,
        primary_simulator: Option<SimulatorId>,
        simulator_id: SimulatorId,
        session: Option<&AgentSession>,
        movement: &AgentMovementComplete,
    ) {
        if primary_simulator != Some(simulator_id) || self.observations.borrow().is_some() {
            return;
        }
        let Some(session) = session else { return };
        if let Some(update) = agent::arrival(session.agent_id, session.session_id, movement)
            && let Err(error) = session_handle.send(update, Delivery::Reliable)
        {
            tracing::debug!(%error, "arrival camera update deferred");
        }
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
        let movement_changed = previous.is_none_or(|previous| {
            previous.movement.control_flags != movement.control_flags
                || previous.movement.stop_sequence != movement.stop_sequence
                || rotation_changed(previous.movement.body_rotation, movement.body_rotation)
        });
        let last_camera =
            self.last_camera.filter(|last| last.simulator == simulator && last.agent_id == agent_id && last.session_id == session_id);
        let camera = camera.or_else(|| movement_changed.then(|| last_camera.map(|last| last.camera)).flatten());
        let origin = origin.or_else(|| movement_changed.then(|| last_camera.map(|last| last.origin)).flatten());
        let (Some(camera), Some(origin)) = (camera, origin) else {
            self.last_sent_at = None;
            self.pending_at = None;
            self.followup_at = None;
            return;
        };
        let current = SentState { simulator, agent_id, session_id, origin, camera, movement };
        let context_changed = previous.is_none_or(|previous| {
            previous.simulator != simulator
                || previous.agent_id != agent_id
                || previous.session_id != session_id
                || previous.origin != origin
        });
        let control_changed = previous.is_none_or(|previous| {
            previous.movement.control_flags != movement.control_flags || previous.movement.stop_sequence != movement.stop_sequence
        });
        let camera_changed = previous.is_none_or(|previous| camera_changed(previous.camera, camera));
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
        if let Some(update) = agent::movement_camera(agent_id, session_id, camera, origin, movement) {
            match send(UplinkMessage::AgentUpdate(update), Delivery::Unreliable) {
                Ok(()) => {
                    self.last_sent = Some(current);
                    self.last_sent_at = Some(now);
                    self.followup_at = control_changed.then_some(now + MIN_UPDATE_INTERVAL);
                    if previous
                        .is_none_or(|previous| previous.simulator != simulator || previous.camera.draw_distance() != camera.draw_distance())
                    {
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

    fn clear_destination(&mut self) {
        self.last_sent = None;
        self.last_camera = None;
        self.last_sent_at = None;
        self.pending_at = None;
        self.followup_at = None;
        self.last_always_run = None;
        self.always_run_retry_at = None;
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
    use std::{cell::Cell, net::UdpSocket, time::Duration};

    use futures::FutureExt;
    use math::{DVec3, Quat, Vec3};
    use network::Delivery;
    use protocol::lludp::{Message, Packet};
    use protocol::messages::SetAlwaysRun;
    use render::scene::Camera;
    use tokio::{
        net::UdpSocket as TokioUdpSocket,
        time::{Instant, advance},
    };
    use uuid::Uuid;

    use crate::{
        agent_movement::{HeldControls, KeyBinding, MovementKey},
        test_helpers::simulator_id,
    };

    use super::{CameraUplink, KEEPALIVE_INTERVAL, MIN_UPDATE_INTERVAL, UplinkMessage, camera_changed};

    fn view(position: DVec3, direction: Vec3) -> Camera {
        Camera::new(position, direction, 1.0, [720, 720]).expect("valid camera").with_quality(64.0, 1.0).expect("valid quality")
    }

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

    async fn receive_packet(socket: &TokioUdpSocket) -> Message {
        let mut bytes = [0; 2048];
        let (length, _) = socket.recv_from(&mut bytes).await.expect("loopback update");
        Packet::decode(&bytes[..length]).expect("valid LLUDP packet").message().clone()
    }

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
