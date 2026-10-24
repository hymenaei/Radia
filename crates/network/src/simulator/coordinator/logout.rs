use std::{error::Error as StdError, fmt, future::Future};

use tokio::{
    sync::oneshot,
    time::{self, Instant},
};

use super::{CoordinatorError, CoordinatorRuntime, CoordinatorState, LOGOUT_TIMEOUT, RuntimeAction};
use crate::error::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LogoutOutcome {
    Sent,
    AlreadySent,
    Unavailable(LogoutUnavailable),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LogoutUnavailable {
    ConnectionClosed,
    ConnectionEnded,
    Deadline,
    QueueFull,
}

pub(super) struct PendingLogout {
    pub(super) completion: oneshot::Receiver<Result<(), Error>>,
    pub(super) deadline: Instant,
}

impl PendingLogout {
    pub(super) async fn wait(&mut self) -> Result<(), LogoutAttemptError> {
        match time::timeout_at(self.deadline, &mut self.completion).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(error))) => Err(logout_completion_error(error)),
            Ok(Err(_)) => Err(LogoutAttemptError::Unavailable(LogoutUnavailable::ConnectionEnded)),
            Err(_) => Err(LogoutAttemptError::Unavailable(LogoutUnavailable::Deadline)),
        }
    }
}

#[derive(Debug)]
pub(crate) enum LogoutAttemptError {
    Unavailable(LogoutUnavailable),
    Unexpected(Error),
}

impl fmt::Display for LogoutAttemptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(formatter, "simulator logout was unavailable: {reason:?}"),
            Self::Unexpected(error) => error.fmt(formatter),
        }
    }
}

impl StdError for LogoutAttemptError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Unexpected(error) => Some(error),
            Self::Unavailable(_) => None,
        }
    }
}

fn logout_completion_error(error: Error) -> LogoutAttemptError {
    match error {
        Error::SimulatorLogoutSendAgeExpired => LogoutAttemptError::Unavailable(LogoutUnavailable::Deadline),
        error => LogoutAttemptError::Unexpected(error),
    }
}

impl CoordinatorState {
    pub(super) async fn disconnect_once(&mut self, logout_already_sent: bool) -> Result<LogoutOutcome, CoordinatorError> {
        tracing::info!(logout_already_sent, "simulator disconnect started");
        self.finish_pending_logout().await?;
        let outcome = if logout_already_sent || self.logout_request_sent {
            LogoutOutcome::AlreadySent
        } else if let Some(reason) = self.logout_unavailable {
            LogoutOutcome::Unavailable(reason)
        } else if self.logout_attempted {
            LogoutOutcome::Unavailable(LogoutUnavailable::ConnectionEnded)
        } else if self.root_connection_failed && self.primary.simulator_id == self.simulator_id {
            self.logout_attempted = true;
            self.record_logout_unavailable(LogoutUnavailable::ConnectionClosed);
            LogoutOutcome::Unavailable(LogoutUnavailable::ConnectionClosed)
        } else {
            self.logout_attempted = true;
            let (completed, response) = oneshot::channel();
            let result = send_logout_with_deadline(
                async {
                    self.primary
                        .logout
                        .send(completed)
                        .await
                        .map_err(|_| LogoutAttemptError::Unavailable(LogoutUnavailable::ConnectionClosed))?;
                    response
                        .await
                        .map_err(|_| LogoutAttemptError::Unavailable(LogoutUnavailable::ConnectionEnded))?
                        .map_err(logout_completion_error)
                },
                LogoutUnavailable::Deadline,
            )
            .await;
            self.apply_logout_attempt_result(result)?;
            if self.logout_request_sent {
                LogoutOutcome::Sent
            } else {
                LogoutOutcome::Unavailable(self.logout_unavailable.unwrap_or(LogoutUnavailable::ConnectionEnded))
            }
        };
        match outcome {
            LogoutOutcome::Sent => {
                tracing::debug!(local_send_completed = true, remote_receipt_confirmed = false, "simulator logout request sent");
            }
            LogoutOutcome::AlreadySent => {
                tracing::debug!(local_send_completed = true, remote_receipt_confirmed = false, "simulator logout request was already sent");
            }
            LogoutOutcome::Unavailable(reason) => {
                self.record_logout_unavailable(reason);
            }
        }
        Ok(outcome)
    }

