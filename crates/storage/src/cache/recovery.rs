use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read},
    path::Path,
    sync::Arc,
};

use super::{Entry, JOURNAL_FILE, Key, NamespaceTable, SNAPSHOT_FILE, StorageError, UnavailableReason, format_error};
use crate::cache::journal::{self, FormatError, Operation};

type RecoveredEntries = HashMap<Key, Arc<Entry>>;

pub(super) fn recover(root: &Path, namespaces: &NamespaceTable) -> Result<(RecoveredEntries, u64, u64, u64, bool, bool), StorageError> {
    let snapshot_path = root.join(SNAPSHOT_FILE);
    let (snapshot, snapshot_bytes) = match read_bounded(&snapshot_path, journal::MAX_SNAPSHOT_BYTES) {
        Ok(Some(bytes)) => match journal::decode_snapshot(&bytes, namespaces) {
            Ok(snapshot) => (Some(snapshot), u64::try_from(bytes.len()).map_err(|_| StorageError::TooLarge)?),
            Err(FormatError::Allocation) => return Err(StorageError::Unavailable(UnavailableReason::Allocation)),
            Err(FormatError::Invalid | FormatError::UnsupportedVersion | FormatError::TruncatedHeader | FormatError::TooLarge) => {
                return Ok((HashMap::new(), 0, 0, 0, true, false));
            }
        },
        Ok(None) => (None, 0),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => return Ok((HashMap::new(), 0, 0, 0, true, false)),
        Err(error) if error.kind() == io::ErrorKind::OutOfMemory => {
            return Err(StorageError::Unavailable(UnavailableReason::Allocation));
        }
        Err(error) => return Err(StorageError::Io(error)),
    };
    let journal_path = root.join(JOURNAL_FILE);
    let journal_bytes = match read_bounded(&journal_path, journal::MAX_JOURNAL_BYTES) {
        Ok(Some(bytes)) => Some(bytes),
        Ok(None) => None,
        Err(error) if error.kind() == io::ErrorKind::InvalidData => return Ok((HashMap::new(), 0, 0, 0, true, false)),
        Err(error) if error.kind() == io::ErrorKind::OutOfMemory => {
            return Err(StorageError::Unavailable(UnavailableReason::Allocation));
        }
        Err(error) => return Err(StorageError::Io(error)),
    };
    let mut namespace_table_changed = snapshot.as_ref().is_some_and(|snapshot| !snapshot.namespace_table_matches);
    let snapshot_sequence = snapshot.as_ref().map(|snapshot| snapshot.sequence).unwrap_or(0);
    let records = snapshot.map(|snapshot| snapshot.records).unwrap_or_default();
    let mut by_key = HashMap::new();
    by_key.try_reserve(records.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
    for record in records {
        if by_key.insert(record.key, Arc::new(Entry::from_record(record))).is_some() {
            return Ok((HashMap::new(), 0, 0, 0, true, false));
        }
    }
    let (sequence, journal_len) = match journal_bytes {
        Some(bytes) => match journal::decode_journal(&bytes, snapshot_sequence, namespaces, |_, operation| {
            apply_operation::<{ journal::MAX_ENTRIES }>(&mut by_key, operation)
        }) {
            Ok(replay) => {
                namespace_table_changed |= !replay.namespace_table_matches;
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
            Err(FormatError::TruncatedHeader) => (snapshot_sequence, write_journal_header(root, namespaces)?),
            Err(FormatError::UnsupportedVersion | FormatError::Invalid | FormatError::TooLarge) => {
                return Ok((HashMap::new(), 0, 0, 0, true, false));
            }
        },
        None => (snapshot_sequence, 0),
    };
    drop_least_recent(&mut by_key, journal::MAX_ENTRIES);
    Ok((by_key, sequence, snapshot_bytes, journal_len, false, namespace_table_changed))
}

fn apply_operation<const ENTRY_LIMIT: usize>(by_key: &mut HashMap<Key, Arc<Entry>>, operation: Operation) -> Result<(), FormatError> {
    match operation {
        Operation::Put(record) => {
            if !by_key.contains_key(&record.key) && by_key.len() > ENTRY_LIMIT {
                return Err(FormatError::Invalid);
            }
            by_key.try_reserve(1).map_err(|_| FormatError::Allocation)?;
            by_key.insert(record.key, Arc::new(Entry::from_record(record)));
        }
        Operation::Remove(key) => {
            by_key.remove(&key);
        }
        Operation::Clear => by_key.clear(),
        Operation::Ignore => {}
    }
    Ok(())
}

fn drop_least_recent(records: &mut HashMap<Key, Arc<Entry>>, limit: usize) {
    while records.len() > limit {
        let oldest = records.iter().min_by_key(|(_, entry)| entry.last_use.load(std::sync::atomic::Ordering::Relaxed)).map(|(key, _)| *key);
        let Some(oldest) = oldest else {
            return;
        };
        records.remove(&oldest);
    }
}

fn write_journal_header(root: &Path, namespaces: &NamespaceTable) -> Result<u64, StorageError> {
    let path = root.join(JOURNAL_FILE);
    let mut header = Vec::new();
    journal::encode_journal_header(&mut header, namespaces).map_err(format_error)?;
    fs::write(path, header)?;
    journal::journal_header_size(namespaces).ok_or(StorageError::TooLarge)
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::cache::{Namespace, NamespaceId, journal::Record};

    const TEST_NAMESPACES: [Namespace; 1] = [Namespace::new("tests/recovery", "bin")];

    fn namespaces() -> NamespaceTable {
        NamespaceTable::new(&TEST_NAMESPACES).expect("valid namespace table")
    }

    fn record(value: u16) -> Record {
        let mut name = [0; 16];
        name[..2].copy_from_slice(&value.to_le_bytes());
        Record { key: Key::new(NamespaceId::new(0), name), size: 1, hash: [0; 32], last_use: 0, use_count: 0 }
    }

    #[test]
    fn eviction_replay_allows_one_transient_entry() {
        let mut bytes = Vec::new();
        let namespaces = namespaces();
        journal::encode_journal_header(&mut bytes, &namespaces).expect("journal header");
        for (sequence, operation) in
            [Operation::Put(record(1)), Operation::Put(record(2)), Operation::Put(record(3)), Operation::Remove(record(1).key)]
                .into_iter()
                .enumerate()
        {
            let sequence = u64::try_from(sequence).expect("sequence") + 1;
            bytes.extend(journal::encode_operation(sequence, &operation).expect("journal operation"));
        }

        let mut by_key = HashMap::new();
        let replay = journal::decode_journal(&bytes, 0, &namespaces, |_, operation| apply_operation::<2>(&mut by_key, operation))
            .expect("replay eviction");

        assert_eq!(replay.sequence, 4);
        assert_eq!(by_key.len(), 2);
        assert!(by_key.contains_key(&record(2).key));
        assert!(by_key.contains_key(&record(3).key));
        std::assert_matches!(apply_operation::<2>(&mut by_key, Operation::Put(record(4))), Ok(()));
        std::assert_matches!(apply_operation::<2>(&mut by_key, Operation::Put(record(5))), Err(FormatError::Invalid));
        assert_eq!(by_key.len(), 3);
    }

    #[test]
    fn torn_eviction_drops_least_recent_entry() {
        let mut records = [record(1), record(2), record(3)]
            .into_iter()
            .map(|record| (record.key, Arc::new(Entry::from_record(record))))
            .collect::<HashMap<_, _>>();
        if let Some(record) = records.get_mut(&record(1).key) {
            record.last_use.store(5, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(record) = records.get_mut(&record(2).key) {
            record.last_use.store(9, std::sync::atomic::Ordering::Relaxed);
        }

        drop_least_recent(&mut records, 2);

        let mut kept = records.keys().copied().collect::<Vec<_>>();
        kept.sort_unstable();
        assert_eq!(kept, [record(1).key, record(2).key]);
    }
}
