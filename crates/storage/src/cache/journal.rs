use std::io::{self, Cursor, Read, Write};

use sha2::{Digest, Sha256};

use super::{Key, MAX_EXTENSION_BYTES, MAX_NAMESPACE_BYTES, MAX_VALUE_BYTES, NamespaceId, NamespaceTable};

pub(super) const SNAPSHOT_BASE_HEADER_BYTES: u64 = 26;
pub(super) const JOURNAL_BASE_HEADER_BYTES: u64 = 14;
pub(super) const SNAPSHOT_CHECKSUM_BYTES: usize = 32;
pub(super) const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MAX_JOURNAL_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MAX_ENTRIES: usize = 262_144;
const SNAPSHOT_MAGIC: &[u8; 8] = b"RADSTOR2";
const JOURNAL_MAGIC: &[u8; 8] = b"RADJNL01";
const VERSION: u32 = 4;
const MAX_RECORD_BYTES: usize = 256;

#[derive(Clone, Copy, Debug)]
pub(super) struct Record {
    pub key: Key,
    pub size: u64,
    pub hash: [u8; 32],
    pub last_use: u64,
    pub use_count: u64,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Operation {
    Put(Record),
    Remove(Key),
    Clear,
    Ignore,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum FormatError {
    Invalid,
    UnsupportedVersion,
    TooLarge,
    Allocation,
}

pub(super) struct Snapshot {
    pub sequence: u64,
    pub records: Vec<Record>,
}

pub(super) struct Replay {
    pub valid_bytes: usize,
    pub sequence: u64,
}

pub(super) fn snapshot_header_size(namespaces: &NamespaceTable) -> Option<u64> {
    SNAPSHOT_BASE_HEADER_BYTES.checked_add(namespace_table_size(namespaces)?)
}

pub(super) fn journal_header_size(namespaces: &NamespaceTable) -> Option<u64> {
    JOURNAL_BASE_HEADER_BYTES.checked_add(namespace_table_size(namespaces)?)
}

pub(super) fn entry_size(_: &Record) -> Option<u64> {
    Some(73)
}

pub(super) fn encode_snapshot(sequence: u64, records: &[Record], namespaces: &NamespaceTable) -> Result<Vec<u8>, FormatError> {
    if records.len() > MAX_ENTRIES {
        return Err(FormatError::TooLarge);
    }
    let header_size = snapshot_header_size(namespaces).ok_or(FormatError::TooLarge)?;
    let record_bytes = u64::try_from(records.len()).map_err(|_| FormatError::TooLarge)?.checked_mul(73).ok_or(FormatError::TooLarge)?;
    let size = header_size.checked_add(record_bytes).ok_or(FormatError::TooLarge)?;
    let size = size.checked_add(u64::try_from(SNAPSHOT_CHECKSUM_BYTES).map_err(|_| FormatError::TooLarge)?).ok_or(FormatError::TooLarge)?;
    let capacity = usize::try_from(size).map_err(|_| FormatError::TooLarge)?;
    if capacity > MAX_SNAPSHOT_BYTES {
        return Err(FormatError::TooLarge);
    }
    let mut output = Vec::new();
    output.try_reserve_exact(capacity).map_err(|_| FormatError::Allocation)?;
    output.write_all(SNAPSHOT_MAGIC).map_err(|_| FormatError::Invalid)?;
    write_u32(&mut output, VERSION).map_err(|_| FormatError::Invalid)?;
    write_u64(&mut output, sequence).map_err(|_| FormatError::Invalid)?;
    write_u32(&mut output, u32::try_from(records.len()).map_err(|_| FormatError::TooLarge)?).map_err(|_| FormatError::Invalid)?;
    write_namespace_table(&mut output, namespaces).map_err(|_| FormatError::Invalid)?;
    for record in records {
        if usize::try_from(record.size).map_or(true, |size| size > MAX_VALUE_BYTES) {
            return Err(FormatError::Invalid);
        }
        write_key(&mut output, record.key).map_err(|_| FormatError::Invalid)?;
        write_u64(&mut output, record.size).map_err(|_| FormatError::Invalid)?;
        output.write_all(&record.hash).map_err(|_| FormatError::Invalid)?;
        write_u64(&mut output, record.last_use).map_err(|_| FormatError::Invalid)?;
        write_u64(&mut output, record.use_count).map_err(|_| FormatError::Invalid)?;
    }
    let checksum: [u8; 32] = Sha256::digest(&output).into();
    output.write_all(&checksum).map_err(|_| FormatError::Invalid)?;
    Ok(output)
}

pub(super) fn decode_snapshot(bytes: &[u8], namespaces: &NamespaceTable) -> Result<Snapshot, FormatError> {
    if bytes.len() > MAX_SNAPSHOT_BYTES
        || bytes.len() < usize::try_from(SNAPSHOT_BASE_HEADER_BYTES).map_err(|_| FormatError::TooLarge)? + SNAPSHOT_CHECKSUM_BYTES
    {
        return Err(FormatError::Invalid);
    }
    let content_end = bytes.len().checked_sub(SNAPSHOT_CHECKSUM_BYTES).ok_or(FormatError::Invalid)?;
    let content = bytes.get(..content_end).ok_or(FormatError::Invalid)?;
    let checksum = bytes.get(content_end..).ok_or(FormatError::Invalid)?;
    if Sha256::digest(content).as_slice() != checksum {
        return Err(FormatError::Invalid);
    }
    let mut input = Cursor::new(content);
    let mut magic = [0; 8];
    input.read_exact(&mut magic).map_err(|_| FormatError::Invalid)?;
    if &magic != SNAPSHOT_MAGIC {
        return Err(FormatError::Invalid);
    }
    if read_u32(&mut input).map_err(|_| FormatError::Invalid)? != VERSION {
        return Err(FormatError::UnsupportedVersion);
    }
    let sequence = read_u64(&mut input).map_err(|_| FormatError::Invalid)?;
    let count = usize::try_from(read_u32(&mut input).map_err(|_| FormatError::Invalid)?).map_err(|_| FormatError::TooLarge)?;
    if count > MAX_ENTRIES {
        return Err(FormatError::TooLarge);
    }
    let namespace_map = read_namespace_map(&mut input, namespaces)?;
    let mut records = Vec::new();
    records.try_reserve_exact(count).map_err(|_| FormatError::Allocation)?;
    let mut keys = std::collections::HashSet::new();
    keys.try_reserve(count).map_err(|_| FormatError::Allocation)?;
    for _ in 0..count {
        let key = read_key(&mut input, &namespace_map)
            .map_err(|error| if error.kind() == io::ErrorKind::OutOfMemory { FormatError::Allocation } else { FormatError::Invalid })?;
        let size = read_u64(&mut input).map_err(|_| FormatError::Invalid)?;
        if size > u64::try_from(MAX_VALUE_BYTES).map_err(|_| FormatError::TooLarge)? {
            return Err(FormatError::Invalid);
        }
        let mut hash = [0; 32];
        input.read_exact(&mut hash).map_err(|_| FormatError::Invalid)?;
        let last_use = read_u64(&mut input).map_err(|_| FormatError::Invalid)?;
        let use_count = read_u64(&mut input).map_err(|_| FormatError::Invalid)?;
        if let Some(key) = key {
            let record = Record { key, size, hash, last_use, use_count };
            if !keys.insert(record.key) {
                return Err(FormatError::Invalid);
            }
            records.push(record);
        }
    }
    if input.position() != u64::try_from(content.len()).map_err(|_| FormatError::TooLarge)? {
        return Err(FormatError::Invalid);
    }
    Ok(Snapshot { sequence, records })
}

pub(super) fn encode_journal_header(output: &mut Vec<u8>, namespaces: &NamespaceTable) -> Result<(), FormatError> {
    let capacity = usize::try_from(journal_header_size(namespaces).ok_or(FormatError::TooLarge)?).map_err(|_| FormatError::TooLarge)?;
    output.try_reserve_exact(capacity).map_err(|_| FormatError::Allocation)?;
    output.write_all(JOURNAL_MAGIC).map_err(|_| FormatError::Invalid)?;
    write_u32(output, VERSION).map_err(|_| FormatError::Invalid)?;
    write_namespace_table(output, namespaces).map_err(|_| FormatError::Invalid)
}

pub(super) fn decode_journal(
    bytes: &[u8],
    snapshot_sequence: u64,
    namespaces: &NamespaceTable,
    mut apply: impl FnMut(u64, Operation) -> Result<(), FormatError>,
) -> Result<Replay, FormatError> {
    if bytes.len() > MAX_JOURNAL_BYTES {
        return Err(FormatError::TooLarge);
    }
    let mut input = Cursor::new(bytes);
    let mut magic = [0; 8];
    input.read_exact(&mut magic).map_err(|_| FormatError::Invalid)?;
    if &magic != JOURNAL_MAGIC {
        return Err(FormatError::Invalid);
    }
    if read_u32(&mut input).map_err(|_| FormatError::Invalid)? != VERSION {
        return Err(FormatError::UnsupportedVersion);
    }
    let namespace_map = read_namespace_map(&mut input, namespaces)?;
    let mut sequence = snapshot_sequence;
    let mut valid_bytes = usize::try_from(input.position()).map_err(|_| FormatError::TooLarge)?;
    loop {
        let start = usize::try_from(input.position()).map_err(|_| FormatError::TooLarge)?;
        if start == bytes.len() {
            break;
        }
        let Some(length_end) = start.checked_add(4) else {
            return Err(FormatError::TooLarge);
        };
        let Some(length_bytes) = bytes.get(start..length_end) else {
            break;
        };
        let length_array: [u8; 4] = length_bytes.try_into().map_err(|_| FormatError::Invalid)?;
        let body_len = usize::try_from(u32::from_le_bytes(length_array)).map_err(|_| FormatError::TooLarge)?;
        if !(9..=MAX_RECORD_BYTES).contains(&body_len) {
            break;
        }
        let body_start = length_end;
        let Some(body_end) = body_start.checked_add(body_len) else {
            return Err(FormatError::TooLarge);
        };
        let Some(record_end) = body_end.checked_add(SNAPSHOT_CHECKSUM_BYTES) else {
            return Err(FormatError::TooLarge);
        };
        let Some(body) = bytes.get(body_start..body_end) else {
            break;
        };
        let Some(checksum) = bytes.get(body_end..record_end) else {
            break;
        };
        if Sha256::digest(body).as_slice() != checksum {
            break;
        }
        let mut record_input = Cursor::new(body);
        let record_sequence = read_u64(&mut record_input).map_err(|_| FormatError::Invalid)?;
        let operation = decode_operation(&mut record_input, &namespace_map)
            .map_err(|error| if error.kind() == io::ErrorKind::OutOfMemory { FormatError::Allocation } else { FormatError::Invalid })?;
        if record_input.position() != u64::try_from(body.len()).map_err(|_| FormatError::TooLarge)? {
            break;
        }
        if record_sequence > snapshot_sequence {
            if record_sequence != sequence.checked_add(1).ok_or(FormatError::TooLarge)? {
                break;
            }
            apply(record_sequence, operation)?;
            sequence = record_sequence;
        }
        valid_bytes = record_end;
        input.set_position(u64::try_from(record_end).map_err(|_| FormatError::TooLarge)?);
    }
    Ok(Replay { valid_bytes, sequence })
}

pub(super) fn encode_operation(sequence: u64, operation: &Operation) -> Result<Vec<u8>, FormatError> {
    let mut body = Vec::new();
    body.try_reserve_exact(MAX_RECORD_BYTES).map_err(|_| FormatError::Allocation)?;
    write_u64(&mut body, sequence).map_err(|_| FormatError::Invalid)?;
    match operation {
        Operation::Put(record) => {
            body.write_all(&[1]).map_err(|_| FormatError::Invalid)?;
            write_key(&mut body, record.key).map_err(|_| FormatError::Invalid)?;
            if usize::try_from(record.size).map_or(true, |size| size > MAX_VALUE_BYTES) {
                return Err(FormatError::Invalid);
            }
            write_u64(&mut body, record.size).map_err(|_| FormatError::Invalid)?;
            body.write_all(&record.hash).map_err(|_| FormatError::Invalid)?;
        }
        Operation::Remove(key) => {
            body.write_all(&[2]).map_err(|_| FormatError::Invalid)?;
            write_key(&mut body, *key).map_err(|_| FormatError::Invalid)?;
        }
        Operation::Clear => body.write_all(&[3]).map_err(|_| FormatError::Invalid)?,
        Operation::Ignore => return Err(FormatError::Invalid),
    }
    if body.len() > MAX_RECORD_BYTES {
        return Err(FormatError::TooLarge);
    }
    let body_len = u32::try_from(body.len()).map_err(|_| FormatError::TooLarge)?;
    let mut output = Vec::new();
    output.try_reserve_exact(4 + body.len() + SNAPSHOT_CHECKSUM_BYTES).map_err(|_| FormatError::Allocation)?;
    write_u32(&mut output, body_len).map_err(|_| FormatError::Invalid)?;
    output.write_all(&body).map_err(|_| FormatError::Invalid)?;
    let checksum: [u8; 32] = Sha256::digest(&body).into();
    output.write_all(&checksum).map_err(|_| FormatError::Invalid)?;
    Ok(output)
}

fn namespace_table_size(namespaces: &NamespaceTable) -> Option<u64> {
    let mut size = 0u64;
    for index in 0..namespaces.len() {
        let id = NamespaceId::new(u8::try_from(index).ok()?);
        let config = namespaces.config(id)?;
        size = size.checked_add(2)?;
        size = size.checked_add(u64::try_from(config.directory.len()).ok()?)?;
        size = size.checked_add(u64::try_from(config.extension.len()).ok()?)?;
    }
    Some(size)
}

fn write_namespace_table(output: &mut Vec<u8>, namespaces: &NamespaceTable) -> io::Result<()> {
    output.write_all(&u16::try_from(namespaces.len()).map_err(|_| io::Error::other("namespace count is invalid"))?.to_le_bytes())?;
    for index in 0..namespaces.len() {
        let id = u8::try_from(index).map_err(|_| io::Error::other("namespace index is invalid"))?;
        let config = namespaces.config(NamespaceId::new(id)).ok_or_else(|| io::Error::other("namespace is unavailable"))?;
        output.write_all(&[u8::try_from(config.directory.len()).map_err(|_| io::Error::other("namespace length is invalid"))?])?;
        output.write_all(config.directory.as_bytes())?;
        output.write_all(&[u8::try_from(config.extension.len()).map_err(|_| io::Error::other("extension length is invalid"))?])?;
        output.write_all(config.extension.as_bytes())?;
    }
    Ok(())
}

fn read_namespace_map(input: &mut Cursor<&[u8]>, current: &NamespaceTable) -> Result<Vec<Option<NamespaceId>>, FormatError> {
    let count = usize::from(read_u16(input).map_err(|_| FormatError::Invalid)?);
    if count > usize::from(u8::MAX) + 1 {
        return Err(FormatError::Invalid);
    }
    let mut mapping = Vec::new();
    mapping.try_reserve_exact(count).map_err(|_| FormatError::Allocation)?;
    for _ in 0..count {
        let directory_len = usize::from(read_u8(input).map_err(|_| FormatError::Invalid)?);
        if directory_len == 0 || directory_len > MAX_NAMESPACE_BYTES {
            return Err(FormatError::Invalid);
        }
        let mut directory = [0; MAX_NAMESPACE_BYTES];
        input.read_exact(directory.get_mut(..directory_len).ok_or(FormatError::Invalid)?).map_err(|_| FormatError::Invalid)?;
        let directory =
            std::str::from_utf8(directory.get(..directory_len).ok_or(FormatError::Invalid)?).map_err(|_| FormatError::Invalid)?;
        if !super::valid_namespace(directory) {
            return Err(FormatError::Invalid);
        }
        let extension_len = usize::from(read_u8(input).map_err(|_| FormatError::Invalid)?);
        if extension_len == 0 || extension_len > MAX_EXTENSION_BYTES {
            return Err(FormatError::Invalid);
        }
        let mut extension = [0; MAX_EXTENSION_BYTES];
        input.read_exact(extension.get_mut(..extension_len).ok_or(FormatError::Invalid)?).map_err(|_| FormatError::Invalid)?;
        let extension =
            std::str::from_utf8(extension.get(..extension_len).ok_or(FormatError::Invalid)?).map_err(|_| FormatError::Invalid)?;
        if !super::valid_extension(extension) {
            return Err(FormatError::Invalid);
        }
        let found = (0..current.len()).find_map(|index| {
            let id = u8::try_from(index).ok().map(NamespaceId::new)?;
            let config = current.config(id)?;
            (config.directory == directory && config.extension == extension).then_some(id)
        });
        mapping.push(found);
    }
    Ok(mapping)
}

fn write_key(output: &mut Vec<u8>, key: Key) -> io::Result<()> {
    output.write_all(&[key.namespace().0])?;
    output.write_all(&key.name())
}

fn read_key(input: &mut Cursor<&[u8]>, namespace_map: &[Option<NamespaceId>]) -> io::Result<Option<Key>> {
    let old_index = read_u8(input)?;
    let Some(namespace) = namespace_map.get(usize::from(old_index)).copied() else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "cache key namespace is invalid"));
    };
    let mut name = [0; 16];
    input.read_exact(&mut name)?;
    Ok(namespace.map(|namespace| Key::new(namespace, name)))
}

