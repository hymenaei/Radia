//! Allocation admission estimates for the built-in XML value decoder.

use std::{
    error, fmt,
    mem::{align_of, size_of},
};

use crate::{Error, Limits, Value};

use super::{Token, Tokens};

/// XML cannot be scanned within its limits or represented by an allocation estimate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BoundError {
    /// Invalid XML framing, LLSD structure, or binary length.
    Malformed,
    /// A configured decoder limit was exceeded.
    LimitExceeded(&'static str),
    /// Allocation arithmetic exceeds the target's `usize` range.
    Overflow,
}

impl fmt::Display for BoundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => f.write_str("malformed LLSD XML allocation input"),
            Self::LimitExceeded(limit) => write!(f, "LLSD {limit} limit exceeded"),
            Self::Overflow => f.write_str("LLSD allocation estimate overflowed"),
        }
    }
}

impl error::Error for BoundError {}

impl From<Error> for BoundError {
    fn from(error: Error) -> Self {
        match error {
            Error::LimitExceeded(limit) => Self::LimitExceeded(limit),
            _ => Self::Malformed,
        }
    }
}

/// Estimates peak additional live allocations of the current built-in [`Value`] decoder.
///
/// The HTTP/input buffer is excluded and must remain separately charged. Scan storage is
/// O(`limits.max_depth`); scalar text is counted without collecting it. The scanner shares
/// the XML tokenizer and limits, but does not replace semantic decoding: scalar values and
/// binary attributes can still be rejected by the decoder after a successful scan.
///
/// The estimate includes collection capacity, strings and map keys, overlapping growth and
/// base64 buffers, parser workspace, and a per-allocation metadata/alignment allowance.
/// Its collection model follows the current Rust `Vec` growth and `BTreeMap` node layout.
/// This is an admission estimate for this decoder, not a guaranteed process heap ceiling:
/// allocator bookkeeping, heap fragmentation, and unrelated allocations are not observable
/// from XML. It does not bound arbitrary custom Serde output or user deserializer work.
///
/// Arithmetic uses the current target's `usize`, including on 32-bit targets. An estimate
/// that cannot be represented returns [`BoundError::Overflow`] rather than wrapping.
///
/// # Errors
///
/// Returns [`BoundError::Malformed`] for invalid framing or structure,
/// [`BoundError::LimitExceeded`] for configured limits, and [`BoundError::Overflow`]
/// for allocation arithmetic overflow. Callers must secure admission before decoding.
pub fn value_allocation_bound(input: &[u8], limits: Limits) -> Result<usize, BoundError> {
    if input.len() > limits.max_input_bytes {
        return Err(BoundError::LimitExceeded("input bytes"));
    }
    let mut scan = Scanner { tokens: Tokens::for_bound(input, limits)?, limits, values: 0, bytes: 0 };
    scan.skip_space()?;
    if !matches!(scan.tokens.take()?, Some(Token::Start("llsd"))) {
        return Err(BoundError::Malformed);
    }
    let mut frames: Vec<Frame> = Vec::new();
    let mut depth = 0;
    loop {
        scan.skip_space()?;
        if let Some(frame) = frames.last_mut() {
            if matches!(scan.tokens.peek()?, Some(Token::End(tag)) if *tag == frame.tag) {
                if frame.needs_value {
                    return Err(BoundError::Malformed);
                }
                scan.tokens.take()?;
                let frame = frames.pop().ok_or(BoundError::Malformed)?;
                scan.collection(frame.tag, frame.items)?;
                if frames.is_empty() {
                    break;
                }
                continue;
            }
            if frame.tag == "map" && !frame.needs_value {
                let (tag, empty) = scan.tag()?;
                if tag != "key" {
                    return Err(BoundError::Malformed);
                }
                let key = scan.scalar(tag, empty)?;
                if key.bytes > limits.max_map_key_bytes {
                    return Err(BoundError::LimitExceeded("map key bytes"));
                }
                scan.text_allocation(key.bytes)?;
                frame.needs_value = true;
                continue;
            }
            frame.items = add(frame.items, 1)?;
            if frame.items > limits.max_container_items {
                return Err(BoundError::LimitExceeded("container items"));
            }
            frame.needs_value = false;
        }
        scan.values = add(scan.values, 1)?;
        if scan.values > limits.max_values {
            return Err(BoundError::LimitExceeded("value count"));
        }
        scan.charge(allocation(9, 1)?)?;
        let (tag, empty) = scan.tag()?;
        let attributes = scan.tokens.binary_attribute_bytes;
        if attributes != 0 {
            scan.charge(add(mul(attributes, 12)?, allocation(64, 2)?)?)?;
        }
        match tag {
            "array" | "map" if !empty => {
                let next_depth = add(frames.len(), 1)?;
                if next_depth > limits.max_depth {
                    return Err(BoundError::LimitExceeded("nesting depth"));
                }
                frames.push(Frame { tag, items: 0, needs_value: false });
                depth = depth.max(next_depth);
                continue;
            }
            "array" | "map" => {}
            "undef" | "boolean" | "integer" | "real" | "uuid" | "string" | "uri" | "date" | "binary" => {
                let scalar = scan.scalar(tag, empty)?;
                let text_bytes = match (tag, scalar.bytes) {
                    ("uuid", 0) => 36,
                    ("date", 0) => 20,
                    _ => scalar.bytes,
                };
                scan.text_allocation(text_bytes)?;
                if tag == "binary" {
                    if scalar.compact % 4 != 0 || scalar.padding > 2 {
                        return Err(BoundError::Malformed);
                    }
                    let decoded = mul(scalar.compact / 4, 3)?;
                    let actual = decoded.checked_sub(scalar.padding).ok_or(BoundError::Malformed)?;
                    if actual > limits.max_binary_bytes {
                        return Err(BoundError::LimitExceeded("binary bytes"));
                    }
                    scan.charge(allocation(scalar.bytes, 1)?)?;
                    scan.charge(allocation(mul(decoded, 2)?, 2)?)?;
                }
            }
            _ => return Err(BoundError::Malformed),
        }
        if frames.is_empty() {
            break;
        }
    }
    scan.skip_space()?;
    if !matches!(scan.tokens.take()?, Some(Token::End("llsd"))) {
        return Err(BoundError::Malformed);
    }
    scan.skip_space()?;
    if scan.tokens.peek()?.is_some() {
        return Err(BoundError::Malformed);
    }
    let per_depth = mul(add(add(size_of::<usize>(), 7)?, size_of::<(bool, bool)>())?, 4)?;
    let levels = add(depth, 2)?.max(4);
    scan.charge(allocation(mul(levels, per_depth)?, 6)?)?;
    Ok(scan.bytes)
}

