use crate::{
    Result,
    encoding::real_text,
    serialization::{Container, Format, Output, Text},
};

pub(super) struct Encoder;

impl Format for Encoder {
    const PREFIX: &'static [u8] = b"<?llsd/notation?>\n";

    fn boolean(out: &mut Output, value: bool) -> Result<()> {
        out.append(if value { b"true" } else { b"false" })
    }

    fn integer(out: &mut Output, value: i32) -> Result<()> {
        out.append(b"i")?;
        out.append(value.to_string().as_bytes())
    }

    fn real(out: &mut Output, value: f64) -> Result<()> {
        out.append(b"r")?;
        out.append(real_text(value).as_bytes())
    }

    fn undefined(out: &mut Output) -> Result<()> {
        out.append(b"!")
    }

    fn text(out: &mut Output, value: Text<'_>) -> Result<()> {
        match value {
            Text::String(v) => quoted(out, v, b'\''),
            Text::Uri(v) => {
                out.append(b"l")?;
                quoted(out, v, b'"')
            }
            Text::Uuid(v) => {
                out.append(b"u")?;
                out.append(v.to_string().as_bytes())
            }
            Text::Date(v) => {
                out.append(b"d")?;
                quoted(out, &v.format()?, b'"')
            }
        }
    }

    fn bytes(out: &mut Output, value: &[u8]) -> Result<()> {
        out.base64(value, b"b64\"", b"\"")
    }

    fn open(out: &mut Output, kind: Container, _hint: Option<usize>) -> Result<usize> {
        let offset = out.bytes.len();
        out.append(&[kind.start()])?;
        Ok(offset)
    }

    fn close(out: &mut Output, kind: Container, _offset: usize, _count: usize) -> Result<()> {
        out.append(&[kind.end()])
    }

    fn key(out: &mut Output, key: &str) -> Result<()> {
        quoted(out, key, b'\'')?;
        out.append(b":")
    }

    fn separator(out: &mut Output, count: usize) -> Result<()> {
        if count > 0 {
            out.append(b",")?;
        }
        Ok(())
    }
}

fn quoted(output: &mut Output, text: &str, quote: u8) -> Result<()> {
    output.append(&[quote])?;
    for &byte in text.as_bytes() {
        match byte {
            b'\\' => output.append(b"\\\\")?,
            b'\n' => output.append(b"\\n")?,
            b'\r' => output.append(b"\\r")?,
            b'\t' => output.append(b"\\t")?,
            byte if byte == quote => output.append(&[b'\\', byte])?,
            0..=0x1f | 0x7f => {
                const HEX: &[u8] = b"0123456789abcdef";
                output.append(&[b'\\', b'x', HEX[(byte >> 4) as usize], HEX[(byte & 15) as usize]])?;
            }
            byte => output.append(&[byte])?,
        }
    }
    output.append(&[quote])
}
