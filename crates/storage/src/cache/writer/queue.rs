use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex, mpsc::SyncSender},
    time::Duration,
};

use super::{WorkerState, is_simulated_crash, lock, reply_result};
use crate::cache::{Key, MAX_QUEUED_WRITE_BYTES, MAX_QUEUED_WRITES, StorageError, UnavailableReason, writer::QUEUE_SWEEP_CHUNK};

const SWEEP_INTERVAL: Duration = Duration::from_millis(5);

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
        let commands = state.commands.drain(..).collect();
        QueueBatch { commands, closed: state.closed }
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
    loop {
        let batch = queue.receive_batch();
        let mut commands = batch.commands.into_iter().peekable();
        while let Some(queued) = commands.next() {
            match queued.command {
                Command::Put { key, bytes, reply } => {
                    let mut puts = vec![(key, bytes)];
                    let mut replies = vec![reply];
                    let mut weights = vec![queued.bytes];
                    while commands.peek().is_some_and(|queued| matches!(&queued.command, Command::Put { .. })) {
                        let Some(next) = commands.next() else {
                            break;
                        };
                        if let Command::Put { key, bytes, reply } = next.command {
                            puts.push((key, bytes));
                            replies.push(reply);
                            weights.push(next.bytes);
                        }
                    }
                    let results = state.put_batch(puts);
                    let simulated_crash = results.iter().any(|result| result.as_ref().is_err_and(is_simulated_crash));
                    for ((reply, weight), result) in replies.into_iter().zip(weights).zip(results) {
                        reply_result(reply, result);
                        queue.complete(weight);
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
                Command::SetLimit { limit, reply } => {
                    state.drain_invalid();
                    reply_result(reply, state.set_limit(limit));
                    queue.complete(queued.bytes);
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