struct Frame {
    tag: &'static str,
    items: usize,
    needs_value: bool,
}

#[derive(Default)]
struct Scalar {
    bytes: usize,
    compact: usize,
    padding: usize,
}

struct Scanner<'input> {
    tokens: Tokens<'input>,
    limits: Limits,
    values: usize,
    bytes: usize,
}

impl Scanner<'_> {
    fn skip_space(&mut self) -> Result<(), BoundError> {
        while matches!(
            self.tokens.peek()?,
            Some(Token::Text(text) | Token::CharacterReference(text)) if text.trim().is_empty()
        ) {
            self.tokens.take()?;
        }
        Ok(())
    }

    fn tag(&mut self) -> Result<(&'static str, bool), BoundError> {
        self.skip_space()?;
        match self.tokens.take()? {
            Some(Token::Start(tag)) => Ok((tag, false)),
            Some(Token::Empty(tag)) => Ok((tag, true)),
            _ => Err(BoundError::Malformed),
        }
    }

    fn scalar(&mut self, tag: &str, empty: bool) -> Result<Scalar, BoundError> {
        let mut scalar = Scalar::default();
        if empty {
            return Ok(scalar);
        }
        loop {
            match self.tokens.take()? {
                Some(Token::End(end)) if end == tag => return Ok(scalar),
                Some(Token::Text(text) | Token::CharacterReference(text)) => {
                    scalar.bytes = add(scalar.bytes, text.len())?;
                    let (limit, name) = if tag == "binary" {
                        (self.limits.max_input_bytes, "input bytes")
                    } else {
                        (self.limits.max_string_bytes, "string bytes")
                    };
                    if scalar.bytes > limit {
                        return Err(BoundError::LimitExceeded(name));
                    }
                    if tag == "binary" {
                        for byte in text.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
                            scalar.compact = add(scalar.compact, 1)?;
                            scalar.padding = if byte == b'=' { add(scalar.padding, 1)? } else { 0 };
                        }
                    }
                }
                _ => return Err(BoundError::Malformed),
            }
        }
    }

    fn charge(&mut self, bytes: usize) -> Result<(), BoundError> {
        self.bytes = add(self.bytes, bytes)?;
        Ok(())
    }

    fn text_allocation(&mut self, bytes: usize) -> Result<(), BoundError> {
        if bytes == 0 {
            return Ok(());
        }
        let capacity = mul(bytes, 2)?.max(8);
        self.charge(allocation(mul(capacity, 2)?, 2)?)
    }

    fn collection(&mut self, tag: &str, items: usize) -> Result<(), BoundError> {
        if items == 0 {
            return Ok(());
        }
        if tag == "array" {
            let capacity = mul(items, 2)?.max(4);
            self.charge(allocation(mul(mul(capacity, size_of::<Value>())?, 2)?, 2)?)
        } else {
            let nodes = if items <= 11 { 1 } else { add((items - 1) / 5, 2)? };
            let node_bytes = add(mul(11, size_of::<(String, Value)>())?, mul(16, size_of::<usize>())?)?;
            self.charge(allocation(mul(nodes, node_bytes)?, nodes)?)
        }
    }
}

