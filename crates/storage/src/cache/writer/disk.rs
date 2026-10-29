use std::{fs, io, path::Path, thread};

use tempfile::NamedTempFile;

use crate::cache::{Key, NamespaceTable, StorageError};

pub(super) const DATA_TEMP_PREFIX: &str = ".data-tmp-";
const TEMP_RETRIES: u32 = 4;

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
