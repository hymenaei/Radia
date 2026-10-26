use std::{
    any::Any,
    error::Error,
    ffi::OsString,
    fmt,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Condvar, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

use futures::channel::oneshot;
use rusqlite::{Connection, Error as RusqliteError, Transaction, TransactionBehavior, ffi::ErrorCode};

pub const MAX_BATCH_ROWS: usize = 256;
pub const MAX_BATCH_BYTES: usize = 1024 * 1024;
pub const MAX_QUEUED_BATCHES: usize = 64;
pub const MAX_QUEUED_ROWS: usize = 131_072;
pub const MAX_QUEUED_BYTES: usize = 32 * 1024 * 1024;
const BUSY_TIMEOUT: Duration = Duration::from_millis(25);

pub enum SqliteError {
    Busy,
    Corrupt,
    SchemaMismatch,
    QueueFull,
    BatchTooLarge { rows: usize, bytes: usize },
    WorkerStopped,
    Unavailable(std::io::Error),
    Sqlite(RusqliteError),
}

impl fmt::Debug for SqliteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => formatter.write_str("Busy"),
            Self::Corrupt => formatter.write_str("Corrupt"),
            Self::SchemaMismatch => formatter.write_str("SchemaMismatch"),
            Self::QueueFull => formatter.write_str("QueueFull"),
            Self::BatchTooLarge { rows, bytes } => {
                formatter.debug_struct("BatchTooLarge").field("rows", rows).field("bytes", bytes).finish()
            }
            Self::WorkerStopped => formatter.write_str("WorkerStopped"),
            Self::Unavailable(error) => formatter.debug_tuple("Unavailable").field(error).finish(),
            Self::Sqlite(error) => formatter.debug_tuple("Sqlite").field(error).finish(),
        }
    }
}

impl fmt::Display for SqliteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => formatter.write_str("SQLite database is busy"),
            Self::Corrupt => formatter.write_str("SQLite database is corrupt"),
            Self::SchemaMismatch => formatter.write_str("SQLite schema does not match the consumer"),
            Self::QueueFull => formatter.write_str("SQLite worker queue is full"),
            Self::BatchTooLarge { rows, bytes } => write!(formatter, "SQLite batch exceeds limits: {rows} rows, {bytes} bytes"),
            Self::WorkerStopped => formatter.write_str("SQLite worker stopped"),
            Self::Unavailable(error) => write!(formatter, "SQLite database is unavailable: {error}"),
            Self::Sqlite(error) => write!(formatter, "SQLite operation failed: {error}"),
        }
    }
}

impl Error for SqliteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unavailable(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for SqliteError {
    fn from(error: std::io::Error) -> Self {
        Self::Unavailable(error)
    }
}

#[derive(Debug)]
pub enum MigrationError {
    SchemaMismatch,
    Sqlite(RusqliteError),
}

impl From<RusqliteError> for MigrationError {
    fn from(error: RusqliteError) -> Self {
        Self::Sqlite(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryReason {
    Corrupt,
    SchemaMismatch,
    ConsumerDiscard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenStatus {
    Created,
    Existing,
    Recreated(RecoveryReason),
}

type BatchAction = dyn for<'transaction> FnOnce(&Transaction<'transaction>) -> Result<(), RusqliteError> + Send + 'static;

pub struct SqliteBatch {
    rows: usize,
    bytes: usize,
    run: Box<BatchAction>,
}

impl SqliteBatch {
    pub fn new<F>(rows: usize, bytes: usize, run: F) -> Result<Self, SqliteError>
    where
        F: for<'transaction> FnOnce(&Transaction<'transaction>) -> Result<(), RusqliteError> + Send + 'static,
    {
        if rows > MAX_BATCH_ROWS || bytes > MAX_BATCH_BYTES {
            return Err(SqliteError::BatchTooLarge { rows, bytes });
        }
        Ok(Self { rows, bytes, run: Box::new(run) })
    }
}

impl fmt::Debug for SqliteBatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("SqliteBatch").field("rows", &self.rows).field("bytes", &self.bytes).finish()
    }
}

pub struct SqliteReceipt<T>(oneshot::Receiver<Result<T, SqliteError>>);

impl<T> fmt::Debug for SqliteReceipt<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SqliteReceipt")
    }
}

