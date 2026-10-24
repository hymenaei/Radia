use std::time::Duration;

use math::DVec3;
use network::{AgentSession, Delivery, Error, SessionHandle, SimulatorId};
use protocol::messages::{AgentMovementComplete, AgentUpdate};
use render::scene::Camera;
use tokio::{
    sync::watch,
    time::{Instant, Interval, MissedTickBehavior},
};
use uuid::Uuid;

use super::agent;

const CAMERA_UPDATE_INTERVAL: Duration = Duration::from_millis(100);
const CAMERA_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

pub(crate) struct CameraUplink {
    camera: watch::Sender<Option<Camera>>,
    observations: watch::Receiver<Option<Camera>>,
    last_sent: Option<(SimulatorId, DVec3, Camera)>,
    last_sent_at: Option<Instant>,
    interval: Interval,
}

impl CameraUplink {
    pub(crate) fn new() -> Self {
        let (camera, observations) = watch::channel(None);
        let mut interval = tokio::time::interval(CAMERA_UPDATE_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        Self { camera, observations, last_sent: None, last_sent_at: None, interval }
    }

    pub(crate) fn sender(&self) -> watch::Sender<Option<Camera>> {
        self.camera.clone()
    }

    pub(crate) async fn next_tick(&mut self) -> Instant {
        self.interval.tick().await
    }

    pub(crate) fn tick(
        &mut self,
        session_handle: &SessionHandle,
        simulator: Option<SimulatorId>,
        session: Option<&AgentSession>,
        origin: Option<DVec3>,
    ) {
        let (Some(simulator), Some(session), Some(origin)) = (simulator, session, origin) else { return };
        self.tick_with(simulator, session.agent_id, session.session_id, origin, |update| session_handle.send(update, Delivery::Unreliable));
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

    fn tick_with(
        &mut self,
        simulator: SimulatorId,
        agent_id: Uuid,
        session_id: Uuid,
        origin: DVec3,
        send: impl FnOnce(AgentUpdate) -> Result<(), Error>,
    ) {
        let Some(view) = *self.observations.borrow() else { return };
        let state = (simulator, origin, view);
        let interval = if self.last_sent == Some(state) { CAMERA_HEARTBEAT_INTERVAL } else { CAMERA_UPDATE_INTERVAL };
        let now = Instant::now();
        if self.last_sent_at.is_none_or(|sent| now.saturating_duration_since(sent) >= interval)
            && let Some(update) = agent::camera(agent_id, session_id, view, origin)
        {
            self.last_sent_at = Some(now);
            match send(update) {
                Ok(()) => {
                    if self
                        .last_sent
                        .is_none_or(|(previous, _, camera)| previous != simulator || camera.draw_distance() != view.draw_distance())
                    {
                        tracing::debug!(draw_distance = view.draw_distance(), "simulator viewing distance updated");
                    }
                    self.last_sent = Some(state);
                }
                Err(error) => {
                    self.last_sent = None;
                    tracing::debug!(%error, "camera update deferred");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use math::Vec3;
    use tokio::time::advance;

    use crate::test_helpers::simulator_id;

    use super::*;

    fn view(x: f64) -> Camera {
        Camera::new(DVec3::new(x, 0.0, 0.0), Vec3::X, 1.0, [720, 720]).expect("valid camera")
    }

    #[tokio::test(start_paused = true)]
    async fn change_waits_one_update_interval() {
        let simulator = simulator_id();
        let mut uplink = CameraUplink::new();
        uplink.camera.send_replace(Some(view(0.0)));
        let mut sent = Vec::new();
        uplink.next_tick().await;
        uplink.tick_with(simulator, Uuid::nil(), Uuid::nil(), DVec3::ZERO, |update| {
            sent.push(update);
            Ok(())
        });
        uplink.camera.send_replace(Some(view(1.0)));

        {
            let tick = uplink.next_tick();
            tokio::pin!(tick);
            assert!(futures::poll!(tick.as_mut()).is_pending());
            advance(CAMERA_UPDATE_INTERVAL - Duration::from_millis(1)).await;
            assert!(futures::poll!(tick.as_mut()).is_pending());
            advance(Duration::from_millis(1)).await;
            tick.await;
        }
        uplink.tick_with(simulator, Uuid::nil(), Uuid::nil(), DVec3::ZERO, |update| {
            sent.push(update);
            Ok(())
        });

        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1].agent_data.camera_center, Vec3::new(1.0, 0.0, 0.0));
    }

    #[tokio::test(start_paused = true)]
    async fn unchanged_uses_heartbeat_interval() {
        let simulator = simulator_id();
        let mut uplink = CameraUplink::new();
        uplink.camera.send_replace(Some(view(0.0)));
        let mut sent = Vec::new();
        uplink.tick_with(simulator, Uuid::nil(), Uuid::nil(), DVec3::ZERO, |update| {
            sent.push(update);
            Ok(())
        });

        advance(CAMERA_HEARTBEAT_INTERVAL - Duration::from_millis(1)).await;
        uplink.tick_with(simulator, Uuid::nil(), Uuid::nil(), DVec3::ZERO, |update| {
            sent.push(update);
            Ok(())
        });
        assert_eq!(sent.len(), 1);

        advance(Duration::from_millis(1)).await;
        uplink.tick_with(simulator, Uuid::nil(), Uuid::nil(), DVec3::ZERO, |update| {
            sent.push(update);
            Ok(())
        });
        assert_eq!(sent.len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_send_resets_sent_state() {
        let simulator = simulator_id();
        let mut uplink = CameraUplink::new();
        uplink.camera.send_replace(Some(view(0.0)));
        let mut attempts = 0;
        uplink.tick_with(simulator, Uuid::nil(), Uuid::nil(), DVec3::ZERO, |_| {
            attempts += 1;
            Err(Error::SessionClosed)
        });

        advance(CAMERA_UPDATE_INTERVAL).await;
        uplink.tick_with(simulator, Uuid::nil(), Uuid::nil(), DVec3::ZERO, |_| {
            attempts += 1;
            Ok(())
        });

        assert_eq!(attempts, 2);
    }
}
