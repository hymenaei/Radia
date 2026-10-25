use std::{
    fs,
    io::{self, Write},
    path::Path,
    thread,
};

use tempfile::{Builder, NamedTempFile};

use super::{FaultHooks, WritePoint};
use crate::cache::{JOURNAL_FILE, SNAPSHOT_FILE, StorageError, format_error, journal};

pub(super) const DATA_TEMP_PREFIX: &str = ".data-tmp-";
pub(super) const SNAPSHOT_TEMP_PREFIX: &str = ".snapshot-tmp-";
pub(super) const JOURNAL_TEMP_PREFIX: &str = ".journal-tmp-";
const TEMP_RETRIES: u32 = 4;

pub(super) fn final_data_name(temp: &NamedTempFile) -> Result<String, StorageError> {
    let base_name = temp
        .path()
        .file_name()
        .ok_or_else(|| StorageError::Io(io::Error::new(io::ErrorKind::InvalidData, "temporary data file has no name")))?;
    let base_name = base_name.to_string_lossy();
    let nonce = base_name
        .strip_prefix(DATA_TEMP_PREFIX)
        .ok_or_else(|| StorageError::Io(io::Error::new(io::ErrorKind::InvalidData, "temporary data file name is invalid")))?;
    let file_name = format!("data-{nonce}.bin");
    if !journal::valid_file_name(&file_name) {
        return Err(StorageError::Io(io::Error::new(io::ErrorKind::InvalidData, "temporary data file name is too long")));
    }
    Ok(file_name)
}

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

pub(super) fn reset_journal_atomic(root: &Path, hooks: &FaultHooks, replaced: impl FnOnce()) -> Result<(), StorageError> {
    let mut temp = Builder::new().prefix(JOURNAL_TEMP_PREFIX).tempfile_in(root)?;
    if let Err(error) = hooks.check(WritePoint::JournalResetTempCreated) {
        return Err(fail_temporary(temp, error));
    }
    let mut header = Vec::new();
    journal::encode_journal_header(&mut header).map_err(format_error)?;
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

pub(super) fn ensure_journal(root: &Path, length: u64) -> Result<u64, StorageError> {
    let path = root.join(JOURNAL_FILE);
    if path.exists() && length >= journal::JOURNAL_HEADER_BYTES {
        return Ok(length);
    }
    let mut header = Vec::new();
    journal::encode_journal_header(&mut header).map_err(format_error)?;
    fs::write(path, header)?;
    Ok(journal::JOURNAL_HEADER_BYTES)
}

pub(super) fn delete_data(directory: &Path, file_name: &str) {
    match fs::remove_file(directory.join(file_name)) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(error = %error, "cache data file cleanup failed"),
    }
}

pub(super) fn remove_path(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) }
}
