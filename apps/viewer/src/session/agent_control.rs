use std::{error::Error, fmt, future::pending, time::Duration};

use math::Vec3;
use network::SimulatorId;
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{Instant, sleep_until},
};
use uuid::Uuid;

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
    pub(crate) object_id: Uuid,
    pub(crate) eye_offset: Vec3,
    pub(crate) at_offset: Vec3,
}

pub(crate) enum AgentControlEvent {
    Command(Option<Command>),
    SitTimedOut,
}

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
    camera_override: watch::Sender<Option<SitCameraOverride>>,
}

impl AgentControlState {
    pub(crate) fn new() -> (AgentControlHandle, Self, watch::Receiver<Option<SitCameraOverride>>) {
        let (sender, commands) = mpsc::channel(COMMAND_CAPACITY);
        let (camera_override, camera_updates) = watch::channel(None);
        (AgentControlHandle { commands: sender }, Self { commands, pending_sit: None, camera_override }, camera_updates)
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
        if let Some(pending) = self.pending_sit.take() {
            let _ = pending.reply.send(Ok(SitOutcome::Accepted { auto_pilot, force_mouselook }));
        }
        self.set_camera_override(camera_override);
        true
    }

    pub(crate) fn refuse_sit(&mut self, simulator: SimulatorId) -> bool {
        self.complete_sit(simulator, Ok(SitOutcome::Refused))
    }

    pub(crate) fn cancel_sit(&mut self, outcome: SitOutcome) {
        if let Some(pending) = self.pending_sit.take() {
            let _ = pending.reply.send(Ok(outcome));
        }
    }

    pub(crate) fn timeout_sit(&mut self) {
        if self.pending_sit.as_ref().is_some_and(|pending| Instant::now() >= pending.deadline) {
            self.cancel_sit(SitOutcome::TimedOut);
        }
    }

    pub(crate) fn fail_sit(&mut self, error: AgentControlError) {
        if let Some(pending) = self.pending_sit.take() {
            let _ = pending.reply.send(Err(error));
        }
    }

    pub(crate) fn cancel_if_simulator_changed(&mut self, simulator: SimulatorId) {
        if self.pending_sit.as_ref().is_some_and(|pending| pending.simulator != simulator) {
            self.cancel_sit(SitOutcome::SimulatorChanged);
        }
    }

    pub(crate) fn clear_camera_override(&self) {
        self.set_camera_override(None);
    }

    fn complete_sit(&mut self, simulator: SimulatorId, outcome: Result<SitOutcome, AgentControlError>) -> bool {
        if !self.pending_sit.as_ref().is_some_and(|pending| pending.simulator == simulator) {
            return false;
        }
        if let Some(pending) = self.pending_sit.take() {
            let _ = pending.reply.send(outcome);
        }
        true
    }

