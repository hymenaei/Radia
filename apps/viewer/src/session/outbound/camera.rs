use std::{collections::VecDeque, time::Duration};

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
use crate::agent_movement::{BODY_ROTATION_DOT_THRESHOLD, FLY, MovementAction, MovementObservation};

const MIN_UPDATE_INTERVAL: Duration = Duration::from_millis(8);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(1);
const CAMERA_POSITION_THRESHOLD: f64 = 0.01;
const CAMERA_AXIS_THRESHOLD: f32 = 0.1f32.to_radians();
const MAX_PENDING_ACTIONS: usize = 16;

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
    pending_actions: VecDeque<MovementAction>,
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
            pending_actions: VecDeque::new(),
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

    pub(crate) fn request_action(&mut self, action: MovementAction) -> bool {
        if self.pending_actions.len() >= MAX_PENDING_ACTIONS {
            return false;
        }
        self.pending_actions.push_back(action);
        self.request_update();
        true
    }

    pub(crate) fn clear_flying(&self) {
        self.movement.send_if_modified(|current| {
            let Some(movement) = current else { return false };
            let previous = movement.control_flags;
            movement.control_flags &= !FLY;
            movement.control_flags != previous
        });
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

        let mut movement = movement.unwrap_or(MovementObservation {
            control_flags: 0,
            body_rotation: Quat::IDENTITY,
            always_run: false,
            stop_sequence: 0,
        });
        let action = self.pending_actions.front().copied();
        if let Some(action) = action {
            movement = movement.with_action(action);
        }
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
        let forced = previous.is_none() || context_changed || control_changed || action.is_some();
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
            let delivery = if action.is_some() { Delivery::Reliable } else { Delivery::Unreliable };
            match send(UplinkMessage::AgentUpdate(update), delivery) {
                Ok(()) => {
                    self.last_sent = Some(current);
                    self.last_sent_at = Some(now);
                    self.followup_at = control_changed.then_some(now + MIN_UPDATE_INTERVAL);
                    if action.is_some() {
                        tracing::debug!(
                            simulator = simulator.get(),
                            ?action,
                            flags = movement.control_flags,
                            "sent one-shot sit control flag"
                        );
                        self.pending_actions.pop_front();
                        self.pending_at = Some(now + MIN_UPDATE_INTERVAL);
                    }
                    if previous.is_none_or(|previous| {
                        previous.simulator != simulator
                            || previous.camera.is_none_or(|last_camera| last_camera.draw_distance() != camera.draw_distance())
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
#[path = "camera_tests.rs"]
mod tests;
