use std::{
    collections::HashSet,
    io::{self, Cursor, Read, Write},
};

use crate::cache::Key;

pub(crate) const INDEX_HEADER_BYTES: u64 = 16;
pub(crate) const MAX_INDEX_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_ENTRIES: usize = 16_384;
const MAGIC: &[u8; 8] = b"RADSTOR1";
const VERSION: u32 = 1;
const MAX_FILE_NAME_BYTES: usize = 128;
const RECORD_FIXED_BYTES: u64 = 90;

#[derive(Debug)]
pub(crate) enum FormatError {
    Invalid,
    UnsupportedVersion,
    TooLarge,
}

#[derive(Clone, Debug)]
pub(crate) struct Record {
    pub key: Key,
    pub file_name: String,
    pub size: u64,
    pub hash: [u8; 32],
    pub last_use: u64,
    pub use_count: u64,
}

pub(crate) fn entry_size(file_name: &str) -> Option<u64> {
    RECORD_FIXED_BYTES.checked_add(u64::try_from(file_name.len()).ok()?)
}

pub(crate) fn encoded_size(records: &[Record]) -> Option<u64> {
    let mut size = INDEX_HEADER_BYTES;
    for record in records {
        size = size.checked_add(entry_size(&record.file_name)?)?;
    }
    Some(size)
}

pub(crate) fn encode(records: &[Record]) -> Result<Vec<u8>, FormatError> {
    if records.len() > MAX_ENTRIES {
        return Err(FormatError::TooLarge);
    }
    let size = encoded_size(records).ok_or(FormatError::TooLarge)?;
    if size > MAX_INDEX_BYTES {
        return Err(FormatError::TooLarge);
    }
    let capacity = usize::try_from(size).map_err(|_| FormatError::TooLarge)?;
    let mut output = Vec::new();
    output.try_reserve_exact(capacity).map_err(|_| FormatError::TooLarge)?;
    output.write_all(MAGIC).map_err(|_| FormatError::Invalid)?;
    write_u32(&mut output, VERSION).map_err(|_| FormatError::Invalid)?;
    write_u32(&mut output, u32::try_from(records.len()).map_err(|_| FormatError::TooLarge)?).map_err(|_| FormatError::Invalid)?;
    for record in records {
        if !valid_file_name(&record.file_name) {
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
    Ok(output)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Vec<Record>, FormatError> {
    if u64::try_from(bytes.len()).map_err(|_| FormatError::TooLarge)? > MAX_INDEX_BYTES {
        return Err(FormatError::TooLarge);
    }
    let mut input = Cursor::new(bytes);
    let mut magic = [0; 8];
    input.read_exact(&mut magic).map_err(|_| FormatError::Invalid)?;
    if &magic != MAGIC {
        return Err(FormatError::Invalid);
    }
    let version = read_u32(&mut input).map_err(|_| FormatError::Invalid)?;
    if version != VERSION {
        return Err(FormatError::UnsupportedVersion);
    }
    let count = usize::try_from(read_u32(&mut input).map_err(|_| FormatError::Invalid)?).map_err(|_| FormatError::TooLarge)?;
    if count > MAX_ENTRIES {
        return Err(FormatError::TooLarge);
    }
    let mut records = Vec::new();
    records.try_reserve_exact(count).map_err(|_| FormatError::TooLarge)?;
    let mut keys = HashSet::with_capacity(count);
    let mut names = HashSet::with_capacity(count);
    for _ in 0..count {
        let mut key_bytes = [0; 32];
        input.read_exact(&mut key_bytes).map_err(|_| FormatError::Invalid)?;
        let name_len = usize::from(read_u16(&mut input).map_err(|_| FormatError::Invalid)?);
        if name_len > MAX_FILE_NAME_BYTES {
            return Err(FormatError::Invalid);
        }
        let mut name_bytes = Vec::new();
        name_bytes.try_reserve_exact(name_len).map_err(|_| FormatError::TooLarge)?;
        name_bytes.resize(name_len, 0);
        input.read_exact(&mut name_bytes).map_err(|_| FormatError::Invalid)?;
        let file_name = String::from_utf8(name_bytes).map_err(|_| FormatError::Invalid)?;
        if !valid_file_name(&file_name) {
            return Err(FormatError::Invalid);
        }
        let size = read_u64(&mut input).map_err(|_| FormatError::Invalid)?;
        let mut hash = [0; 32];
        input.read_exact(&mut hash).map_err(|_| FormatError::Invalid)?;
        let last_use = read_u64(&mut input).map_err(|_| FormatError::Invalid)?;
        let use_count = read_u64(&mut input).map_err(|_| FormatError::Invalid)?;
        let record = Record { key: Key::new(key_bytes), file_name, size, hash, last_use, use_count };
        if !keys.insert(record.key) || !names.insert(record.file_name.clone()) || size > super::cache::MAX_VALUE_BYTES as u64 {
            return Err(FormatError::Invalid);
        }
        records.push(record);
    }
    if input.position() != u64::try_from(bytes.len()).map_err(|_| FormatError::TooLarge)? {
        return Err(FormatError::Invalid);
    }
    Ok(records)
}

pub(crate) fn valid_file_name(file_name: &str) -> bool {
    file_name.starts_with("data-")
        && file_name.ends_with(".bin")
        && file_name.len() <= MAX_FILE_NAME_BYTES
        && file_name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' || byte == b'.')
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