    fn set_camera_override(&self, camera_override: Option<SitCameraOverride>) {
        self.camera_override.send_if_modified(|previous| {
            if *previous == camera_override {
                false
            } else {
                *previous = camera_override;
                true
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use math::{Quat, Vec3};
    use protocol::{
        lludp::{BlockValues, FieldValue, Message, Packet, SchemaMessage},
        messages::{
            AgentRequestSit, AgentSit, AvatarSitResponse,
            agent::agent_request_sit::{AgentData as SitAgentData, TargetObject},
            agent::avatar_sit_response::{SitObject, SitTransform},
        },
    };
    use tokio::{net::UdpSocket, time::advance};
    use uuid::Uuid;

    use crate::{session::inbound::decode_sit_response, test_helpers::simulator_id};

    use super::{AgentControlEvent, AgentControlState, Command, SIT_RESPONSE_TIMEOUT, SitCameraOverride, SitOutcome};

    async fn receive_message(socket: &UdpSocket) -> Message {
        let mut bytes = [0; 2048];
        let (length, _) = socket.recv_from(&mut bytes).await.expect("loopback packet");
        Packet::decode(&bytes[..length]).expect("valid packet").message().clone()
    }

    async fn send_message(socket: &UdpSocket, destination: std::net::SocketAddr, sequence: u32, message: Message) {
        let bytes = Packet::new(sequence, message).encode().expect("encode packet");
        socket.send_to(&bytes, destination).await.expect("send loopback packet");
    }

    fn alert_message() -> Message {
        let schema = SchemaMessage::schema_by_name("AlertMessage").expect("AlertMessage schema");
        Message::Schema(
            SchemaMessage::new(
                schema,
                vec![
                    BlockValues::single(vec![FieldValue::Bytes(b"SitFailed".to_vec())]),
                    BlockValues::new(Vec::new()),
                    BlockValues::new(Vec::new()),
                ],
            )
            .expect("valid AlertMessage"),
        )
    }

    #[tokio::test]
    async fn loopback_simulator_accepts_object_sit_and_receives_agent_sit() {
        let simulator_socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind loopback simulator");
        let viewer_socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind loopback viewer");
        let simulator_address = simulator_socket.local_addr().expect("simulator address");
        let viewer_address = viewer_socket.local_addr().expect("viewer address");
        let simulator = simulator_id();
        let agent_id = Uuid::from_u128(1);
        let session_id = Uuid::from_u128(2);
        let object_id = Uuid::from_u128(3);
        let (handle, mut state, camera_updates) = AgentControlState::new();
        let call = tokio::spawn(async move { handle.sit_on_object(object_id, Vec3::new(1.0, 2.0, 3.0)).await });

        let AgentControlEvent::Command(Some(Command::SitOnObject { object_id: requested, offset, reply })) = state.next_event().await
        else {
            panic!("object-sit command")
        };
        assert_eq!(requested, object_id);
        assert_eq!(offset, Vec3::new(1.0, 2.0, 3.0));
        let request = AgentRequestSit {
            agent_data: SitAgentData { agent_id, session_id },
            target_object: TargetObject { target_id: object_id, offset },
        };
        send_message(&viewer_socket, simulator_address, 1, Message::AgentRequestSit(request.clone())).await;
        assert_eq!(receive_message(&simulator_socket).await, Message::AgentRequestSit(request));
        state.begin_sit(object_id, simulator, reply);

        let response = AvatarSitResponse {
            sit_object: SitObject { id: Some(object_id) },
            sit_transform: SitTransform {
                auto_pilot: Some(true),
                sit_position: Some(Vec3::new(4.0, 5.0, 6.0)),
                sit_rotation: Some(Quat::IDENTITY),
                camera_eye_offset: Some(Vec3::new(2.0, 0.0, 1.0)),
                camera_at_offset: Some(Vec3::new(0.0, 0.0, 1.0)),
                force_mouselook: Some(false),
            },
        };
        send_message(&simulator_socket, viewer_address, 2, Message::AvatarSitResponse(response)).await;
        let Message::AvatarSitResponse(response) = receive_message(&viewer_socket).await else { panic!("AvatarSitResponse") };
        let response = decode_sit_response(&Message::AvatarSitResponse(response)).expect("valid sit response");
        let camera_override = response.camera_override;
        assert!(state.accept_sit(simulator, response.object_id, response.auto_pilot, response.force_mouselook, camera_override,));

        let sit = AgentSit { agent_id, session_id };
        send_message(&viewer_socket, simulator_address, 3, Message::AgentSit(sit.clone())).await;
        assert_eq!(receive_message(&simulator_socket).await, Message::AgentSit(sit));
        assert_eq!(
            *camera_updates.borrow(),
            Some(SitCameraOverride { object_id, eye_offset: Vec3::new(2.0, 0.0, 1.0), at_offset: Vec3::new(0.0, 0.0, 1.0) })
        );
        assert_eq!(call.await.expect("sit task"), Ok(SitOutcome::Accepted { auto_pilot: true, force_mouselook: false }));
    }

    #[tokio::test]
    async fn loopback_alert_refuses_object_sit() {
        let simulator_socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind loopback simulator");
        let viewer_socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind loopback viewer");
        let viewer_address = viewer_socket.local_addr().expect("viewer address");
        let simulator = simulator_id();
        let object_id = Uuid::from_u128(3);
        let (handle, mut state, _) = AgentControlState::new();
        let call = tokio::spawn(async move { handle.sit_on_object(object_id, Vec3::ZERO).await });
        let AgentControlEvent::Command(Some(Command::SitOnObject { reply, .. })) = state.next_event().await else {
            panic!("object-sit command")
        };
        state.begin_sit(object_id, simulator, reply);

        send_message(&simulator_socket, viewer_address, 1, alert_message()).await;
        let message = receive_message(&viewer_socket).await;
        assert_eq!(message.name(), "AlertMessage");
        assert!(state.refuse_sit(simulator));
        assert_eq!(call.await.expect("sit task"), Ok(SitOutcome::Refused));
    }

    #[tokio::test(start_paused = true)]
    async fn sit_request_times_out_after_the_bounded_response_window() {
        let (handle, mut state, _) = AgentControlState::new();
        let object_id = Uuid::from_u128(3);
        let simulator = simulator_id();
        let call = tokio::spawn(async move { handle.sit_on_object(object_id, Vec3::ZERO).await });
        let AgentControlEvent::Command(Some(Command::SitOnObject { reply, .. })) = state.next_event().await else {
            panic!("object-sit command")
        };
        state.begin_sit(object_id, simulator, reply);
        advance(SIT_RESPONSE_TIMEOUT).await;
        assert!(matches!(state.next_event().await, AgentControlEvent::SitTimedOut));
        state.timeout_sit();
        assert_eq!(call.await.expect("sit task"), Ok(SitOutcome::TimedOut));
    }

    #[tokio::test]
    async fn standing_cancels_a_pending_object_sit() {
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
    fn object_sit_rejects_nil_ids_and_non_finite_offsets() {
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