fn decode_operation(input: &mut Cursor<&[u8]>, namespace_map: &[Option<NamespaceId>]) -> io::Result<Operation> {
    let tag = read_u8(input)?;
    match tag {
        1 => {
            let key = read_key(input, namespace_map)?;
            let size = read_u64(input)?;
            if usize::try_from(size).map_or(true, |size| size > MAX_VALUE_BYTES) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "journal value is too large"));
            }
            let mut hash = [0; 32];
            input.read_exact(&mut hash)?;
            Ok(match key {
                Some(key) => Operation::Put(Record { key, size, hash, last_use: 0, use_count: 0 }),
                None => Operation::Ignore,
            })
        }
        2 => Ok(match read_key(input, namespace_map)? {
            Some(key) => Operation::Remove(key),
            None => Operation::Ignore,
        }),
        3 => Ok(Operation::Clear),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "journal operation is invalid")),
    }
}

fn read_u8(input: &mut Cursor<&[u8]>) -> io::Result<u8> {
    let mut value = [0; 1];
    input.read_exact(&mut value)?;
    value.first().copied().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cache key is truncated"))
}

fn write_u32(output: &mut Vec<u8>, value: u32) -> io::Result<()> {
    output.write_all(&value.to_le_bytes())
}

fn write_u64(output: &mut Vec<u8>, value: u64) -> io::Result<()> {
    output.write_all(&value.to_le_bytes())
}

fn read_u16(input: &mut Cursor<&[u8]>) -> io::Result<u16> {
    let mut bytes = [0; 2];
    input.read_exact(&mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(input: &mut Cursor<&[u8]>) -> io::Result<u32> {
    let mut bytes = [0; 4];
    input.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(input: &mut Cursor<&[u8]>) -> io::Result<u64> {
    let mut bytes = [0; 8];
    input.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}