impl<T> SqliteReceipt<T> {
    pub async fn wait(self) -> Result<T, SqliteError> {
        self.0.await.unwrap_or(Err(SqliteError::WorkerStopped))
    }
}

type WorkResult = Result<Box<dyn Any + Send>, SqliteError>;
type Work = Box<dyn FnOnce(&mut Connection) -> WorkResult + Send + 'static>;
type Completion = Box<dyn FnOnce(WorkResult) + Send + 'static>;
type Migration = dyn Fn(&mut Connection) -> Result<(), MigrationError> + Send + Sync + 'static;

struct QueuedWork {
    work: Work,
    completion: Completion,
    bytes: usize,
    rows: usize,
    control: bool,
    write: bool,
    reset: bool,
}

#[derive(Default)]
struct QueueState {
    work: std::collections::VecDeque<QueuedWork>,
    pending_bytes: usize,
    pending_rows: usize,
    pending_batches: usize,
    control_pending: bool,
    blocked: bool,
    closed: bool,
}

#[derive(Default)]
struct WorkQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

impl WorkQueue {
    fn try_send(&self, queued: QueuedWork) -> Result<(), SqliteError> {
        let mut state = lock(&self.state);
        if state.closed {
            return Err(SqliteError::WorkerStopped);
        }
        if queued.control {
            if state.control_pending {
                return Err(SqliteError::QueueFull);
            }
            state.control_pending = true;
            state.blocked = true;
        } else {
            let Some(next_bytes) = state.pending_bytes.checked_add(queued.bytes) else {
                return Err(SqliteError::QueueFull);
            };
            let Some(next_rows) = state.pending_rows.checked_add(queued.rows) else {
                return Err(SqliteError::QueueFull);
            };
            if state.blocked || state.pending_batches >= MAX_QUEUED_BATCHES || next_bytes > MAX_QUEUED_BYTES || next_rows > MAX_QUEUED_ROWS
            {
                return Err(SqliteError::QueueFull);
            }
            state.pending_batches += 1;
            state.pending_bytes = next_bytes;
            state.pending_rows = next_rows;
        }
        state.work.push_back(queued);
        self.ready.notify_one();
        Ok(())
    }

    fn receive(&self) -> Vec<QueuedWork> {
        let mut state = lock(&self.state);
        while state.work.is_empty() && !state.closed {
            state = self.ready.wait(state).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        state.work.drain(..).collect()
    }

    fn complete(&self, bytes: usize, rows: usize, control: bool) {
        let mut state = lock(&self.state);
        if control {
            state.control_pending = false;
            state.blocked = false;
        } else {
            state.pending_batches = state.pending_batches.saturating_sub(1);
            state.pending_bytes = state.pending_bytes.saturating_sub(bytes);
            state.pending_rows = state.pending_rows.saturating_sub(rows);
        }
    }

    fn close(&self) {
        let mut state = lock(&self.state);
        state.closed = true;
        self.ready.notify_all();
    }
}

pub struct SqliteDb {
    path: PathBuf,
    queue: Arc<WorkQueue>,
    worker: Option<JoinHandle<()>>,
    failed_writes: Arc<AtomicBool>,
}

impl fmt::Debug for SqliteDb {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("SqliteDb").field("path", &self.path).finish()
    }
}

