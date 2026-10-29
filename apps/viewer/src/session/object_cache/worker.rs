use std::{
    sync::{Arc, mpsc as std_mpsc},
    thread::JoinHandle,
    time::Duration,
};

use protocol::{
    lludp::{Message, Packet},
    messages::{ObjectUpdate, ObjectUpdateCompressed},
};
use storage::sqlite::{SqliteBatch, SqliteDb, SqliteError};
use tokio::runtime::Handle;

use super::storage::{make_clear_all_batches, make_scope_delete_submission, make_write_batches};
use super::{
    CacheWriteJob, CacheWriteOperation, EncodedEntry, MAX_OBJECT_CACHE_ENTRY_BYTES, MAX_PENDING_SUBMISSIONS, MIN_OBJECT_CACHE_ENTRY_BYTES,
    ObjectCacheEvent,
};
pub(in crate::session) struct CacheWriteWorker {
    pub(super) sender: std_mpsc::SyncSender<CacheWriteJob>,
    thread: JoinHandle<()>,
}

impl CacheWriteWorker {
    pub(super) fn start(db: Arc<SqliteDb>, runtime: Handle) -> std::io::Result<Self> {
        let (sender, receiver) = std_mpsc::sync_channel(MAX_PENDING_SUBMISSIONS);
        let thread = std::thread::Builder::new().name("object-cache-write".to_owned()).spawn(move || {
            let mut invalidation_failed = false;
            while let Ok(job) = receiver.recv() {
                process_write_job(Arc::clone(&db), runtime.clone(), job, &mut invalidation_failed);
            }
        })?;
        Ok(Self { sender, thread })
    }

    pub(super) fn shutdown(self) -> Result<(), String> {
        let Self { sender, thread } = self;
        drop(sender);
        thread.join().map_err(|_| "object cache write worker stopped unexpectedly".to_owned())
    }
}

fn process_write_job(db: Arc<SqliteDb>, runtime: Handle, job: CacheWriteJob, invalidation_failed: &mut bool) {
    let CacheWriteJob { operation, permit } = job;
    let Some(permit) = permit else { return };
    match operation {
        CacheWriteOperation::Update { simulator_id, scope, message, last_used } => {
            if *invalidation_failed {
                drop(permit.send(ObjectCacheEvent::WriteDropped));
                return;
            }
            let entries: Arc<[EncodedEntry]> = encode_entries(&message).into();
            if entries.is_empty() {
                drop(permit.send(ObjectCacheEvent::WriteApplied { simulator_id, scope, entries }));
                return;
            }
            let batches = match make_write_batches(scope.clone(), Arc::clone(&entries), last_used) {
                Ok(batches) => batches,
                Err(_) => {
                    drop(permit.send(ObjectCacheEvent::WriteDropped));
                    return;
                }
            };
            match db.submit_batches(batches) {
                Ok(receipt) => {
                    drop(runtime.spawn(async move {
                        match receipt.wait().await {
                            Ok(()) => drop(permit.send(ObjectCacheEvent::WriteApplied { simulator_id, scope, entries })),
                            Err(_) => drop(permit.send(ObjectCacheEvent::WriteDropped)),
                        }
                    }));
                }
                Err(_) => drop(permit.send(ObjectCacheEvent::WriteDropped)),
            }
        }
        CacheWriteOperation::DeleteScope { scope } => {
            let applied = submit_invalidation(&db, &runtime, || make_scope_delete_submission(scope.clone()).batches);
            if applied {
                drop(permit.send(ObjectCacheEvent::InvalidationApplied));
            } else {
                *invalidation_failed = true;
                drop(permit.send(ObjectCacheEvent::InvalidationDropped));
            }
        }
        CacheWriteOperation::ClearAll => {
            let applied = submit_invalidation(&db, &runtime, make_clear_all_batches);
            if applied {
                drop(permit.send(ObjectCacheEvent::InvalidationApplied));
            } else {
                *invalidation_failed = true;
                drop(permit.send(ObjectCacheEvent::InvalidationDropped));
            }
        }
    }
}