fn add(a: usize, b: usize) -> Result<usize, BoundError> {
    a.checked_add(b).ok_or(BoundError::Overflow)
}
fn mul(a: usize, b: usize) -> Result<usize, BoundError> {
    a.checked_mul(b).ok_or(BoundError::Overflow)
}

fn allocation(bytes: usize, count: usize) -> Result<usize, BoundError> {
    let overhead = add(mul(4, size_of::<usize>())?, align_of::<Value>())?;
    add(bytes, mul(count, overhead)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_limits_agree() {
        let defaults = Limits::default();
        let inputs: &[&[u8]] = &[
            b"<llsd><array><string>a&amp;b</string><binary> Zg== </binary></array></llsd>",
            b"<llsd><map><key/><array/><key>key</key><uuid/></map></llsd>",
            b"<llsd><array><array><undef/></array></array></llsd>",
            b"<llsd><map><key>same</key><integer>1</integer><key>same</key><integer>2</integer></map></llsd>",
            b"<llsd><string>a<![CDATA[b]]>&#x1f600;</string></llsd>",
        ];
        for input in inputs {
            for n in 0..=8 {
                for limits in [
                    Limits { max_depth: n, ..defaults },
                    Limits { max_values: n, ..defaults },
                    Limits { max_container_items: n, ..defaults },
                    Limits { max_string_bytes: n, ..defaults },
                    Limits { max_map_key_bytes: n, ..defaults },
                    Limits { max_binary_bytes: n, ..defaults },
                ] {
                    if super::super::from_slice_with_limits::<Value>(input, limits).is_ok() {
                        assert!(value_allocation_bound(input, limits).is_ok(), "accepted input rejected: {input:?}, {limits:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn framing_and_input_limits() {
        assert_eq!(value_allocation_bound(b"<llsd><array></map></llsd>", Limits::default()), Err(BoundError::Malformed));
        assert_eq!(
            value_allocation_bound(b"<llsd><undef/></llsd>", Limits { max_input_bytes: 1, ..Limits::default() }),
            Err(BoundError::LimitExceeded("input bytes"))
        );
    }

    #[test]
    fn reports_each_value_limit() {
        let cases: &[(&[u8], Limits, &str)] = &[
            (b"<llsd><undef/></llsd>", Limits { max_values: 0, ..Limits::default() }, "value count"),
            (b"<llsd><array><undef/></array></llsd>", Limits { max_depth: 0, ..Limits::default() }, "nesting depth"),
            (b"<llsd><array><undef/></array></llsd>", Limits { max_container_items: 0, ..Limits::default() }, "container items"),
            (b"<llsd><string>x</string></llsd>", Limits { max_string_bytes: 0, ..Limits::default() }, "string bytes"),
            (b"<llsd><map><key>x</key><undef/></map></llsd>", Limits { max_map_key_bytes: 0, ..Limits::default() }, "map key bytes"),
            (b"<llsd><binary>Zg==</binary></llsd>", Limits { max_binary_bytes: 0, ..Limits::default() }, "binary bytes"),
        ];

        for (input, limits, limit) in cases {
            assert_eq!(value_allocation_bound(input, *limits), Err(BoundError::LimitExceeded(limit)), "{input:?}");
        }
    }

    #[test]
    fn accepts_decoder_supported_xml_shapes() {
        let inputs: &[&[u8]] = &[
            b"<llsd><array/></llsd>",
            b"<llsd><map/></llsd>",
            b"<llsd><map><key/><undef/><key>x</key><string>y</string></map></llsd>",
            b"<llsd><string>one&amp;two<![CDATA[&three]]>four</string></llsd>",
            b"<llsd><binary> Z g = = </binary></llsd>",
            b"<llsd><array><uuid/><date/><uri/></array></llsd>",
        ];

        for input in inputs {
            let limits = Limits::default();
            assert!(super::super::from_slice_with_limits::<Value>(input, limits).is_ok(), "decoder rejected {input:?}");
            assert!(value_allocation_bound(input, limits).is_ok(), "bound scanner rejected {input:?}");
        }
    }

    #[test]
    fn classifies_malformed_xml() {
        for input in [
            &b"<llsd><mystery/></llsd>"[..],
            b"<llsd><array></map></llsd>",
            b"<llsd><map><key>x</key></map></llsd>",
            b"<llsd><undef/></llsd><llsd><undef/></llsd>",
        ] {
            assert_eq!(value_allocation_bound(input, Limits::default()), Err(BoundError::Malformed), "{input:?}");
        }
    }

    #[test]
    fn arithmetic_overflow() {
        assert_eq!(add(usize::MAX, 1), Err(BoundError::Overflow));
        assert_eq!(mul(usize::MAX, 2), Err(BoundError::Overflow));
        assert_eq!(allocation(usize::MAX, 1), Err(BoundError::Overflow));
        if usize::BITS == 32 {
            assert_eq!(mul(1 << 30, 4), Err(BoundError::Overflow));
        }
    }
}
