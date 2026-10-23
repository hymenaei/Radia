use std::io::Read;

use crate::{Error, Result};

pub(crate) fn base64_decoded_len(input: &[u8]) -> Result<usize> {
    if !input.len().is_multiple_of(4) {
        return Err(Error::InvalidValue("invalid LLSD base64 length".into()));
    }
    let padding = input.iter().rev().take_while(|&&byte| byte == b'=').count();
    if padding > 2 {
        return Err(Error::InvalidValue("invalid LLSD base64 padding".into()));
    }
    input
        .len()
        .checked_div(4)
        .and_then(|groups| groups.checked_mul(3))
        .and_then(|length| length.checked_sub(padding))
        .ok_or_else(|| Error::InvalidValue("invalid LLSD base64 length".into()))
}

pub(crate) fn strip_header<'a>(input: &'a [u8], marker: &[u8]) -> Option<&'a [u8]> {
    if input.get(..2)? != b"<?" {
        return None;
    }

    let mut offset = 2;
    while input.get(offset).is_some_and(u8::is_ascii_whitespace) {
        offset += 1;
    }
    let marker_end = offset.checked_add(marker.len())?;
    if !input.get(offset..marker_end)?.eq_ignore_ascii_case(marker) {
        return None;
    }
    offset = marker_end;
    while input.get(offset).is_some_and(u8::is_ascii_whitespace) {
        offset += 1;
    }
    if input.get(offset..offset.checked_add(2)?)? != b"?>" {
        return None;
    }
    offset += 2;
    if input.get(offset..).is_some_and(|rest| rest.starts_with(b"\r\n")) {
        offset += 2;
    } else if input.get(offset).is_some_and(|byte| *byte == b'\n' || *byte == b'\r') {
        offset += 1;
    }
    Some(&input[offset..])
}

pub(crate) fn parse_real(value: &str) -> Result<f64> {
    if value.eq_ignore_ascii_case("nan") {
        Ok(f64::NAN)
    } else if ["inf", "+inf", "infinity", "+infinity"].iter().any(|text| value.eq_ignore_ascii_case(text)) {
        Ok(f64::INFINITY)
    } else if ["-inf", "-infinity"].iter().any(|text| value.eq_ignore_ascii_case(text)) {
        Ok(f64::NEG_INFINITY)
    } else {
        value.parse().map_err(|_| Error::InvalidValue("invalid LLSD real".into()))
    }
}

pub(crate) fn real_text(value: f64) -> String {
    if value.is_nan() {
        "nan".into()
    } else if value == f64::INFINITY {
        "inf".into()
    } else if value == f64::NEG_INFINITY {
        "-inf".into()
    } else {
        value.to_string()
    }
}

pub(crate) fn read_limited<R: Read>(reader: &mut R, max_bytes: usize) -> Result<Vec<u8>> {
    let max = max_bytes.saturating_add(1);
    let mut bytes = Vec::with_capacity(max.min(8192));
    reader.take(max as u64).read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(Error::LimitExceeded("input bytes"));
    }
    Ok(bytes)
}
