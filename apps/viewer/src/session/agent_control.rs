use std::{error::Error, fmt, future::pending, time::Duration};

use math::Vec3;
use network::{Delivery, SessionHandle, SimulatorId};
use protocol::{
    lludp::{FieldValue, Message},
    messages::{
        AgentRequestSit, AgentSit,
        agent::agent_request_sit::{AgentData as SitAgentData, TargetObject},
    },
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{Instant, sleep_until},
};
use uuid::Uuid;

use crate::{agent_movement::MovementAction, session::inbound::decode_sit_response};

const COMMAND_CAPACITY: usize = 16;
pub(crate) const SIT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentControlError {
    InvalidObject,
    InvalidOffset,
    Busy,
    QueueFull,
    SessionEnded,
    SimulatorUnavailable,
    SendFailed,
}

impl fmt::Display for AgentControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidObject => "the sit target is invalid",
            Self::InvalidOffset => "the sit offset is invalid",
            Self::Busy => "a sit request is already pending",
            Self::QueueFull => "the agent control queue is full",
            Self::SessionEnded => "the agent session ended",
            Self::SimulatorUnavailable => "the primary simulator is unavailable",
            Self::SendFailed => "the simulator did not accept the request",
        })
    }
}

impl Error for AgentControlError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SitOutcome {
    Accepted { auto_pilot: bool, force_mouselook: bool },
    Refused,
    TimedOut,
    Cancelled,
    SimulatorChanged,
}

#[derive(Clone)]
pub struct AgentControlHandle {
    commands: mpsc::Sender<Command>,
}

impl AgentControlHandle {
    pub async fn sit_on_object(&self, object_id: Uuid, offset: Vec3) -> Result<SitOutcome, AgentControlError> {
        if object_id.is_nil() {
            return Err(AgentControlError::InvalidObject);
        }
        if !offset.is_finite() {
            return Err(AgentControlError::InvalidOffset);
        }
        let (reply, response) = oneshot::channel();
        self.try_send(Command::SitOnObject { object_id, offset, reply })?;
        response.await.map_err(|_| AgentControlError::SessionEnded)?
    }

    pub async fn sit_on_ground(&self) -> Result<(), AgentControlError> {
        let (reply, response) = oneshot::channel();
        self.try_send(Command::SitOnGround { reply })?;
        response.await.map_err(|_| AgentControlError::SessionEnded)?
    }

    pub async fn stand_up(&self) -> Result<(), AgentControlError> {
        let (reply, response) = oneshot::channel();
        self.try_send(Command::StandUp { reply })?;
        response.await.map_err(|_| AgentControlError::SessionEnded)?
    }