impl SqliteDb {
    pub fn open<F>(path: impl AsRef<Path>, migrate: F) -> Result<(Self, OpenStatus), SqliteError>
    where
        F: Fn(&mut Connection) -> Result<(), MigrationError> + Send + Sync + 'static,
    {
        let path = path.as_ref().to_path_buf();
        let queue = Arc::new(WorkQueue::default());
        let worker_queue = Arc::clone(&queue);
        let migration: Arc<Migration> = Arc::new(migrate);
        let thread_migration = Arc::clone(&migration);
        let failed_writes = Arc::new(AtomicBool::new(false));
        let worker_failures = Arc::clone(&failed_writes);
        let worker_path = path.clone();
        let (ready, started) = mpsc::sync_channel(0);
        let worker = thread::Builder::new().name("storage-sqlite".to_owned()).spawn(move || {
            match open_database(&worker_path, thread_migration.as_ref()) {
                Ok(opened) => {
                    if ready.send(Ok(opened.status)).is_err() {
                        return;
                    }
                    worker_loop(&worker_path, thread_migration, &worker_queue, worker_failures, opened);
                }
                Err(error) => {
                    if let Err(send_error) = ready.send(Err(error)) {
                        drop(send_error);
                    }
                }
            }
        })?;
        let status = match started.recv() {
            Ok(Ok(status)) => status,
            Ok(Err(error)) => {
                if worker.join().is_err() {
                    return Err(SqliteError::WorkerStopped);
                }
                return Err(error);
            }
            Err(_) => {
                if worker.join().is_err() {
                    return Err(SqliteError::WorkerStopped);
                }
                return Err(SqliteError::WorkerStopped);
            }
        };
        Ok((Self { path, queue, worker: Some(worker), failed_writes }, status))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn had_write_failure(&self) -> bool {
        self.failed_writes.load(Ordering::Relaxed)
    }

    pub fn shutdown(mut self) -> Result<(), SqliteError> {
        self.queue.close();
        match self.worker.take() {
            Some(worker) => worker.join().map_err(|_| SqliteError::WorkerStopped),
            None => Ok(()),
        }
    }

    pub fn submit_batches(&self, batches: Vec<SqliteBatch>) -> Result<SqliteReceipt<()>, SqliteError> {
        self.submit(batches, false)
    }

    pub fn submit_control_batch(&self, batch: SqliteBatch) -> Result<SqliteReceipt<()>, SqliteError> {
        self.submit(vec![batch], true)
    }

    pub fn read<F, T>(&self, weight_bytes: usize, query: F) -> Result<SqliteReceipt<T>, SqliteError>
    where
        F: for<'transaction> FnOnce(&Transaction<'transaction>) -> Result<T, RusqliteError> + Send + 'static,
        T: Send + 'static,
    {
        if weight_bytes > MAX_QUEUED_BYTES {
            return Err(SqliteError::QueueFull);
        }
        let (reply, receiver) = oneshot::channel();
        let work = Box::new(move |connection: &mut Connection| {
            let result = (|| {
                let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
                let value = query(&transaction)?;
                transaction.commit()?;
                Ok(value)
            })()
            .map_err(map_sqlite_error);
            result.map(|value| Box::new(value) as Box<dyn Any + Send>)
        });
        let completion = Box::new(move |result: WorkResult| {
            let result = match result {
                Ok(value) => value.downcast::<T>().map(|value| *value).map_err(|_| SqliteError::WorkerStopped),
                Err(error) => Err(error),
            };
            if let Err(send_error) = reply.send(result) {
                drop(send_error);
            }
        });
        self.queue.try_send(QueuedWork { work, completion, bytes: weight_bytes, rows: 0, control: false, write: false, reset: false })?;
        Ok(SqliteReceipt(receiver))
    }

    pub fn discard(&self) -> Result<SqliteReceipt<()>, SqliteError> {
        let (reply, receiver) = oneshot::channel();
        let work: Work = Box::new(|_: &mut Connection| Ok(Box::new(()) as Box<dyn Any + Send>));
        let completion = Box::new(move |result: WorkResult| {
            let result = result.map(|_| ());
            if let Err(send_error) = reply.send(result) {
                drop(send_error);
            }
        });
        self.queue.try_send(QueuedWork { work, completion, bytes: 0, rows: 0, control: true, write: true, reset: true })?;
        Ok(SqliteReceipt(receiver))
    }

    fn submit(&self, batches: Vec<SqliteBatch>, control: bool) -> Result<SqliteReceipt<()>, SqliteError> {
        let bytes = batches.iter().try_fold(0usize, |total, batch| total.checked_add(batch.bytes)).ok_or(SqliteError::QueueFull)?;
        let rows = batches.iter().try_fold(0usize, |total, batch| total.checked_add(batch.rows)).ok_or(SqliteError::QueueFull)?;
        if bytes > MAX_QUEUED_BYTES {
            return Err(SqliteError::QueueFull);
        }
        let (reply, receiver) = oneshot::channel();
        let work = Box::new(move |connection: &mut Connection| {
            let result = run_batches(connection, batches);
            result.map(|()| Box::new(()) as Box<dyn Any + Send>)
        });
        let completion = Box::new(move |result: WorkResult| {
            let result = result.map(|_| ());
            if let Err(send_error) = reply.send(result) {
                drop(send_error);
            }
        });
        self.queue.try_send(QueuedWork { work, completion, bytes, rows, control, write: true, reset: false })?;
        Ok(SqliteReceipt(receiver))
    }
}

