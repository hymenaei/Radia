use serde::de::DeserializeOwned;

use crate::{Limits, Result, binary, encoding::strip_header, notation, xml};

pub fn from_slice_with_limits<T: DeserializeOwned>(input: &[u8], limits: Limits) -> Result<T> {
    let input = input.trim_ascii_start();
    if strip_header(input, b"llsd/binary").is_some() {
        binary::from_slice_with_limits(input, limits)
    } else if strip_header(input, b"llsd/notation").is_some() {
        notation::from_slice_with_limits(input, limits)
    } else if let Some(body) = strip_header(input, b"llsd/xml") {
        xml::from_slice_with_limits(body.trim_ascii_start(), limits)
    } else if input.starts_with(b"<") {
        xml::from_slice_with_limits(input, limits)
    } else {
        notation::from_slice_with_limits(input, limits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;

    fn decode(input: &[u8]) -> Value {
        from_slice_with_limits(input, Limits::default()).expect("decode LLSD")
    }

    #[test]
    fn detects_official_headers_case_insensitively() {
        let expected = decode(b"{'type':'sky'}");
        for input in [
            &b"<? llsd/notation ?>\n{'type':'sky'}"[..],
            b"<?llsd/notation?>\n{'type':'sky'}",
            b"<? LLSD/Notation ?>\r\n{'type':'sky'}",
            b"<? LLSD/XML ?>\n<llsd><map><key>type</key><string>sky</string></map></llsd>",
            b"<?xml version=\"1.0\"?><llsd><map><key>type</key><string>sky</string></map></llsd>",
            b"  {'type':'sky'}",
        ] {
            assert_eq!(decode(input), expected, "{}", String::from_utf8_lossy(input));
        }
    }

    #[test]
    fn detects_binary_header() {
        let mut input = b"<? LLSD/Binary ?>\n{".to_vec();
        input.extend(1u32.to_be_bytes());
        input.push(b'k');
        input.extend(4u32.to_be_bytes());
        input.extend(b"type");
        input.push(b's');
        input.extend(3u32.to_be_bytes());
        input.extend(b"sky}");
        assert_eq!(decode(&input), decode(b"{'type':'sky'}"));
    }
}