    fn try_send(&self, command: Command) -> Result<(), AgentControlError> {
        self.commands.try_send(command).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => AgentControlError::QueueFull,
            mpsc::error::TrySendError::Closed(_) => AgentControlError::SessionEnded,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SitCameraOverride {
    pub(crate) eye_offset: Vec3,
    pub(crate) at_offset: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SitResponseState {
    pub(crate) object_id: Uuid,
    pub(crate) accepted: bool,
    pub(crate) camera_override: Option<SitCameraOverride>,
}

#[derive(Debug)]
pub(crate) enum AgentControlEvent {
    Command(Option<Command>),
    SitTimedOut,
}

#[derive(Debug)]
pub(crate) enum Command {
    SitOnObject { object_id: Uuid, offset: Vec3, reply: oneshot::Sender<Result<SitOutcome, AgentControlError>> },
    SitOnGround { reply: oneshot::Sender<Result<(), AgentControlError>> },
    StandUp { reply: oneshot::Sender<Result<(), AgentControlError>> },
}

struct PendingSit {
    object_id: Uuid,
    simulator: SimulatorId,
    deadline: Instant,
    reply: oneshot::Sender<Result<SitOutcome, AgentControlError>>,
}

pub(crate) struct AgentControlState {
    commands: mpsc::Receiver<Command>,
    pending_sit: Option<PendingSit>,
    sit_response: watch::Sender<Option<SitResponseState>>,
}

impl AgentControlState {
    pub(crate) fn new() -> (AgentControlHandle, Self, watch::Receiver<Option<SitResponseState>>) {
        let (sender, commands) = mpsc::channel(COMMAND_CAPACITY);
        let (sit_response, sit_updates) = watch::channel(None);
        (AgentControlHandle { commands: sender }, Self { commands, pending_sit: None, sit_response }, sit_updates)
    }

    pub(crate) async fn next_event(&mut self) -> AgentControlEvent {
        let deadline = self.pending_sit.as_ref().map(|pending| pending.deadline);
        let timeout = async move {
            match deadline {
                Some(deadline) => sleep_until(deadline).await,
                None => pending().await,
            }
        };
        tokio::select! {
            biased;
            command = self.commands.recv() => AgentControlEvent::Command(command),
            _ = timeout => AgentControlEvent::SitTimedOut,
        }
    }

    pub(crate) fn begin_sit(
        &mut self,
        object_id: Uuid,
        simulator: SimulatorId,
        reply: oneshot::Sender<Result<SitOutcome, AgentControlError>>,
    ) {
        self.pending_sit = Some(PendingSit { object_id, simulator, deadline: Instant::now() + SIT_RESPONSE_TIMEOUT, reply });
    }

    pub(crate) fn has_pending_sit(&self) -> bool {
        self.pending_sit.is_some()
    }

    pub(crate) fn expects_response(&self, simulator: SimulatorId, object_id: Uuid) -> bool {
        self.pending_sit.as_ref().is_some_and(|pending| pending.simulator == simulator && pending.object_id == object_id)
    }

    pub(crate) fn accept_sit(
        &mut self,
        simulator: SimulatorId,
        object_id: Uuid,
        auto_pilot: bool,
        force_mouselook: bool,
        camera_override: Option<SitCameraOverride>,
    ) -> bool {
        if !self.pending_sit.as_ref().is_some_and(|pending| pending.simulator == simulator && pending.object_id == object_id) {
            return false;
        }
        self.set_sit_response(Some(SitResponseState { object_id, accepted: true, camera_override }));
        if let Some(pending) = self.pending_sit.take()
            && pending.reply.send(Ok(SitOutcome::Accepted { auto_pilot, force_mouselook })).is_err()
        {
            tracing::trace!("sit caller stopped before receiving acceptance");
        }
        true
    }

    pub(crate) fn refuse_sit(&mut self, simulator: SimulatorId) -> bool {
        self.complete_sit(simulator, Ok(SitOutcome::Refused))
    }

    pub(crate) fn cancel_sit(&mut self, outcome: SitOutcome) {
        if let Some(pending) = self.pending_sit.take()
            && pending.reply.send(Ok(outcome)).is_err()
        {
            tracing::trace!("sit caller stopped before receiving cancellation");
        }
    }

    pub(crate) fn timeout_sit(&mut self) {
        if self.pending_sit.as_ref().is_some_and(|pending| Instant::now() >= pending.deadline) {
            self.cancel_sit(SitOutcome::TimedOut);
        }
    }

    pub(crate) fn fail_sit(&mut self, error: AgentControlError) {
        if let Some(pending) = self.pending_sit.take()
            && pending.reply.send(Err(error)).is_err()
        {
            tracing::trace!("sit caller stopped before receiving failure");
        }
    }

    pub(crate) fn cancel_if_simulator_changed(&mut self, simulator: SimulatorId) {
        if self.pending_sit.as_ref().is_some_and(|pending| pending.simulator != simulator) {
            self.cancel_sit(SitOutcome::SimulatorChanged);
        }
    }

    pub(crate) fn clear_sit_acceptance(&self) {
        self.sit_response.send_if_modified(|previous| {
            let Some(mut response) = *previous else { return false };
            if !response.accepted {
                return false;
            }
            response.accepted = false;
            *previous = response.camera_override.is_some().then_some(response);
            true
        });
    }

    fn complete_sit(&mut self, simulator: SimulatorId, outcome: Result<SitOutcome, AgentControlError>) -> bool {
        if !self.pending_sit.as_ref().is_some_and(|pending| pending.simulator == simulator) {
            return false;
        }
        if let Some(pending) = self.pending_sit.take()
            && pending.reply.send(outcome).is_err()
        {
            tracing::trace!("sit caller stopped before receiving outcome");
        }
        true
    }

    fn set_sit_response(&self, response: Option<SitResponseState>) {
        self.sit_response.send_if_modified(|previous| {
            if *previous == response {
                false
            } else {
                *previous = response;
                true
            }
        });
    }
}

pub(super) trait SitTransport {
    fn send_request(&self, request: AgentRequestSit, delivery: Delivery) -> Result<(), network::Error>;

    fn send_sit(&self, sit: AgentSit, delivery: Delivery) -> Result<(), network::Error>;
}

impl SitTransport for SessionHandle {
    fn send_request(&self, request: AgentRequestSit, delivery: Delivery) -> Result<(), network::Error> {
        self.send(request, delivery)
    }

    fn send_sit(&self, sit: AgentSit, delivery: Delivery) -> Result<(), network::Error> {
        self.send(sit, delivery)
    }
}

pub(super) fn process_agent_control_command<T: SitTransport>(
    transport: &T,
    agent_control: &mut AgentControlState,
    camera: &mut crate::session::CameraUplink,
    primary_simulator: Option<SimulatorId>,
    active_agent: Option<(Uuid, Uuid)>,
    command: Command,
) {
    match command {
        Command::SitOnObject { object_id, offset, reply } => {
            if agent_control.has_pending_sit() {
                if reply.send(Err(AgentControlError::Busy)).is_err() {
                    tracing::trace!("sit caller stopped before receiving the busy outcome");
                }
                return;
            }
            let (Some(simulator), Some((agent_id, session_id))) = (primary_simulator, active_agent) else {
                if reply.send(Err(AgentControlError::SimulatorUnavailable)).is_err() {
                    tracing::trace!("sit caller stopped before receiving the simulator outcome");
                }
                return;
            };
            let request = AgentRequestSit {
                agent_data: SitAgentData { agent_id, session_id },
                target_object: TargetObject { target_id: object_id, offset },
            };
            match transport.send_request(request, Delivery::Reliable) {
                Ok(()) => agent_control.begin_sit(object_id, simulator, reply),
                Err(error) => {
                    tracing::debug!(simulator = simulator.get(), %error, "sit request could not be queued");
                    if reply.send(Err(AgentControlError::SendFailed)).is_err() {
                        tracing::trace!("sit caller stopped before receiving the send outcome");
                    }
                }
            }
        }
        Command::SitOnGround { reply } => {
            if primary_simulator.is_none() || active_agent.is_none() {
                if reply.send(Err(AgentControlError::SimulatorUnavailable)).is_err() {
                    tracing::trace!("ground-sit caller stopped before receiving the simulator outcome");
                }
                return;
            }
            agent_control.cancel_sit(SitOutcome::Cancelled);
            agent_control.clear_sit_acceptance();
            camera.request_action(MovementAction::SitOnGround);
            if reply.send(Ok(())).is_err() {
                tracing::trace!("ground-sit caller stopped before receiving the queued outcome");
            }
        }
        Command::StandUp { reply } => {
            if primary_simulator.is_none() || active_agent.is_none() {
                if reply.send(Err(AgentControlError::SimulatorUnavailable)).is_err() {
                    tracing::trace!("stand-up caller stopped before receiving the simulator outcome");
                }
                return;
            }
            agent_control.cancel_sit(SitOutcome::Cancelled);
            agent_control.clear_sit_acceptance();
            camera.request_action(MovementAction::StandUp);
            if reply.send(Ok(())).is_err() {
                tracing::trace!("stand-up caller stopped before receiving the queued outcome");
            }
        }
    }
}

pub(super) fn process_sit_response<T: SitTransport>(
    transport: &T,
    agent_control: &mut AgentControlState,
    simulator: SimulatorId,
    active_agent: Option<(Uuid, Uuid)>,
    message: &Message,
) {
    let Some(response) = decode_sit_response(message) else { return };
    if !agent_control.expects_response(simulator, response.object_id) {
        return;
    }
    let Some((agent_id, session_id)) = active_agent else {
        agent_control.fail_sit(AgentControlError::SimulatorUnavailable);
        return;
    };
    let sit = AgentSit { agent_id, session_id };
    match transport.send_sit(sit, Delivery::Reliable) {
        Ok(()) => {
            agent_control.accept_sit(
                simulator,
                response.object_id,
                response.auto_pilot,
                response.force_mouselook,
                response.camera_override,
            );
        }
        Err(error) => {
            tracing::debug!(simulator = simulator.get(), %error, "sit acceptance could not be queued");
            agent_control.fail_sit(AgentControlError::SendFailed);
        }
    }
}

pub(super) fn is_sit_refusal_alert(message: &Message) -> bool {
    let Message::Schema(alert) = message else { return false };
    if message.name() != "AlertMessage" {
        return false;
    }
    let Some(FieldValue::Bytes(message)) =
        alert.blocks().first().and_then(|block| block.instances.first()).and_then(|instance| instance.fields.first())
    else {
        return false;
    };
    std::str::from_utf8(message).is_ok_and(|message| message.trim_end_matches('\0') == "SitFailed")
}

pub(super) fn handle_sit_alert(
    agent_control: &mut AgentControlState,
    primary_simulator: Option<SimulatorId>,
    simulator: SimulatorId,
    message: &Message,
) -> bool {
    primary_simulator == Some(simulator) && is_sit_refusal_alert(message) && agent_control.refuse_sit(simulator)
}

#[cfg(test)]
mod tests {
    use math::Vec3;
    use tokio::{sync::oneshot, time::advance};
    use uuid::Uuid;

    use crate::test_helpers::simulator_id;

    use super::{AgentControlEvent, AgentControlState, Command, SIT_RESPONSE_TIMEOUT, SitCameraOverride, SitOutcome, SitResponseState};

    #[tokio::test(start_paused = true)]
    async fn sit_request_times_out() {
        let (handle, mut state, _) = AgentControlState::new();
        let object_id = Uuid::from_u128(3);
        let simulator = simulator_id();
        let call = tokio::spawn(async move { handle.sit_on_object(object_id, Vec3::ZERO).await });
        let AgentControlEvent::Command(Some(Command::SitOnObject { reply, .. })) = state.next_event().await else {
            panic!("object-sit command")
        };
        state.begin_sit(object_id, simulator, reply);
        advance(SIT_RESPONSE_TIMEOUT).await;
        std::assert_matches!(state.next_event().await, AgentControlEvent::SitTimedOut);
        state.timeout_sit();
        assert_eq!(call.await.expect("sit task"), Ok(SitOutcome::TimedOut));
    }

    #[tokio::test]
    async fn stand_cancels_pending_sit() {
        let (handle, mut state, _) = AgentControlState::new();
        let object_id = Uuid::from_u128(3);
        let simulator = simulator_id();
        let sit = tokio::spawn({
            let handle = handle.clone();
            async move { handle.sit_on_object(object_id, Vec3::ZERO).await }
        });
        let AgentControlEvent::Command(Some(Command::SitOnObject { reply, .. })) = state.next_event().await else {
            panic!("object-sit command")
        };
        state.begin_sit(object_id, simulator, reply);
        let stand = tokio::spawn(async move { handle.stand_up().await });
        let AgentControlEvent::Command(Some(Command::StandUp { reply })) = state.next_event().await else { panic!("stand-up command") };
        state.cancel_sit(SitOutcome::Cancelled);
        assert!(reply.send(Ok(())).is_ok());
        assert_eq!(sit.await.expect("sit task"), Ok(SitOutcome::Cancelled));
        assert_eq!(stand.await.expect("stand task"), Ok(()));
    }

    #[test]
    fn stand_keeps_camera_override() {
        let (_, mut state, response) = AgentControlState::new();
        let (reply, _result) = oneshot::channel();
        let simulator = simulator_id();
        let object_id = Uuid::from_u128(3);
        let camera_override = SitCameraOverride { eye_offset: Vec3::X, at_offset: Vec3::Y };
        state.begin_sit(object_id, simulator, reply);
        assert!(state.accept_sit(simulator, object_id, true, false, Some(camera_override)));

        assert_eq!(*response.borrow(), Some(SitResponseState { object_id, accepted: true, camera_override: Some(camera_override) }));
        state.clear_sit_acceptance();
        assert_eq!(*response.borrow(), Some(SitResponseState { object_id, accepted: false, camera_override: Some(camera_override) }));
    }

    #[test]
    fn dropped_reply_clears_pending_sit() {
        let (_, mut state, _) = AgentControlState::new();
        let (reply, response) = oneshot::channel();
        let simulator = simulator_id();
        let object_id = Uuid::from_u128(3);
        state.begin_sit(object_id, simulator, reply);
        drop(response);

        std::assert_matches!(state.accept_sit(simulator, object_id, true, false, None), true);
        std::assert_matches!(state.has_pending_sit(), false);
    }

    #[test]
    fn invalid_sit_target_is_rejected() {
        let (handle, _, _) = AgentControlState::new();
        assert_eq!(
            futures::executor::block_on(handle.sit_on_object(Uuid::nil(), Vec3::ZERO)),
            Err(super::AgentControlError::InvalidObject)
        );
        assert_eq!(
            futures::executor::block_on(handle.sit_on_object(Uuid::from_u128(3), Vec3::splat(f32::NAN))),
            Err(super::AgentControlError::InvalidOffset)
        );
    }
}
