use std::{
    error::Error,
    fs,
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    sync::mpsc,
};

use futures::executor::block_on;
use rusqlite::{Connection, Error as RusqliteError, Transaction};
use tempfile::tempdir;

use super::*;

fn migrate(connection: &mut Connection) -> Result<(), MigrationError> {
    connection.execute_batch("CREATE TABLE IF NOT EXISTS values_table (value INTEGER PRIMARY KEY)")?;
    Ok(())
}

fn versioned_migration(version: i64) -> impl Fn(&mut Connection) -> Result<(), MigrationError> + Send + Sync + 'static {
    move |connection| {
        let current: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if current == 0 {
            connection.execute_batch("CREATE TABLE versioned (value INTEGER PRIMARY KEY)")?;
            connection.pragma_update(None, "user_version", version)?;
            Ok(())
        } else if current == version {
            Ok(())
        } else {
            Err(MigrationError::SchemaMismatch)
        }
    }
}

fn insert(value: i64) -> Result<SqliteBatch, SqliteError> {
    SqliteBatch::new(1, 8, move |transaction| {
        transaction.execute("INSERT INTO values_table(value) VALUES (?1)", [value])?;
        Ok(())
    })
}

#[test]
fn batch_bounds_are_enforced() {
    std::assert_matches!(
        SqliteBatch::new(MAX_BATCH_ROWS + 1, 1, |_: &Transaction<'_>| Ok(())),
        Err(SqliteError::BatchTooLarge { rows, bytes: 1 }) if rows == MAX_BATCH_ROWS + 1
    );
    std::assert_matches!(
        SqliteBatch::new(1, MAX_BATCH_BYTES + 1, |_: &Transaction<'_>| Ok(())),
        Err(SqliteError::BatchTooLarge { rows: 1, bytes }) if bytes == MAX_BATCH_BYTES + 1
    );
}

#[test]
fn garbage_database_is_recreated() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("inventory.sqlite");
    fs::write(&path, b"not sqlite data")?;

    let (db, status) = SqliteDb::open(&path, migrate)?;

    assert_eq!(status, OpenStatus::Recreated(RecoveryReason::Corrupt));
    let receipt = db.read(0, |transaction| transaction.query_row("SELECT COUNT(*) FROM values_table", [], |row| row.get::<_, i64>(0)))?;
    assert_eq!(block_on(receipt.wait())?, 0);
    db.shutdown()?;
    Ok(())
}

#[test]
fn remove_if_idle_keeps_an_open_database_and_removes_it_after_shutdown() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("inventory.sqlite");
    let (db, _) = SqliteDb::open(&path, migrate)?;

    assert!(!SqliteDb::remove_if_idle(&path)?);
    assert!(path.exists());

    db.shutdown()?;
    assert!(SqliteDb::remove_if_idle(&path)?);
    assert!(!path.exists());
    Ok(())
}

#[test]
fn schema_mismatch_is_recreated() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("inventory.sqlite");
    let (first, status) = SqliteDb::open(&path, versioned_migration(1))?;
    assert_eq!(status, OpenStatus::Created);
    first.shutdown()?;

    let (second, status) = SqliteDb::open(&path, versioned_migration(2))?;

    assert_eq!(status, OpenStatus::Recreated(RecoveryReason::SchemaMismatch));
    let receipt = second.read(0, |transaction| transaction.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)))?;
    assert_eq!(block_on(receipt.wait())?, 2);
    second.shutdown()?;
    Ok(())
}

#[test]
fn full_queue_admits_control_work() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let (db, _) = SqliteDb::open(directory.path().join("inventory.sqlite"), migrate)?;
    let (started, started_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let blocker = SqliteBatch::new(1, 8, move |transaction| {
        transaction.execute("INSERT INTO values_table(value) VALUES (0)", [])?;
        started.send(()).map_err(|_| RusqliteError::InvalidQuery)?;
        release_rx.recv().map_err(|_| RusqliteError::InvalidQuery)?;
        Ok(())
    })?;
    let blocker_receipt = db.submit_batches(vec![blocker])?;
    started_rx.recv()?;

    let mut queued = Vec::new();
    for value in 1..MAX_QUEUED_BATCHES {
        queued.push(db.submit_batches(vec![insert(value as i64)?])?);
    }
    std::assert_matches!(db.submit_batches(vec![insert(1000)?]), Err(SqliteError::QueueFull));

    let control = SqliteBatch::new(1, 8, |transaction| {
        transaction.execute("UPDATE values_table SET value=10000 WHERE value=0", [])?;
        Ok(())
    })?;
    let control_receipt = db.submit_control_batch(control)?;
    std::assert_matches!(db.submit_batches(vec![insert(1001)?]), Err(SqliteError::QueueFull));

    release.send(())?;
    block_on(blocker_receipt.wait())?;
    for receipt in queued {
        block_on(receipt.wait())?;
    }
    block_on(control_receipt.wait())?;
    let read = db.read(0, |transaction| transaction.query_row("SELECT COUNT(*) FROM values_table", [], |row| row.get::<_, i64>(0)))?;
    assert_eq!(block_on(read.wait())?, MAX_QUEUED_BATCHES as i64);
    db.shutdown()?;
    Ok(())
}