impl Drop for SqliteDb {
    fn drop(&mut self) {
        self.queue.close();
        if let Some(worker) = self.worker.take() {
            drop(worker);
        }
    }
}

struct OpenedDatabase {
    connection: Connection,
    lock: File,
    status: OpenStatus,
}

fn open_database(path: &Path, migrate: &Migration) -> Result<OpenedDatabase, SqliteError> {
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let existed = path.try_exists()?;
    let shared = open_shared_lock(path)?;
    let attempt = open_connection(path).and_then(|mut connection| {
        check_database(&connection)?;
        run_migration(&mut connection, migrate)?;
        Ok(connection)
    });
    match attempt {
        Ok(connection) => {
            Ok(OpenedDatabase { connection, lock: shared, status: if existed { OpenStatus::Existing } else { OpenStatus::Created } })
        }
        Err(error) if recovery_reason(&error).is_some() => {
            drop(shared);
            recreate_database(path, migrate, recovery_reason(&error).ok_or(SqliteError::Corrupt)?)
        }
        Err(error) => Err(error),
    }
}

fn recreate_database(path: &Path, migrate: &Migration, reason: RecoveryReason) -> Result<OpenedDatabase, SqliteError> {
    let exclusive = open_lock_file(path)?;
    match exclusive.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Err(SqliteError::Busy),
        Err(std::fs::TryLockError::Error(error)) => return Err(SqliteError::Unavailable(error)),
    }
    remove_database_files(path)?;
    let mut connection = open_connection(path)?;
    check_database(&connection)?;
    run_migration(&mut connection, migrate)?;
    drop(connection);
    drop(exclusive);
    let shared = open_shared_lock(path)?;
    let connection = open_connection(path)?;
    check_database(&connection)?;
    Ok(OpenedDatabase { connection, lock: shared, status: OpenStatus::Recreated(reason) })
}

fn open_connection(path: &Path) -> Result<Connection, SqliteError> {
    let connection = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_CREATE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(map_sqlite_error)?;
    connection.busy_timeout(BUSY_TIMEOUT).map_err(map_sqlite_error)?;
    let journal_mode: String = connection.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0)).map_err(map_sqlite_error)?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(SqliteError::Sqlite(RusqliteError::InvalidQuery));
    }
    connection.pragma_update(None, "synchronous", "NORMAL").map_err(map_sqlite_error)?;
    Ok(connection)
}

fn check_database(connection: &Connection) -> Result<(), SqliteError> {
    let mut statement = connection.prepare("PRAGMA quick_check").map_err(map_sqlite_error)?;
    let mut rows = statement.query([]).map_err(map_sqlite_error)?;
    let mut checked = false;
    while let Some(row) = rows.next().map_err(map_sqlite_error)? {
        checked = true;
        let result: String = row.get(0).map_err(map_sqlite_error)?;
        if result != "ok" {
            return Err(SqliteError::Corrupt);
        }
    }
    if checked { Ok(()) } else { Err(SqliteError::Corrupt) }
}

fn run_migration(connection: &mut Connection, migrate: &Migration) -> Result<(), SqliteError> {
    match migrate(connection) {
        Ok(()) => Ok(()),
        Err(MigrationError::SchemaMismatch) => Err(SqliteError::SchemaMismatch),
        Err(MigrationError::Sqlite(error)) => Err(map_sqlite_error(error)),
    }
}

fn open_shared_lock(path: &Path) -> Result<File, SqliteError> {
    let file = open_lock_file(path)?;
    match file.try_lock_shared() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(SqliteError::Busy),
        Err(std::fs::TryLockError::Error(error)) => Err(SqliteError::Unavailable(error)),
    }
}

fn open_lock_file(path: &Path) -> Result<File, SqliteError> {
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut lock_name: OsString = path.file_name().unwrap_or_default().to_os_string();
    lock_name.push(".maintenance.lock");
    Ok(OpenOptions::new().create(true).truncate(false).read(true).write(true).open(parent.join(lock_name))?)
}

