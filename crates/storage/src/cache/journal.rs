use std::io::{self, Cursor, Read, Write};

use sha2::{Digest, Sha256};

use super::{Key, MAX_VALUE_BYTES};

pub(super) const SNAPSHOT_HEADER_BYTES: u64 = 24;
pub(super) const SNAPSHOT_CHECKSUM_BYTES: usize = 32;
pub(super) const JOURNAL_HEADER_BYTES: u64 = 12;
pub(super) const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MAX_JOURNAL_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MAX_ENTRIES: usize = 262_144;
const SNAPSHOT_MAGIC: &[u8; 8] = b"RADSTOR2";
const JOURNAL_MAGIC: &[u8; 8] = b"RADJNL01";
const VERSION: u32 = 2;
const MAX_FILE_NAME_BYTES: usize = 128;
const MAX_RECORD_BYTES: usize = 256;

#[derive(Clone, Debug)]
pub(super) struct Record {
    pub key: Key,
    pub file_name: String,
    pub size: u64,
    pub hash: [u8; 32],
    pub last_use: u64,
    pub use_count: u64,
}

#[derive(Clone, Debug)]
pub(super) enum Operation {
    Put(Record),
    Remove(Key),
    Clear,
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

pub(super) fn entry_size(record: &Record) -> Option<u64> {
    90u64.checked_add(u64::try_from(record.file_name.len()).ok()?)
}

pub(super) fn encode_snapshot(sequence: u64, records: &[Record]) -> Result<Vec<u8>, FormatError> {
    if records.len() > MAX_ENTRIES {
        return Err(FormatError::TooLarge);
    }
    let record_bytes = records.iter().try_fold(0u64, |size, record| size.checked_add(entry_size(record)?)).ok_or(FormatError::TooLarge)?;
    let size = SNAPSHOT_HEADER_BYTES.checked_add(record_bytes).ok_or(FormatError::TooLarge)?;
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
    for record in records {
        if !valid_file_name(&record.file_name) || record.size > u64::try_from(MAX_VALUE_BYTES).map_err(|_| FormatError::TooLarge)? {
            return Err(FormatError::Invalid);
        }
        output.write_all(record.key.as_bytes()).map_err(|_| FormatError::Invalid)?;
        write_u16(&mut output, u16::try_from(record.file_name.len()).map_err(|_| FormatError::TooLarge)?)
            .map_err(|_| FormatError::Invalid)?;
        output.write_all(record.file_name.as_bytes()).map_err(|_| FormatError::Invalid)?;
        write_u64(&mut output, record.size).map_err(|_| FormatError::Invalid)?;
        output.write_all(&record.hash).map_err(|_| FormatError::Invalid)?;
        write_u64(&mut output, record.last_use).map_err(|_| FormatError::Invalid)?;
        write_u64(&mut output, record.use_count).map_err(|_| FormatError::Invalid)?;
    }
    let checksum: [u8; 32] = Sha256::digest(&output).into();
    output.write_all(&checksum).map_err(|_| FormatError::Invalid)?;
    Ok(output)
}

pub(super) fn decode_snapshot(bytes: &[u8]) -> Result<Snapshot, FormatError> {
    if bytes.len() > MAX_SNAPSHOT_BYTES
        || bytes.len() < usize::try_from(SNAPSHOT_HEADER_BYTES).map_err(|_| FormatError::TooLarge)? + SNAPSHOT_CHECKSUM_BYTES
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
    let mut records = Vec::new();
    records.try_reserve_exact(count).map_err(|_| FormatError::Allocation)?;
    let mut keys = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    keys.try_reserve(count).map_err(|_| FormatError::Allocation)?;
    names.try_reserve(count).map_err(|_| FormatError::Allocation)?;
    for _ in 0..count {
        let mut key_bytes = [0; 32];
        input.read_exact(&mut key_bytes).map_err(|_| FormatError::Invalid)?;
        let name_len = usize::from(read_u16(&mut input).map_err(|_| FormatError::Invalid)?);
        if name_len > MAX_FILE_NAME_BYTES {
            return Err(FormatError::Invalid);
        }
        let mut name_bytes = Vec::new();
        name_bytes.try_reserve_exact(name_len).map_err(|_| FormatError::Allocation)?;
        name_bytes.resize(name_len, 0);
        input.read_exact(&mut name_bytes).map_err(|_| FormatError::Invalid)?;
        let file_name = String::from_utf8(name_bytes).map_err(|_| FormatError::Invalid)?;
        if !valid_file_name(&file_name) {
            return Err(FormatError::Invalid);
        }
        let size = read_u64(&mut input).map_err(|_| FormatError::Invalid)?;
        if size > u64::try_from(MAX_VALUE_BYTES).map_err(|_| FormatError::TooLarge)? {
            return Err(FormatError::Invalid);
        }
        let mut hash = [0; 32];
        input.read_exact(&mut hash).map_err(|_| FormatError::Invalid)?;
        let last_use = read_u64(&mut input).map_err(|_| FormatError::Invalid)?;
        let use_count = read_u64(&mut input).map_err(|_| FormatError::Invalid)?;
        let record = Record { key: Key::new(key_bytes), file_name, size, hash, last_use, use_count };
        if !keys.insert(record.key) || !names.insert(record.file_name.clone()) {
            return Err(FormatError::Invalid);
        }
        records.push(record);
    }
    if input.position() != u64::try_from(content.len()).map_err(|_| FormatError::TooLarge)? {
        return Err(FormatError::Invalid);
    }
    Ok(Snapshot { sequence, records })
}

pub(super) fn encode_journal_header(output: &mut Vec<u8>) -> Result<(), FormatError> {
    output
        .try_reserve_exact(usize::try_from(JOURNAL_HEADER_BYTES).map_err(|_| FormatError::TooLarge)?)
        .map_err(|_| FormatError::Allocation)?;
    output.write_all(JOURNAL_MAGIC).map_err(|_| FormatError::Invalid)?;
    write_u32(output, VERSION).map_err(|_| FormatError::Invalid)
}

pub(super) fn decode_journal(
    bytes: &[u8],
    snapshot_sequence: u64,
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
    let mut sequence = snapshot_sequence;
    let mut valid_bytes = usize::try_from(JOURNAL_HEADER_BYTES).map_err(|_| FormatError::TooLarge)?;
    loop {
        let start = usize::try_from(input.position()).map_err(|_| FormatError::TooLarge)?;
        if start == bytes.len() {
            break;
        }
        let Some(length_bytes) = bytes.get(start..start.saturating_add(4)) else {
            break;
        };
        let length_array: [u8; 4] = length_bytes.try_into().map_err(|_| FormatError::Invalid)?;
        let body_len = usize::try_from(u32::from_le_bytes(length_array)).map_err(|_| FormatError::TooLarge)?;
        if !(9..=MAX_RECORD_BYTES).contains(&body_len) {
            break;
        }
        let body_start = start.checked_add(4).ok_or(FormatError::TooLarge)?;
        let body_end = body_start.checked_add(body_len).ok_or(FormatError::TooLarge)?;
        let record_end = body_end.checked_add(SNAPSHOT_CHECKSUM_BYTES).ok_or(FormatError::TooLarge)?;
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
        let operation = decode_operation(&mut record_input)
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
            body.write_all(record.key.as_bytes()).map_err(|_| FormatError::Invalid)?;
            if !valid_file_name(&record.file_name) {
                return Err(FormatError::Invalid);
            }
            write_u16(&mut body, u16::try_from(record.file_name.len()).map_err(|_| FormatError::TooLarge)?)
                .map_err(|_| FormatError::Invalid)?;
            body.write_all(record.file_name.as_bytes()).map_err(|_| FormatError::Invalid)?;
            write_u64(&mut body, record.size).map_err(|_| FormatError::Invalid)?;
            body.write_all(&record.hash).map_err(|_| FormatError::Invalid)?;
        }
        Operation::Remove(key) => {
            body.write_all(&[2]).map_err(|_| FormatError::Invalid)?;
            body.write_all(key.as_bytes()).map_err(|_| FormatError::Invalid)?;
        }
        Operation::Clear => body.write_all(&[3]).map_err(|_| FormatError::Invalid)?,
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

pub(super) fn valid_file_name(file_name: &str) -> bool {
    file_name.starts_with("data-")
        && file_name.ends_with(".bin")
        && file_name.len() <= MAX_FILE_NAME_BYTES
        && file_name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' || byte == b'.')
}

fn decode_operation(input: &mut Cursor<&[u8]>) -> io::Result<Operation> {
    let mut tag = [0; 1];
    input.read_exact(&mut tag)?;
    match tag.first().copied() {
        Some(1) => {
            let mut key_bytes = [0; 32];
            input.read_exact(&mut key_bytes)?;
            let name_len = usize::from(read_u16(input)?);
            if name_len > MAX_FILE_NAME_BYTES {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "journal file name is too long"));
            }
            let mut name_bytes = Vec::new();
            name_bytes.try_reserve_exact(name_len).map_err(|_| io::Error::new(io::ErrorKind::OutOfMemory, "journal allocation failed"))?;
            name_bytes.resize(name_len, 0);
            input.read_exact(&mut name_bytes)?;
            let file_name =
                String::from_utf8(name_bytes).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "journal file name is invalid"))?;
            if !valid_file_name(&file_name) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "journal file name is invalid"));
            }
            let size = read_u64(input)?;
            if size > u64::try_from(MAX_VALUE_BYTES).unwrap_or(u64::MAX) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "journal value is too large"));
            }
            let mut hash = [0; 32];
            input.read_exact(&mut hash)?;
            Ok(Operation::Put(Record { key: Key::new(key_bytes), file_name, size, hash, last_use: 0, use_count: 0 }))
        }
        Some(2) => {
            let mut key_bytes = [0; 32];
            input.read_exact(&mut key_bytes)?;
            Ok(Operation::Remove(Key::new(key_bytes)))
        }
        Some(3) => Ok(Operation::Clear),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "journal operation is invalid")),
    }
}

fn write_u16(output: &mut Vec<u8>, value: u16) -> io::Result<()> {
    output.write_all(&value.to_le_bytes())
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