fn submit_invalidation(db: &SqliteDb, runtime: &Handle, mut batches: impl FnMut() -> Vec<SqliteBatch>) -> bool {
    loop {
        let receipt = match db.submit_batches(batches()) {
            Ok(receipt) => receipt,
            Err(SqliteError::Busy | SqliteError::QueueFull) => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(_) => return false,
        };
        match runtime.block_on(receipt.wait()) {
            Ok(()) => return true,
            Err(SqliteError::Busy | SqliteError::QueueFull) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return false,
        }
    }
}

fn encode_entries(message: &Message) -> Vec<EncodedEntry> {
    match message {
        Message::ObjectUpdate(update) => update
            .objects
            .iter()
            .filter_map(|object| {
                let local_id = object.local_id;
                if local_id == 0 {
                    return None;
                }
                let payload =
                    Packet::new(0, Message::ObjectUpdate(ObjectUpdate { region_data: update.region_data, objects: vec![object.clone()] }))
                        .encode()
                        .ok()?;
                (payload.len() >= MIN_OBJECT_CACHE_ENTRY_BYTES && payload.len() <= MAX_OBJECT_CACHE_ENTRY_BYTES).then_some(EncodedEntry {
                    local_id,
                    crc: object.crc,
                    update_kind: 0,
                    update_flags: object.update_flags,
                    payload: Arc::from(payload),
                })
            })
            .collect(),
        Message::ObjectUpdateCompressed(update) => update
            .objects
            .iter()
            .filter_map(|object| {
                let local_id = object.local_id;
                if local_id == 0 {
                    return None;
                }
                let payload = Packet::new(
                    0,
                    Message::ObjectUpdateCompressed(ObjectUpdateCompressed {
                        region_data: update.region_data,
                        objects: vec![object.clone()],
                    }),
                )
                .encode()
                .ok()?;
                (payload.len() >= MIN_OBJECT_CACHE_ENTRY_BYTES && payload.len() <= MAX_OBJECT_CACHE_ENTRY_BYTES).then_some(EncodedEntry {
                    local_id,
                    crc: object.crc,
                    update_kind: 1,
                    update_flags: object.update_flags,
                    payload: Arc::from(payload),
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use network::Grid;
    use protocol::lludp::Message;
    use tokio::runtime::Handle;

    use super::CacheWriteWorker;
    use crate::session::object_cache::storage::open_at;
    use crate::session::object_cache::test_support::*;
    use crate::session::object_cache::{ObjectCacheEvent, ObjectCacheSession};
    #[tokio::test]
    async fn region_reset_deletes_writes_already_queued_for_the_previous_scope() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let path = directory.path().join("objects.sqlite");
        let mut cache = open_cache(&path);
        cache.record_update(SIMULATOR, fixture_message(vectors::OBJECT_UPDATE));
        cache.set_region_identity(SIMULATOR, &region_handshake(NEXT_CACHE_ID));
        flush(&mut cache).await;
        cache.close().await;

        let (db, _) = open_at(&path).expect("reopen cache after region reset");
        assert_eq!(cache_count(&db).await, 0);
    }

    #[tokio::test]
    async fn pending_writes_are_bounded() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let path = directory.path().join("objects.sqlite");
        let (db, status) = open_at(&path).expect("open test database");
        let db = Arc::new(db);
        let writer = CacheWriteWorker::start(Arc::clone(&db), Handle::current()).expect("start cache writer");
        let mut cache = ObjectCacheSession::new(Grid::Agni);
        cache.set_region_identity(SIMULATOR, &region_handshake(CACHE_ID));

        for local_id in 1..=u32::try_from(super::MAX_PENDING_SUBMISSIONS + 1).expect("test write count fits u32") {
            let mut message = fixture_message(vectors::OBJECT_UPDATE);
            let Message::ObjectUpdate(update) = &mut message else {
                panic!("object update fixture should be a full update");
            };
            update.objects.truncate(1);
            update.objects[0].local_id = local_id;
            cache.record_update(SIMULATOR, message);
        }

        assert!(cache.handle_event(ObjectCacheEvent::Ready { db: Arc::clone(&db), status, writer }).is_none());
        flush(&mut cache).await;
        cache.close().await;
        assert_eq!(cache_count(&db).await, super::MAX_PENDING_SUBMISSIONS);
    }
}