fn remove_database_files(path: &Path) -> Result<(), SqliteError> {
    for suffix in ["", "-wal", "-shm"] {
        let mut file_name = path.as_os_str().to_os_string();
        file_name.push(suffix);
        match fs::remove_file(PathBuf::from(file_name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(SqliteError::Unavailable(error)),
        }
    }
    Ok(())
}

fn recovery_reason(error: &SqliteError) -> Option<RecoveryReason> {
    match error {
        SqliteError::Corrupt => Some(RecoveryReason::Corrupt),
        SqliteError::SchemaMismatch => Some(RecoveryReason::SchemaMismatch),
        _ => None,
    }
}

fn map_sqlite_error(error: RusqliteError) -> SqliteError {
    if matches!(
        &error,
        RusqliteError::FromSqlConversionFailure(..)
            | RusqliteError::InvalidColumnIndex(..)
            | RusqliteError::InvalidColumnName(..)
            | RusqliteError::InvalidColumnType(..)
            | RusqliteError::IntegralValueOutOfRange(..)
    ) {
        return SqliteError::Corrupt;
    }
    let RusqliteError::SqliteFailure(details, _) = &error else {
        return SqliteError::Sqlite(error);
    };
    match details.code {
        ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => SqliteError::Busy,
        ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => SqliteError::Corrupt,
        ErrorCode::SchemaChanged => SqliteError::SchemaMismatch,
        _ => SqliteError::Sqlite(error),
    }
}

fn run_batches(connection: &mut Connection, batches: Vec<SqliteBatch>) -> Result<(), SqliteError> {
    for batch in batches {
        let transaction = connection.transaction().map_err(map_sqlite_error)?;
        (batch.run)(&transaction).map_err(map_sqlite_error)?;
        transaction.commit().map_err(map_sqlite_error)?;
    }
    Ok(())
}

struct WorkerDatabase {
    connection: Option<Connection>,
    lock: Option<File>,
}

fn worker_loop(path: &Path, migrate: Arc<Migration>, queue: &WorkQueue, failed_writes: Arc<AtomicBool>, opened: OpenedDatabase) {
    let mut database = WorkerDatabase { connection: Some(opened.connection), lock: Some(opened.lock) };
    loop {
        let work = queue.receive();
        if work.is_empty() {
            break;
        }
        for queued in work {
            let QueuedWork { work, completion, bytes, rows, control, write, reset } = queued;
            let result = if reset {
                reset_database(path, migrate.as_ref(), &mut database)
            } else {
                run_work(path, migrate.as_ref(), &mut database, work)
            };
            if !reset && let Some(reason) = recovery_reason_from_result(&result) {
                let recovery = reopen_after_damage(path, migrate.as_ref(), &mut database, reason);
                if recovery.is_err() {
                    database.connection = None;
                    database.lock = None;
                }
            }
            if reset && result.is_ok() {
                failed_writes.store(false, Ordering::Relaxed);
            } else if write && result.is_err() {
                failed_writes.store(true, Ordering::Relaxed);
            }
            completion(result);
            queue.complete(bytes, rows, control);
        }
    }
    drop(database.connection.take());
    drop(database.lock.take());
}

fn run_work(path: &Path, migrate: &Migration, database: &mut WorkerDatabase, work: Work) -> WorkResult {
    if database.connection.is_none() {
        let opened = open_database(path, migrate)?;
        database.connection = Some(opened.connection);
        database.lock = Some(opened.lock);
    }
    match database.connection.as_mut() {
        Some(connection) => work(connection),
        None => Err(SqliteError::WorkerStopped),
    }
}

fn recovery_reason_from_result(result: &WorkResult) -> Option<RecoveryReason> {
    match result {
        Err(SqliteError::Corrupt) => Some(RecoveryReason::Corrupt),
        Err(SqliteError::SchemaMismatch) => Some(RecoveryReason::SchemaMismatch),
        _ => None,
    }
}

fn reopen_after_damage(path: &Path, migrate: &Migration, database: &mut WorkerDatabase, reason: RecoveryReason) -> Result<(), SqliteError> {
    drop(database.connection.take());
    drop(database.lock.take());
    match recreate_database(path, migrate, reason) {
        Ok(opened) => {
            database.connection = Some(opened.connection);
            database.lock = Some(opened.lock);
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn reset_database(path: &Path, migrate: &Migration, database: &mut WorkerDatabase) -> WorkResult {
    drop(database.connection.take());
    drop(database.lock.take());
    match recreate_database(path, migrate, RecoveryReason::ConsumerDiscard) {
        Ok(opened) => {
            database.connection = Some(opened.connection);
            database.lock = Some(opened.lock);
            Ok(Box::new(()) as Box<dyn Any + Send>)
        }
        Err(error) => Err(error),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
#[path = "sqlite_tests.rs"]
mod tests;
