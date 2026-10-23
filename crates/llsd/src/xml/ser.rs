use std::fmt;

use crate::{
    Error, Result,
    encoding::real_text,
    serialization::{Container, Format, Output, Text},
};

pub(super) struct Encoder;
impl Format for Encoder {
    const PREFIX: &'static [u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><llsd>";
    const SUFFIX: &'static [u8] = b"</llsd>";

    fn boolean(out: &mut Output, value: bool) -> Result<()> {
        write_text_element(out, "boolean", if value { "true" } else { "false" })
    }

    fn integer(out: &mut Output, value: i32) -> Result<()> {
        write_text_element(out, "integer", &value.to_string())
    }

    fn real(out: &mut Output, value: f64) -> Result<()> {
        write_text_element(out, "real", &real_text(value))
    }

    fn undefined(out: &mut Output) -> Result<()> {
        write_text_element(out, "undef", "")
    }

    fn text(out: &mut Output, value: Text<'_>) -> Result<()> {
        match value {
            Text::String(v) => write_text_element(out, "string", v),
            Text::Uri(v) => write_text_element(out, "uri", v),
            Text::Uuid(v) => write_text_element(out, "uuid", &v.to_string()),
            Text::Date(v) => write_text_element(out, "date", &v.format()?),
        }
    }

    fn bytes(out: &mut Output, value: &[u8]) -> Result<()> {
        out.base64(value, b"<binary>", b"</binary>")
    }

    fn open(out: &mut Output, kind: Container, _hint: Option<usize>) -> Result<usize> {
        let offset = out.bytes.len();
        out.append(match kind {
            Container::Array => b"<array>",
            Container::Map => b"<map>",
        })?;
        Ok(offset)
    }

    fn close(out: &mut Output, kind: Container, _offset: usize, _count: usize) -> Result<()> {
        out.append(match kind {
            Container::Array => b"</array>",
            Container::Map => b"</map>",
        })
    }

    fn key(out: &mut Output, key: &str) -> Result<()> {
        write_text_element(out, "key", key)
    }

    fn charge_key(key: &str, budget: usize) -> Result<()> {
        charge_xml_key(key, budget)
    }

    fn display<T: fmt::Display + ?Sized>(value: &T, tag: &str, budget: Option<usize>) -> Result<String> {
        match budget {
            Some(budget) => bounded_display(value, budget.saturating_sub(xml_tag_overhead(tag))),
            None => Ok(value.to_string()),
        }
    }
}

struct BoundedDisplay {
    value: String,
    encoded_len: usize,
    max_encoded_len: usize,
    error: Option<Error>,
}

impl fmt::Write for BoundedDisplay {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let length = match xml_escaped_len(text) {
            Ok(length) => length,
            Err(error) => {
                self.error = Some(error);
                return Err(fmt::Error);
            }
        };
        let Some(encoded_len) = self.encoded_len.checked_add(length) else {
            self.error = Some(Error::LimitExceeded("encoded length"));
            return Err(fmt::Error);
        };
        if encoded_len > self.max_encoded_len {
            self.error = Some(Error::LimitExceeded("encoded length"));
            return Err(fmt::Error);
        }
        self.value.push_str(text);
        self.encoded_len = encoded_len;
        Ok(())
    }
}

fn write_text_element(output: &mut Output, tag: &str, text: &str) -> Result<()> {
    let length = xml_tag_overhead(tag).checked_add(xml_escaped_len(text)?).ok_or(Error::LimitExceeded("encoded length"))?;
    output.check_space(length)?;
    output.bytes.reserve(length);
    output.bytes.push(b'<');
    output.bytes.extend_from_slice(tag.as_bytes());
    output.bytes.push(b'>');
    escape_xml(text, &mut output.bytes);
    output.bytes.extend_from_slice(b"</");
    output.bytes.extend_from_slice(tag.as_bytes());
    output.bytes.push(b'>');
    Ok(())
}

fn xml_tag_overhead(tag: &str) -> usize {
    5 + tag.len() * 2
}

fn charge_xml_key(key: &str, budget: usize) -> Result<()> {
    let length = xml_tag_overhead("key").checked_add(xml_escaped_len(key)?).ok_or(Error::LimitExceeded("encoded length"))?;
    if length > budget {
        return Err(Error::LimitExceeded("encoded length"));
    }
    Ok(())
}

fn xml_escaped_len(text: &str) -> Result<usize> {
    let mut length = 0usize;
    for character in text.chars() {
        if matches!(character, '\u{0}'..='\u{8}' | '\u{b}'..='\u{c}' | '\u{e}'..='\u{1f}' | '\u{fffe}' | '\u{ffff}') {
            return Err(Error::InvalidValue("text contains a character forbidden in XML 1.0".into()));
        }
        let escaped_len = match character {
            '&' => 5,
            '<' | '>' => 4,
            '"' | '\'' => 6,
            '\r' => 5,
            value => value.len_utf8(),
        };
        length = length.checked_add(escaped_len).ok_or(Error::LimitExceeded("encoded length"))?;
    }
    Ok(length)
}

fn bounded_display<T: fmt::Display + ?Sized>(value: &T, max_encoded_len: usize) -> Result<String> {
    let mut output = BoundedDisplay { value: String::new(), encoded_len: 0, max_encoded_len, error: None };
    let formatted = fmt::write(&mut output, format_args!("{value}"));
    if let Some(error) = output.error.take() {
        return Err(error);
    }
    if formatted.is_err() {
        return Err(Error::InvalidValue("string formatting failed".into()));
    }
    Ok(output.value)
}

fn escape_xml(text: &str, output: &mut Vec<u8>) {
    for character in text.chars() {
        match character {
            '&' => output.extend_from_slice(b"&amp;"),
            '<' => output.extend_from_slice(b"&lt;"),
            '>' => output.extend_from_slice(b"&gt;"),
            '\"' => output.extend_from_slice(b"&quot;"),
            '\'' => output.extend_from_slice(b"&apos;"),
            '\r' => output.extend_from_slice(b"&#13;"),
            value => {
                let mut bytes = [0; 4];
                output.extend_from_slice(value.encode_utf8(&mut bytes).as_bytes());
            }
        }
    }
}