#[test]
fn second_process_write_returns_busy() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("inventory.sqlite");
    let (db, _) = SqliteDb::open(&path, migrate)?;
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("sqlite::tests::busy_process_helper")
        .arg("--nocapture")
        .env("RADIA_SQLITE_BUSY_PATH", &path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("child stdout missing")?;
    let mut locked = false;
    let mut stdout = BufReader::new(stdout);
    for line in stdout.by_ref().lines() {
        if line?.trim() == "sqlite write locked" {
            locked = true;
            break;
        }
    }
    assert!(locked, "child process did not acquire the SQLite write lock");

    let receipt = db.submit_batches(vec![insert(55)?])?;
    std::assert_matches!(block_on(receipt.wait()), Err(SqliteError::Busy));

    child.stdin.take().ok_or("child stdin missing")?.write_all(b"continue\n")?;
    let mut remaining_output = String::new();
    stdout.read_to_string(&mut remaining_output)?;
    assert!(child.wait()?.success());
    db.shutdown()?;
    Ok(())
}

#[test]
fn repair_waits_for_other_process() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let path = directory.path().join("inventory.sqlite");
    let (initial, _) = SqliteDb::open(&path, versioned_migration(1))?;
    initial.shutdown()?;
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("sqlite::tests::maintenance_process_helper")
        .arg("--nocapture")
        .env("RADIA_SQLITE_MAINTENANCE_PATH", &path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("child stdout missing")?;
    let mut stdout = BufReader::new(stdout);
    let mut held = false;
    for line in stdout.by_ref().lines() {
        if line?.trim() == "sqlite maintenance held" {
            held = true;
            break;
        }
    }
    assert!(held, "child process did not retain the database lock");

    std::assert_matches!(SqliteDb::open(&path, versioned_migration(2)), Err(SqliteError::Busy));
    assert!(path.exists(), "failed repair removed a database another process holds open");

    child.stdin.take().ok_or("child stdin missing")?.write_all(b"continue\n")?;
    let mut remaining_output = String::new();
    stdout.read_to_string(&mut remaining_output)?;
    assert!(child.wait()?.success());
    let (repaired, status) = SqliteDb::open(&path, versioned_migration(2))?;
    assert_eq!(status, OpenStatus::Recreated(RecoveryReason::SchemaMismatch));
    repaired.shutdown()?;
    Ok(())
}

#[test]
fn busy_process_helper() -> Result<(), Box<dyn Error>> {
    let Some(path) = std::env::var_os("RADIA_SQLITE_BUSY_PATH") else {
        return Ok(());
    };
    let path = std::path::PathBuf::from(path);
    let (db, _) = SqliteDb::open(path, migrate)?;
    let batch = SqliteBatch::new(1, 8, |transaction: &Transaction<'_>| {
        transaction.execute("INSERT INTO values_table(value) VALUES (99)", [])?;
        println!("sqlite write locked");
        std::io::stdout().flush().map_err(|_| RusqliteError::InvalidQuery)?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).map_err(|_| RusqliteError::InvalidQuery)?;
        Ok(())
    });
    let receipt = db.submit_batches(vec![batch?])?;
    block_on(receipt.wait())?;
    db.shutdown()?;
    Ok(())
}

#[test]
fn maintenance_process_helper() -> Result<(), Box<dyn Error>> {
    let Some(path) = std::env::var_os("RADIA_SQLITE_MAINTENANCE_PATH") else {
        return Ok(());
    };
    let (db, _) = SqliteDb::open(path, versioned_migration(1))?;
    let batch = SqliteBatch::new(1, 8, |transaction: &Transaction<'_>| {
        transaction.execute("INSERT INTO versioned(value) VALUES (1)", [])?;
        println!("sqlite maintenance held");
        std::io::stdout().flush().map_err(|_| RusqliteError::InvalidQuery)?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).map_err(|_| RusqliteError::InvalidQuery)?;
        Ok(())
    });
    let receipt = db.submit_batches(vec![batch?])?;
    block_on(receipt.wait())?;
    db.shutdown()?;
    Ok(())
}
