use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex, mpsc::SyncSender},
    time::{Duration, Instant},
};

use super::{
    WorkerState, clone_error,
    support::{is_simulated_crash, lock, reply_result},
};
use crate::cache::{
    Key, MAX_QUEUED_WRITE_BYTES, MAX_QUEUED_WRITES, StorageError, UnavailableReason, writer::QUEUE_SWEEP_CHUNK,
    writer::eviction::low_watermark,
};

const SWEEP_INTERVAL: Duration = Duration::from_millis(5);
const MAINTENANCE_RETRY_INITIAL: Duration = Duration::from_millis(100);
const MAINTENANCE_RETRY_MAX: Duration = Duration::from_secs(1);

pub(in crate::cache) enum Command {
    Put { key: Key, bytes: Vec<u8>, reply: SyncSender<Result<(), StorageError>> },
    Remove { key: Key, reply: SyncSender<Result<bool, StorageError>> },
    Clear { reply: SyncSender<Result<(), StorageError>> },
    SetLimit { limit: u64, reply: SyncSender<Result<(), StorageError>> },
    Flush { reply: SyncSender<Result<(), StorageError>> },
    Shutdown { reply: SyncSender<Result<(), StorageError>> },
}

struct QueuedCommand {
    command: Command,
    bytes: usize,
}

#[derive(Default)]
struct QueueState {
    commands: VecDeque<QueuedCommand>,
    pending_bytes: usize,
    pending_commands: usize,
    closed: bool,
}

#[derive(Default)]
pub(in crate::cache) struct WriteQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
    space: Condvar,
}

struct QueueBatch {
    commands: Vec<QueuedCommand>,
    closed: bool,
}

impl WriteQueue {
    pub fn try_send(&self, command: Command, bytes: usize) -> Result<(), StorageError> {
        let mut state = lock(&self.state);
        if state.closed {
            return Err(StorageError::Unavailable(UnavailableReason::WriterStopped));
        }
        let Some(next_bytes) = state.pending_bytes.checked_add(bytes) else {
            return Err(StorageError::Unavailable(UnavailableReason::WriteQueueFull));
        };
        if next_bytes > MAX_QUEUED_WRITE_BYTES || state.pending_commands >= MAX_QUEUED_WRITES {
            return Err(StorageError::Unavailable(UnavailableReason::WriteQueueFull));
        }
        state.pending_bytes = next_bytes;
        state.pending_commands += 1;
        state.commands.push_back(QueuedCommand { command, bytes });
        self.ready.notify_one();
        Ok(())
    }

