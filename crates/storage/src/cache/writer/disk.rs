use std::{
    fs,
    io::{self, Write},
    path::Path,
    thread,
};

use tempfile::{Builder, NamedTempFile};

use super::{FaultHooks, WritePoint};
use crate::cache::{JOURNAL_FILE, Key, NamespaceTable, SNAPSHOT_FILE, StorageError, format_error, journal};

pub(super) const DATA_TEMP_PREFIX: &str = ".data-tmp-";
pub(super) const SNAPSHOT_TEMP_PREFIX: &str = ".snapshot-tmp-";
pub(super) const JOURNAL_TEMP_PREFIX: &str = ".journal-tmp-";
const TEMP_RETRIES: u32 = 4;

pub(super) fn write_snapshot_atomic(root: &Path, bytes: &[u8], hooks: &FaultHooks, replaced: impl FnOnce()) -> Result<(), StorageError> {
    let temp = Builder::new().prefix(SNAPSHOT_TEMP_PREFIX).tempfile_in(root)?;
    let mut temp = temp;
    if let Err(error) = hooks.check(WritePoint::SnapshotTempCreated) {
        return Err(fail_temporary(temp, error));
    }
    temp.write_all(bytes)?;
    if let Err(error) = hooks.check(WritePoint::SnapshotWritten) {
        return Err(fail_temporary(temp, error));
    }
    temp.as_file().sync_all()?;
    if let Err(error) = hooks.check(WritePoint::SnapshotSynced) {
        return Err(fail_temporary(temp, error));
    }
    persist_with_retry(temp, &root.join(SNAPSHOT_FILE)).map_err(StorageError::Io)?;
    replaced();
    hooks.check(WritePoint::SnapshotRenamed).map_err(StorageError::Io)
}

pub(super) fn reset_journal_atomic(
    root: &Path,
    namespaces: &NamespaceTable,
    hooks: &FaultHooks,
    replaced: impl FnOnce(),
) -> Result<(), StorageError> {
    let mut temp = Builder::new().prefix(JOURNAL_TEMP_PREFIX).tempfile_in(root)?;
    if let Err(error) = hooks.check(WritePoint::JournalResetTempCreated) {
        return Err(fail_temporary(temp, error));
    }
    let mut header = Vec::new();
    journal::encode_journal_header(&mut header, namespaces).map_err(format_error)?;
    temp.write_all(&header)?;
    if let Err(error) = hooks.check(WritePoint::JournalResetWritten) {
        return Err(fail_temporary(temp, error));
    }
    temp.as_file().sync_all()?;
    if let Err(error) = hooks.check(WritePoint::JournalResetSynced) {
        return Err(fail_temporary(temp, error));
    }
    persist_with_retry(temp, &root.join(JOURNAL_FILE)).map_err(StorageError::Io)?;
    replaced();
    hooks.check(WritePoint::JournalResetRenamed).map_err(StorageError::Io)
}

pub(super) fn persist_with_retry(mut temp: NamedTempFile, destination: &Path) -> io::Result<()> {
    for attempt in 0..TEMP_RETRIES {
        match temp.persist(destination) {
            Ok(file) => {
                drop(file);
                return Ok(());
            }
            Err(failure) => {
                let error = failure.error;
                if error.kind() != io::ErrorKind::PermissionDenied || attempt + 1 == TEMP_RETRIES {
                    return Err(error);
                }
                temp = failure.file;
                thread::yield_now();
            }
        }
    }
    Err(io::Error::other("temporary file replacement failed"))
}

pub(super) fn fail_temporary(temp: NamedTempFile, error: io::Error) -> StorageError {
    if error.kind() == io::ErrorKind::Interrupted {
        match temp.keep() {
            Ok((file, _)) => drop(file),
            Err(failure) => return StorageError::Io(failure.error),
        }
    }
    StorageError::Io(error)
}

pub(super) fn ensure_journal(root: &Path, length: u64, namespaces: &NamespaceTable) -> Result<u64, StorageError> {
    let path = root.join(JOURNAL_FILE);
    let header_size = journal::journal_header_size(namespaces).ok_or(StorageError::TooLarge)?;
    if path.exists() && length >= header_size {
        return Ok(length);
    }
    let mut header = Vec::new();
    journal::encode_journal_header(&mut header, namespaces).map_err(format_error)?;
    fs::write(path, header)?;
    Ok(header_size)
}

pub(super) fn delete_data(root: &Path, namespaces: &NamespaceTable, key: &Key) {
    let Some(path) = namespaces.path(root, *key) else {
        return;
    };
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(error = %error, "cache data file cleanup failed"),
    }
    let mut directory = path.parent();
    while let Some(current) = directory {
        if current == root {
            break;
        }
        match fs::remove_dir(current) {
            Ok(()) => directory = current.parent(),
            Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty) => break,
            Err(error) => {
                tracing::warn!(error = %error, "cache data directory cleanup failed");
                break;
            }
        }
    }
}

pub(super) fn remove_path(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) }
}
