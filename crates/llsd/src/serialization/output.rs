use base64::{Engine, engine::general_purpose::STANDARD};

use crate::{Error, Result};

pub(crate) struct Output {
    pub(crate) bytes: Vec<u8>,
    max_bytes: Option<usize>,
}

impl Output {
    pub(super) fn new(max_bytes: Option<usize>) -> Self {
        Self { bytes: Vec::new(), max_bytes }
    }

    pub(crate) fn remaining(&self) -> Option<usize> {
        self.max_bytes.map(|max| max.saturating_sub(self.bytes.len()))
    }

    pub(crate) fn check_space(&self, length: usize) -> Result<()> {
        if self.remaining().is_some_and(|remaining| length > remaining) {
            return Err(Error::LimitExceeded("encoded length"));
        }
        Ok(())
    }

    pub(crate) fn append(&mut self, bytes: &[u8]) -> Result<()> {
        self.check_space(bytes.len())?;
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    pub(crate) fn base64(&mut self, bytes: &[u8], prefix: &[u8], suffix: &[u8]) -> Result<()> {
        let encoded_len = bytes
            .len()
            .checked_add(2)
            .and_then(|n| n.checked_div(3))
            .and_then(|n| n.checked_mul(4))
            .ok_or(Error::LimitExceeded("encoded length"))?;
        let length = prefix
            .len()
            .checked_add(encoded_len)
            .and_then(|n| n.checked_add(suffix.len()))
            .ok_or(Error::LimitExceeded("encoded length"))?;
        self.check_space(length)?;
        self.bytes.reserve(length);
        self.bytes.extend_from_slice(prefix);
        let start = self.bytes.len();
        self.bytes.resize(start + encoded_len, 0);
        STANDARD.encode_slice(bytes, &mut self.bytes[start..]).map_err(|e| Error::InvalidValue(e.to_string()))?;
        self.bytes.extend_from_slice(suffix);
        Ok(())
    }
}
