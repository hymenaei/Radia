use std::mem;

use tokio::{sync::oneshot, time::Instant};

use crate::error::Error;

use super::{ConnectionError, LOGOUT_TIMEOUT};

pub(super) enum Phase {
    Connecting { region_handshake_seen: bool },
    Established { region_handshake_seen: bool },
    Closing { stage: ConnectionStage, logout: LogoutState, deadline: Instant, reason: Option<&'static str> },
    Draining { stage: ConnectionStage, logout: LogoutState, deadline: Instant, reason: Option<&'static str> },
}

#[derive(Clone, Copy)]
pub(super) enum ConnectionStage {
    Connecting { region_handshake_seen: bool },
    Established { region_handshake_seen: bool },
}

pub(super) enum LogoutState {
    NotRequested,
    AwaitingLocalSend(Vec<oneshot::Sender<Result<(), Error>>>),
    Complete,
}

pub(super) enum LogoutAction {
    Queue(oneshot::Sender<Result<(), Error>>),
    Complete(oneshot::Sender<Result<(), Error>>),
    Waiting,
}

pub(super) enum RunControl {
    Continue,
    Finish(Option<&'static str>),
}

impl Phase {
    pub(super) fn new() -> Self {
        Self::Connecting { region_handshake_seen: false }
    }

    pub(super) fn is_established(&self) -> bool {
        matches!(self.stage(), ConnectionStage::Established { .. })
    }

    pub(super) fn is_closing(&self) -> bool {
        matches!(self, Self::Closing { .. } | Self::Draining { .. })
    }

    pub(super) fn is_draining(&self) -> bool {
        matches!(self, Self::Draining { .. })
    }

    pub(super) fn has_region_handshake(&self) -> bool {
        match self.stage() {
            ConnectionStage::Connecting { region_handshake_seen } | ConnectionStage::Established { region_handshake_seen } => {
                region_handshake_seen
            }
        }
    }

    pub(super) fn mark_region_handshake(&mut self) {
        match self {
            Self::Connecting { region_handshake_seen }
            | Self::Established { region_handshake_seen }
            | Self::Closing { stage: ConnectionStage::Connecting { region_handshake_seen }, .. }
            | Self::Closing { stage: ConnectionStage::Established { region_handshake_seen }, .. }
            | Self::Draining { stage: ConnectionStage::Connecting { region_handshake_seen }, .. }
            | Self::Draining { stage: ConnectionStage::Established { region_handshake_seen }, .. } => {
                *region_handshake_seen = true;
            }
        }
    }

    pub(super) fn mark_established(&mut self) -> bool {
        match self {
            Self::Connecting { region_handshake_seen } => {
                *self = Self::Established { region_handshake_seen: *region_handshake_seen };
                true
            }
            Self::Established { .. } => false,
            Self::Closing { stage, .. } | Self::Draining { stage, .. } => match stage {
                ConnectionStage::Connecting { region_handshake_seen } => {
                    *stage = ConnectionStage::Established { region_handshake_seen: *region_handshake_seen };
                    true
                }
                ConnectionStage::Established { .. } => false,
            },
        }
    }

    pub(super) fn close(&mut self, reason: Option<&'static str>, reset_deadline: bool) {
        let deadline = Instant::now() + LOGOUT_TIMEOUT;
        match self {
            Self::Connecting { region_handshake_seen } => {
                *self = Self::Closing {
                    stage: ConnectionStage::Connecting { region_handshake_seen: *region_handshake_seen },
                    logout: LogoutState::NotRequested,
                    deadline,
                    reason,
                };
            }
            Self::Established { region_handshake_seen } => {
                *self = Self::Closing {
                    stage: ConnectionStage::Established { region_handshake_seen: *region_handshake_seen },
                    logout: LogoutState::NotRequested,
                    deadline,
                    reason,
                };
            }
            Self::Closing { deadline: current_deadline, reason: current_reason, .. }
            | Self::Draining { deadline: current_deadline, reason: current_reason, .. } => {
                if reset_deadline {
                    *current_deadline = deadline;
                }
                if let Some(reason) = reason {
                    *current_reason = Some(reason);
                }
            }
        }
    }

    pub(super) fn begin_draining(&mut self) {
        let transition = match self {
            Self::Closing { stage, logout, deadline, reason } => {
                Some((*stage, mem::replace(logout, LogoutState::NotRequested), *deadline, *reason))
            }
            Self::Connecting { .. } | Self::Established { .. } | Self::Draining { .. } => None,
        };
        if let Some((stage, logout, deadline, reason)) = transition {
            *self = Self::Draining { stage, logout, deadline, reason };
        }
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        match self {
            Self::Closing { deadline, .. } | Self::Draining { deadline, .. } => Some(*deadline),
            Self::Connecting { .. } | Self::Established { .. } => None,
        }
    }

    pub(super) fn close_reason(&self) -> Option<&'static str> {
        match self {
            Self::Closing { reason, .. } | Self::Draining { reason, .. } => *reason,
            Self::Connecting { .. } | Self::Established { .. } => None,
        }
    }

    pub(super) fn logout_requested(&self) -> bool {
        matches!(self.logout_state(), Some(LogoutState::AwaitingLocalSend(_) | LogoutState::Complete))
    }

    pub(super) fn request_logout(&mut self, completed: oneshot::Sender<Result<(), Error>>) -> LogoutAction {
        if !self.is_closing() {
            self.close(None, false);
        }
        match self {
            Self::Closing { logout, .. } | Self::Draining { logout, .. } => match logout {
                LogoutState::NotRequested => {
                    *logout = LogoutState::AwaitingLocalSend(Vec::new());
                    LogoutAction::Queue(completed)
                }
                LogoutState::AwaitingLocalSend(waiters) => {
                    waiters.push(completed);
                    LogoutAction::Waiting
                }
                LogoutState::Complete => LogoutAction::Complete(completed),
            },
            Self::Connecting { .. } | Self::Established { .. } => unreachable!("logout closes the connection"),
        }
    }

    pub(super) fn complete_logout(&mut self, error: Option<ConnectionError>) {
        let Some(logout) = self.logout_state_mut() else { return };
        let LogoutState::AwaitingLocalSend(waiters) = mem::replace(logout, LogoutState::Complete) else { return };
        for waiter in waiters {
            let result = error.as_ref().map_or(Ok(()), |error| Err(error.to_network_error()));
            if let Err(error) = waiter.send(result) {
                drop(error);
            }
        }
    }

    fn stage(&self) -> ConnectionStage {
        match self {
            Self::Connecting { region_handshake_seen } => ConnectionStage::Connecting { region_handshake_seen: *region_handshake_seen },
            Self::Established { region_handshake_seen } => ConnectionStage::Established { region_handshake_seen: *region_handshake_seen },
            Self::Closing { stage, .. } | Self::Draining { stage, .. } => *stage,
        }
    }

    fn logout_state(&self) -> Option<&LogoutState> {
        match self {
            Self::Closing { logout, .. } | Self::Draining { logout, .. } => Some(logout),
            Self::Connecting { .. } | Self::Established { .. } => None,
        }
    }

    fn logout_state_mut(&mut self) -> Option<&mut LogoutState> {
        match self {
            Self::Closing { logout, .. } | Self::Draining { logout, .. } => Some(logout),
            Self::Connecting { .. } | Self::Established { .. } => None,
        }
    }
}