    pub fn send_terminal(&self, command: Command) -> Result<(), StorageError> {
        let mut state = lock(&self.state);
        while !state.closed && state.pending_commands >= MAX_QUEUED_WRITES {
            state = match self.space.wait(state) {
                Ok(state) => state,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
        if state.closed {
            return Err(StorageError::Unavailable(UnavailableReason::WriterStopped));
        }
        state.pending_commands += 1;
        state.commands.push_back(QueuedCommand { command, bytes: 0 });
        state.closed = true;
        self.ready.notify_one();
        Ok(())
    }

    fn receive_batch(&self) -> QueueBatch {
        let mut state = lock(&self.state);
        if state.commands.is_empty() && !state.closed {
            let (next, _) = match self.ready.wait_timeout(state, SWEEP_INTERVAL) {
                Ok(result) => result,
                Err(poisoned) => poisoned.into_inner(),
            };
            state = next;
        }
        let Some(first) = state.commands.pop_front() else {
            return QueueBatch { commands: Vec::new(), closed: state.closed };
        };
        let group_puts = matches!(&first.command, Command::Put { .. });
        let group_limits = matches!(&first.command, Command::SetLimit { .. });
        let mut commands = vec![first];
        loop {
            let same_group = state.commands.front().is_some_and(|queued| {
                (group_puts && matches!(&queued.command, Command::Put { .. }))
                    || (group_limits && matches!(&queued.command, Command::SetLimit { .. }))
            });
            if !same_group {
                break;
            }
            if let Some(next) = state.commands.pop_front() {
                commands.push(next);
            }
        }
        QueueBatch { commands, closed: state.closed && state.commands.is_empty() }
    }

    fn complete(&self, bytes: usize) {
        let mut state = lock(&self.state);
        state.pending_bytes = state.pending_bytes.saturating_sub(bytes);
        state.pending_commands = state.pending_commands.saturating_sub(1);
        self.space.notify_all();
    }

    pub fn close(&self) {
        let mut state = lock(&self.state);
        state.closed = true;
        self.ready.notify_all();
        self.space.notify_all();
    }
}

pub(in crate::cache) fn writer_loop(mut state: WorkerState, queue: Arc<WriteQueue>) {
    #[cfg(test)]
    if state.hooks.check(super::WritePoint::WriterReady).is_err() {
        return;
    }
    let mut maintenance_retry_at = None;
    let mut maintenance_retry_delay = MAINTENANCE_RETRY_INITIAL;
    let mut maintenance_warning_open = false;
    loop {
        let batch = queue.receive_batch();
        let idle = batch.commands.is_empty();
        let contains_put = batch.commands.iter().any(|queued| matches!(&queued.command, Command::Put { .. }));
        if !idle {
            maintenance_retry_at = None;
            maintenance_retry_delay = MAINTENANCE_RETRY_INITIAL;
        }
        state.evictions_this_batch = 0;
        let mut commands = batch.commands.into_iter().peekable();
        let mut limit_enforcement_attempted = false;
        while let Some(queued) = commands.next() {
            match queued.command {
                Command::Put { key, bytes, reply } => {
                    let mut puts = vec![QueuedCommand { command: Command::Put { key, bytes, reply }, bytes: queued.bytes }];
                    while commands.peek().is_some_and(|queued| matches!(&queued.command, Command::Put { .. })) {
                        let Some(next) = commands.next() else {
                            break;
                        };
                        puts.push(next);
                    }
                    let results = state.put_batch(
                        puts.len(),
                        puts.iter().map(|queued| match &queued.command {
                            Command::Put { key, bytes, .. } => (*key, bytes.as_slice()),
                            _ => unreachable!(),
                        }),
                    );
                    let simulated_crash = results.iter().any(|result| result.as_ref().is_err_and(is_simulated_crash));
                    for (queued, result) in puts.into_iter().zip(results) {
                        if let Command::Put { reply, .. } = queued.command {
                            reply_result(reply, result);
                        }
                        queue.complete(queued.bytes);
                    }
                    if simulated_crash {
                        return;
                    }
                }
                Command::Remove { key, reply } => {
                    state.drain_invalid();
                    reply_result(reply, state.remove(key));
                    queue.complete(queued.bytes);
                }
                Command::Clear { reply } => {
                    state.drain_invalid();
                    reply_result(reply, state.clear());
                    queue.complete(queued.bytes);
                }
                Command::SetLimit { limit: first_limit, reply: first_reply } => {
                    let mut limits =
                        vec![QueuedCommand { command: Command::SetLimit { limit: first_limit, reply: first_reply }, bytes: queued.bytes }];
                    while commands.peek().is_some_and(|queued| matches!(&queued.command, Command::SetLimit { .. })) {
                        let Some(next) = commands.next() else {
                            break;
                        };
                        limits.push(next);
                    }
                    let minimum_limit = state.read_state.minimum_size_limit;
                    let final_valid_limit = limits
                        .iter()
                        .filter_map(|queued| match &queued.command {
                            Command::SetLimit { limit, .. } if *limit >= minimum_limit => Some(*limit),
                            _ => None,
                        })
                        .next_back();
                    state.drain_invalid();
                    let result = final_valid_limit.map(|limit| state.set_limit(limit));
                    limit_enforcement_attempted = result.is_some();
                    for queued in limits {
                        if let Command::SetLimit { limit, reply } = queued.command {
                            let caller_result = if limit < minimum_limit {
                                Err(StorageError::LimitTooSmall)
                            } else {
                                match result.as_ref().expect("valid limit group has an enforcement result") {
                                    Ok(()) => Ok(()),
                                    Err(error) => Err(clone_error(error)),
                                }
                            };
                            reply_result(reply, caller_result);
                        }
                        queue.complete(queued.bytes);
                    }
                    #[cfg(test)]
                    if state.hooks.check(super::WritePoint::SetLimitReplied).is_err() {
                        return;
                    }
                }
                Command::Flush { reply } => {
                    state.drain_invalid();
                    state.sweep_all();
                    let result = state.flush();
                    let simulated_crash = result.as_ref().is_err_and(is_simulated_crash);
                    reply_result(reply, result);
                    queue.complete(queued.bytes);
                    if simulated_crash {
                        return;
                    }
                }
                Command::Shutdown { reply } => {
                    reply_result(reply, state.shutdown());
                    queue.complete(queued.bytes);
                    return;
                }
            }
        }
        state.drain_touches_best_effort();
        state.drain_invalid();
        let watermark = low_watermark(state.size_limit, state.empty_cache_size());
        let maintenance_eligible = !batch.closed && !limit_enforcement_attempted && (idle || !contains_put);
        if maintenance_eligible && state.accounted_size() <= watermark {
            maintenance_retry_at = None;
            maintenance_retry_delay = MAINTENANCE_RETRY_INITIAL;
            maintenance_warning_open = false;
        } else if maintenance_eligible && maintenance_retry_at.is_none_or(|deadline| Instant::now() >= deadline) {
            #[cfg(test)]
            if state.hooks.check(super::WritePoint::MaintenanceStarted).is_err() {
                return;
            }
            match state.enforce_limit(watermark) {
                Ok(()) => {
                    maintenance_retry_at = None;
                    maintenance_retry_delay = MAINTENANCE_RETRY_INITIAL;
                    maintenance_warning_open = false;
                }
                Err(error) => {
                    if !maintenance_warning_open {
                        tracing::warn!(error = %error, "cache limit enforcement failed");
                        maintenance_warning_open = true;
                    }
                    maintenance_retry_at = Some(Instant::now() + maintenance_retry_delay);
                    maintenance_retry_delay = (maintenance_retry_delay * 2).min(MAINTENANCE_RETRY_MAX);
                }
            }
            #[cfg(test)]
            if state.hooks.check(super::WritePoint::MaintenanceFinished).is_err() {
                return;
            }
        }
        #[cfg(test)]
        if state.hooks.check(super::WritePoint::MaintenanceCycleComplete).is_err() {
            return;
        }
        state.sweep_step(QUEUE_SWEEP_CHUNK);
        if batch.closed {
            state.drain_invalid();
            state.sweep_all();
            if let Err(error) = state.flush() {
                tracing::warn!(error = %error, "cache shutdown flush failed");
            }
            return;
        }
    }
}
