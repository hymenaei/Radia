use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, Read},
    path::Path,
};

use super::{JOURNAL_FILE, Key, SNAPSHOT_FILE, StorageError, UnavailableReason, format_error};
use crate::cache::journal::{self, FormatError, Operation, Record};

pub(super) fn recover(root: &Path) -> Result<(Vec<Record>, u64, u64, u64, bool), StorageError> {
    let snapshot_path = root.join(SNAPSHOT_FILE);
    let (snapshot, snapshot_bytes) = match read_bounded(&snapshot_path, journal::MAX_SNAPSHOT_BYTES) {
        Ok(Some(bytes)) => match journal::decode_snapshot(&bytes) {
            Ok(snapshot) => (Some(snapshot), u64::try_from(bytes.len()).map_err(|_| StorageError::TooLarge)?),
            Err(FormatError::Allocation) => return Err(StorageError::Unavailable(UnavailableReason::Allocation)),
            Err(FormatError::Invalid | FormatError::UnsupportedVersion | FormatError::TooLarge) => return Ok((Vec::new(), 0, 0, 0, true)),
        },
        Ok(None) => (None, 0),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => return Ok((Vec::new(), 0, 0, 0, true)),
        Err(error) if error.kind() == io::ErrorKind::OutOfMemory => {
            return Err(StorageError::Unavailable(UnavailableReason::Allocation));
        }
        Err(error) => return Err(StorageError::Io(error)),
    };
    let journal_path = root.join(JOURNAL_FILE);
    let journal_bytes = match read_bounded(&journal_path, journal::MAX_JOURNAL_BYTES) {
        Ok(Some(bytes)) => Some(bytes),
        Ok(None) => None,
        Err(error) if error.kind() == io::ErrorKind::InvalidData => return Ok((Vec::new(), 0, 0, 0, true)),
        Err(error) if error.kind() == io::ErrorKind::OutOfMemory => {
            return Err(StorageError::Unavailable(UnavailableReason::Allocation));
        }
        Err(error) => return Err(StorageError::Io(error)),
    };
    let snapshot_sequence = snapshot.as_ref().map(|snapshot| snapshot.sequence).unwrap_or(0);
    let mut records = snapshot.map(|snapshot| snapshot.records).unwrap_or_default();
    let mut by_key = HashMap::new();
    by_key.try_reserve(records.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
    for record in records.drain(..) {
        by_key.insert(record.key, record);
    }
    let mut names = HashSet::new();
    names.try_reserve(by_key.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
    for record in by_key.values() {
        names.insert(record.file_name.clone());
    }
    let (sequence, journal_len) = match journal_bytes {
        Some(bytes) if bytes.len() < usize::try_from(journal::JOURNAL_HEADER_BYTES).map_err(|_| StorageError::TooLarge)? => {
            (snapshot_sequence, write_journal_header(root)?)
        }
        Some(bytes) => {
            match journal::decode_journal(&bytes, snapshot_sequence, |_, operation| apply_operation(&mut by_key, &mut names, operation)) {
                Ok(replay) => {
                    let valid = replay.valid_bytes;
                    if valid < bytes.len() {
                        OpenOptions::new()
                            .write(true)
                            .open(&journal_path)?
                            .set_len(u64::try_from(valid).map_err(|_| StorageError::TooLarge)?)?;
                    }
                    (replay.sequence, u64::try_from(valid).map_err(|_| StorageError::TooLarge)?)
                }
                Err(FormatError::Allocation) => return Err(StorageError::Unavailable(UnavailableReason::Allocation)),
                Err(FormatError::UnsupportedVersion | FormatError::Invalid | FormatError::TooLarge) => {
                    return Ok((Vec::new(), 0, 0, 0, true));
                }
            }
        }
        None => (snapshot_sequence, 0),
    };
    let mut records = Vec::new();
    records.try_reserve_exact(by_key.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
    records.extend(by_key.into_values());
    if records.len() > journal::MAX_ENTRIES {
        return Ok((Vec::new(), 0, 0, 0, true));
    }
    Ok((records, sequence, snapshot_bytes, journal_len, false))
}

fn apply_operation(by_key: &mut HashMap<Key, Record>, names: &mut HashSet<String>, operation: Operation) -> Result<(), FormatError> {
    match operation {
        Operation::Put(record) => {
            if let Some(old) = by_key.remove(&record.key) {
                names.remove(&old.file_name);
            }
            if names.contains(&record.file_name) || by_key.len() >= journal::MAX_ENTRIES {
                return Err(FormatError::Invalid);
            }
            names.try_reserve(1).map_err(|_| FormatError::Allocation)?;
            by_key.try_reserve(1).map_err(|_| FormatError::Allocation)?;
            names.insert(record.file_name.clone());
            by_key.insert(record.key, record);
        }
        Operation::Remove(key) => {
            if let Some(old) = by_key.remove(&key) {
                names.remove(&old.file_name);
            }
        }
        Operation::Clear => {
            by_key.clear();
            names.clear();
        }
    }
    Ok(())
}

fn write_journal_header(root: &Path) -> Result<u64, StorageError> {
    let path = root.join(JOURNAL_FILE);
    let mut header = Vec::new();
    journal::encode_journal_header(&mut header).map_err(format_error)?;
    fs::write(path, header)?;
    Ok(journal::JOURNAL_HEADER_BYTES)
}

pub(super) fn discard_index_files(root: &Path) -> io::Result<()> {
    for name in [SNAPSHOT_FILE, JOURNAL_FILE] {
        match fs::remove_file(root.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn read_bounded(path: &Path, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    let limit_u64 = u64::try_from(limit).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "cache index limit is invalid"))?;
    if metadata.len() > limit_u64 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "cache index exceeds its format bound"));
    }
    let capacity =
        usize::try_from(metadata.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "cache index length is invalid"))?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(capacity).map_err(|_| io::Error::new(io::ErrorKind::OutOfMemory, "cache index allocation failed"))?;
    file.take(limit_u64.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "cache index exceeds its format bound"));
    }
    Ok(Some(bytes))
}
