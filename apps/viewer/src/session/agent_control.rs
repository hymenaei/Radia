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
use world::{Parent, World};

use crate::{agent_movement::MovementAction, session::inbound::decode_sit_response};

const COMMAND_CAPACITY: usize = 16;
pub(crate) const SIT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentControlError {
    InvalidObject,
    InvalidOffset,
    InvalidSitResponse,
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
            Self::InvalidSitResponse => "the simulator returned an invalid sit target",
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
    pub(crate) generation: u64,
    pub(crate) object_id: Uuid,
    pub(crate) requested_object_id: Uuid,
    pub(crate) camera_override: Option<SitCameraOverride>,
    pub(crate) force_mouselook: bool,
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
    next_sit_response_generation: u64,
    sit_response: watch::Sender<Option<SitResponseState>>,
}

impl AgentControlState {
    pub(crate) fn new() -> (AgentControlHandle, Self, watch::Receiver<Option<SitResponseState>>) {
        let (sender, commands) = mpsc::channel(COMMAND_CAPACITY);
        let (sit_response, sit_updates) = watch::channel(None);
        (
            AgentControlHandle { commands: sender },
            Self { commands, pending_sit: None, next_sit_response_generation: 0, sit_response },
            sit_updates,
        )
    }

    pub(crate) async fn next_event(&mut self) -> AgentControlEvent {
        let deadline = self.pending_sit.as_ref().map(|pending| pending.deadline);
        let timeout = async move {
            match deadline {
                Some(deadline) => sleep_until(deadline).await,
                None => pending().await,
            }
        };
        let command = tokio::select! {
            biased;
            _ = timeout => return AgentControlEvent::SitTimedOut,
            command = self.commands.recv() => command,
        };
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            AgentControlEvent::SitTimedOut
        } else {
            AgentControlEvent::Command(command)
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

    pub(crate) fn expects_response(&self, world: &World, simulator: SimulatorId, object_id: Uuid) -> bool {
        self.pending_sit
            .as_ref()
            .is_some_and(|pending| pending.simulator == simulator && same_sit_linkset(world, pending.object_id, object_id))
    }

    pub(crate) fn accept_sit(
        &mut self,
        simulator: SimulatorId,
        object_id: Uuid,
        auto_pilot: bool,
        force_mouselook: bool,
        camera_override: Option<SitCameraOverride>,
    ) -> bool {
        let Some(pending) = self.pending_sit.as_ref().filter(|pending| pending.simulator == simulator) else {
            return false;
        };
        let requested_object_id = pending.object_id;
        let generation = self.next_sit_response_generation;
        self.next_sit_response_generation = generation.wrapping_add(1);
        self.set_sit_response(Some(SitResponseState { generation, object_id, requested_object_id, camera_override, force_mouselook }));
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
        if let Some(pending) = self.pending_sit.take() {
            tracing::debug!(simulator = pending.simulator.get(), object = %pending.object_id, ?outcome, "sit outcome completed");
            if pending.reply.send(Ok(outcome)).is_err() {
                tracing::trace!("sit caller stopped before receiving cancellation");
            }
        }
    }

    pub(crate) fn timeout_sit(&mut self) {
        if self.pending_sit.as_ref().is_some_and(|pending| Instant::now() >= pending.deadline) {
            self.cancel_sit(SitOutcome::TimedOut);
        }
    }

    pub(crate) fn fail_sit(&mut self, error: AgentControlError) {
        if let Some(pending) = self.pending_sit.take() {
            tracing::debug!(simulator = pending.simulator.get(), object = %pending.object_id, %error, "sit outcome failed");
            if pending.reply.send(Err(error)).is_err() {
                tracing::trace!("sit caller stopped before receiving failure");
            }
        }
    }

    pub(crate) fn cancel_if_simulator_changed(&mut self, simulator: SimulatorId) {
        if self.pending_sit.as_ref().is_some_and(|pending| pending.simulator != simulator) {
            self.cancel_sit(SitOutcome::SimulatorChanged);
        }
    }

    pub(crate) fn clear_sit_response_without_camera(&self) {
        self.sit_response.send_if_modified(|previous| {
            let Some(response) = *previous else { return false };
            if response.camera_override.is_some() {
                return false;
            }
            *previous = None;
            true
        });
    }

    fn complete_sit(&mut self, simulator: SimulatorId, outcome: Result<SitOutcome, AgentControlError>) -> bool {
        if !self.pending_sit.as_ref().is_some_and(|pending| pending.simulator == simulator) {
            return false;
        }
        if let Some(pending) = self.pending_sit.take() {
            tracing::debug!(simulator = pending.simulator.get(), object = %pending.object_id, ?outcome, "sit outcome completed");
            if pending.reply.send(outcome).is_err() {
                tracing::trace!("sit caller stopped before receiving outcome");
            }
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

fn same_sit_linkset(world: &World, requested_object_id: Uuid, response_object_id: Uuid) -> bool {
    requested_object_id == response_object_id
        || linkset_root(world, requested_object_id)
            .is_some_and(|requested_root| linkset_root(world, response_object_id) == Some(requested_root))
}

fn linkset_root(world: &World, object_id: Uuid) -> Option<Uuid> {
    let mut entity = world.entity(object_id)?;
    for _ in 0..=world.object_count() {
        let object = world.object(entity)?;
        match object.parent() {
            Parent::Root => return Some(object.id()),
            Parent::Resolved(parent) => entity = parent,
            Parent::Unknown | Parent::Pending { .. } => return None,
        }
    }
    None
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
                Ok(()) => {
                    tracing::debug!(simulator = simulator.get(), target = %object_id, offset = ?offset, "sent AgentRequestSit");
                    agent_control.begin_sit(object_id, simulator, reply);
                }
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
            if !camera.request_action(MovementAction::SitOnGround) {
                if reply.send(Err(AgentControlError::QueueFull)).is_err() {
                    tracing::trace!("ground-sit caller stopped before receiving the queue outcome");
                }
                return;
            }
            camera.clear_flying();
            agent_control.cancel_sit(SitOutcome::Cancelled);
            agent_control.clear_sit_response_without_camera();
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
            if !camera.request_action(MovementAction::StandUp) {
                if reply.send(Err(AgentControlError::QueueFull)).is_err() {
                    tracing::trace!("stand-up caller stopped before receiving the queue outcome");
                }
                return;
            }
            agent_control.cancel_sit(SitOutcome::Cancelled);
            agent_control.clear_sit_response_without_camera();
            if reply.send(Ok(())).is_err() {
                tracing::trace!("stand-up caller stopped before receiving the queued outcome");
            }
        }
    }
}

pub(super) fn process_sit_response<T: SitTransport>(
    transport: &T,
    agent_control: &mut AgentControlState,
    camera: &mut crate::session::CameraUplink,
    world: &World,
    simulator: SimulatorId,
    active_agent: Option<(Uuid, Uuid)>,
    message: &Message,
) {
    let Some(response) = decode_sit_response(message) else {
        if message.name() == "AvatarSitResponse" {
            tracing::debug!(simulator = simulator.get(), matched = false, "received invalid AvatarSitResponse");
        }
        return;
    };
    let matched = agent_control.expects_response(world, simulator, response.object_id);
    tracing::debug!(
        simulator = simulator.get(),
        object = %response.object_id,
        auto_pilot = response.auto_pilot,
        matched,
        "received AvatarSitResponse"
    );
    if !matched {
        return;
    }
    if response.target.is_none() {
        tracing::debug!(simulator = simulator.get(), object = %response.object_id, "sit response has no valid target transform");
        agent_control.fail_sit(AgentControlError::InvalidSitResponse);
        return;
    }
    if world.entity(response.object_id).and_then(|entity| world.object(entity)).is_none() {
        tracing::debug!(simulator = simulator.get(), object = %response.object_id, "sit response references an unknown object");
        agent_control.fail_sit(AgentControlError::InvalidSitResponse);
        return;
    }
    let Some((agent_id, session_id)) = active_agent else {
        agent_control.fail_sit(AgentControlError::SimulatorUnavailable);
        return;
    };
    if !response.auto_pilot {
        match transport.send_sit(AgentSit { agent_id, session_id }, Delivery::Reliable) {
            Ok(()) => tracing::debug!(simulator = simulator.get(), object = %response.object_id, "sent AgentSit"),
            Err(error) => {
                tracing::debug!(simulator = simulator.get(), %error, "sit acceptance could not be queued");
                agent_control.fail_sit(AgentControlError::SendFailed);
                return;
            }
        }
    }
    if agent_control.accept_sit(simulator, response.object_id, response.auto_pilot, response.force_mouselook, response.camera_override) {
        camera.clear_flying();
        tracing::debug!(
            simulator = simulator.get(),
            object = %response.object_id,
            auto_pilot = response.auto_pilot,
            "sit outcome accepted"
        );
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

    use super::{
        AgentControlError, AgentControlEvent, AgentControlState, COMMAND_CAPACITY, Command, SIT_RESPONSE_TIMEOUT, SitCameraOverride,
        SitOutcome, SitResponseState,
    };

    #[tokio::test(start_paused = true)]
    async fn sit_request_times_out() {
        let (handle, mut state, _) = AgentControlState::new();
        let object_id = Uuid::from_u128(3);
        let simulator = simulator_id();
        let call = tokio::spawn({
            let handle = handle.clone();
            async move { handle.sit_on_object(object_id, Vec3::ZERO).await }
        });
        let AgentControlEvent::Command(Some(Command::SitOnObject { reply, .. })) = state.next_event().await else {
            panic!("object-sit command")
        };
        state.begin_sit(object_id, simulator, reply);
        advance(SIT_RESPONSE_TIMEOUT).await;
        std::assert_matches!(state.next_event().await, AgentControlEvent::SitTimedOut);
        state.timeout_sit();
        assert_eq!(call.await.expect("sit task"), Ok(SitOutcome::TimedOut));
    }

    #[tokio::test(start_paused = true)]
    async fn expired_deadline_precedes_queued_commands() {
        let (handle, mut state, _) = AgentControlState::new();
        let object_id = Uuid::from_u128(4);
        let simulator = simulator_id();
        let mut call = Box::pin(handle.sit_on_object(object_id, Vec3::ZERO));
        assert!(futures::poll!(&mut call).is_pending());
        let AgentControlEvent::Command(Some(Command::SitOnObject { reply, .. })) = state.next_event().await else {
            panic!("object-sit command")
        };
        state.begin_sit(object_id, simulator, reply);

        let mut queued_calls = Vec::new();
        for _ in 0..COMMAND_CAPACITY {
            let mut queued = Box::pin(handle.sit_on_object(Uuid::from_u128(10), Vec3::ZERO));
            assert!(futures::poll!(&mut queued).is_pending());
            queued_calls.push(queued);
        }
        let mut overflow = Box::pin(handle.sit_on_object(Uuid::from_u128(11), Vec3::ZERO));
        std::assert_matches!(futures::poll!(&mut overflow), std::task::Poll::Ready(Err(AgentControlError::QueueFull)));
        advance(SIT_RESPONSE_TIMEOUT).await;

        std::assert_matches!(state.next_event().await, AgentControlEvent::SitTimedOut);
        state.timeout_sit();
        assert_eq!(call.await, Ok(SitOutcome::TimedOut));
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

        assert_eq!(
            *response.borrow(),
            Some(SitResponseState {
                generation: 0,
                object_id,
                requested_object_id: object_id,
                camera_override: Some(camera_override),
                force_mouselook: false,
            })
        );
        state.clear_sit_response_without_camera();
        assert_eq!(
            *response.borrow(),
            Some(SitResponseState {
                generation: 0,
                object_id,
                requested_object_id: object_id,
                camera_override: Some(camera_override),
                force_mouselook: false,
            })
        );
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