    pub(super) fn apply_logout_attempt_result(&mut self, result: Result<(), LogoutAttemptError>) -> Result<(), CoordinatorError> {
        match result {
            Ok(()) => {
                self.logout_request_sent = true;
                self.logout_unavailable = None;
                Ok(())
            }
            Err(LogoutAttemptError::Unavailable(reason)) => {
                self.record_logout_unavailable(reason);
                Ok(())
            }
            Err(LogoutAttemptError::Unexpected(error)) => Err(error.into()),
        }
    }

    pub(super) async fn finish_pending_logout(&mut self) -> Result<(), CoordinatorError> {
        if let Some(mut pending) = self.pending_primary_logout.take() {
            let result = pending.wait().await;
            self.apply_logout_attempt_result(result)?;
        }
        Ok(())
    }

    pub(super) fn record_logout_unavailable(&mut self, reason: LogoutUnavailable) {
        if self.logout_unavailable == Some(reason) {
            return;
        }
        self.logout_unavailable = Some(reason);
        tracing::warn!(simulator = self.primary.simulator_id.get(), ?reason, "simulator disconnect completed without sending logout");
    }

    pub(super) async fn graceful_disconnect(&mut self, logout_already_sent: bool) -> Result<(), CoordinatorError> {
        let result = self.disconnect_once(logout_already_sent).await;
        self.neighbor_cancellation.send_replace(true);
        result.map(|_| ())
    }
}

impl CoordinatorRuntime<'_> {
    pub(super) async fn handle_disconnect_changed(&mut self) -> Result<RuntimeAction, CoordinatorError> {
        self.coordinator.graceful_disconnect(self.coordinator.logout_request_sent).await?;
        Ok(RuntimeAction::Exit(Ok(())))
    }

    pub(super) fn handle_primary_logout_completed(
        &mut self,
        result: Result<(), LogoutAttemptError>,
    ) -> Result<RuntimeAction, CoordinatorError> {
        self.coordinator.pending_primary_logout = None;
        self.coordinator.apply_logout_attempt_result(result)?;
        Ok(RuntimeAction::Continue)
    }

    pub(super) async fn handle_logout_deadline(&mut self) -> Result<RuntimeAction, CoordinatorError> {
        self.coordinator.finish_pending_logout().await?;
        if *self.disconnect_requested.borrow() {
            self.coordinator.neighbor_cancellation.send_replace(true);
            return Ok(RuntimeAction::Exit(Ok(())));
        }
        Ok(RuntimeAction::Exit(finish_coordinator_logout(self.run_state.disconnect_reason)))
    }

    pub(super) async fn handle_event_receiver_closed(&mut self) -> Result<RuntimeAction, CoordinatorError> {
        self.coordinator.graceful_disconnect(false).await?;
        Ok(RuntimeAction::Exit(Ok(())))
    }
}

pub(super) fn finish_logout(reason: Option<&'static str>) -> Option<&'static str> {
    reason
}

pub(super) fn finish_coordinator_logout(reason: Option<&'static str>) -> Result<(), CoordinatorError> {
    match finish_logout(reason) {
        Some(reason) => Err(CoordinatorError::ShutdownReason(reason)),
        None => Ok(()),
    }
}

pub(super) async fn send_logout_with_deadline(
    send: impl Future<Output = Result<(), LogoutAttemptError>>,
    deadline_reason: LogoutUnavailable,
) -> Result<(), LogoutAttemptError> {
    time::timeout(LOGOUT_TIMEOUT, send).await.map_err(|_| LogoutAttemptError::Unavailable(deadline_reason))?
}
