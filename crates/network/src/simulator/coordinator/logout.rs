use std::future::Future;

use tokio::{
    sync::oneshot,
    time::{self, Instant},
};

use super::{LOGOUT_TIMEOUT, SimulatorCoordinator};
use crate::{error::Error, status::NetworkLifecycle};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LogoutOutcome {
    Sent,
    AlreadySent,
    Unavailable(LogoutUnavailable),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LogoutUnavailable {
    ConnectionClosed,
    ConnectionEnded,
    Deadline,
    QueueFull,
}

impl LogoutUnavailable {
    fn description(self) -> &'static str {
        match self {
            Self::ConnectionClosed => "the active simulator connection is closed",
            Self::ConnectionEnded => "the active simulator connection ended before sending logout",
            Self::Deadline => "the bounded logout send deadline expired",
            Self::QueueFull => "the active simulator logout queue was full",
        }
    }
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

#[derive(Debug, Eq, PartialEq)]
pub(super) enum LogoutAttemptError {
    Unavailable(LogoutUnavailable),
    Unexpected(String),
}

fn logout_completion_error(error: Error) -> LogoutAttemptError {
    match error {
        Error::SimulatorLogoutSendAgeExpired => LogoutAttemptError::Unavailable(LogoutUnavailable::Deadline),
        error => LogoutAttemptError::Unexpected(error.to_string()),
    }
}

impl SimulatorCoordinator {
    pub(super) async fn disconnect_once(&mut self, logout_already_sent: bool) -> Result<LogoutOutcome, String> {
        tracing::info!(logout_already_sent, "simulator disconnect started");
        self.status.set_lifecycle(NetworkLifecycle::Disconnecting);
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

    pub(super) fn apply_logout_attempt_result(&mut self, result: Result<(), LogoutAttemptError>) -> Result<(), String> {
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
            Err(LogoutAttemptError::Unexpected(error)) => Err(error),
        }
    }

    pub(super) async fn finish_pending_logout(&mut self) -> Result<(), String> {
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
        let message = format!("Simulator logout request was not sent before disconnect completed: {}", reason.description());
        tracing::warn!(?reason, "simulator disconnect completed without sending logout");
        self.status.record_error(message);
    }

    pub(super) async fn graceful_disconnect(&mut self, logout_already_sent: bool) -> Result<(), String> {
        let result = self.disconnect_once(logout_already_sent).await;
        self.neighbor_cancellation.send_replace(true);
        result.map(|_| ())
    }
}

pub(super) fn finish_logout(reason: Option<&'static str>) -> Result<(), String> {
    reason.map_or(Ok(()), |reason| Err(reason.to_owned()))
}

pub(super) async fn send_logout_with_deadline(
    send: impl Future<Output = Result<(), LogoutAttemptError>>,
    deadline_reason: LogoutUnavailable,
) -> Result<(), LogoutAttemptError> {
    time::timeout(LOGOUT_TIMEOUT, send).await.map_err(|_| LogoutAttemptError::Unavailable(deadline_reason))?
}
