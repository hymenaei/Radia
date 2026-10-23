use crate::{
    Error, Result,
    serialization::{Container, Format, Output, Text},
};

pub(super) struct Encoder;

impl Format for Encoder {
    const PREFIX: &'static [u8] = b"<?llsd/binary?>\n";
    const HUMAN_READABLE: bool = false;

    fn boolean(out: &mut Output, value: bool) -> Result<()> {
        out.append(if value { b"1" } else { b"0" })
    }

    fn integer(out: &mut Output, value: i32) -> Result<()> {
        out.append(b"i")?;
        out.append(&value.to_be_bytes())
    }

    fn real(out: &mut Output, value: f64) -> Result<()> {
        out.append(b"r")?;
        out.append(&value.to_be_bytes())
    }

    fn undefined(out: &mut Output) -> Result<()> {
        out.append(b"!")
    }

    fn text(out: &mut Output, value: Text<'_>) -> Result<()> {
        match value {
            Text::String(v) => sized(out, b's', v.as_bytes()),
            Text::Uri(v) => sized(out, b'l', v.as_bytes()),
            Text::Uuid(v) => {
                out.append(b"u")?;
                out.append(v.as_bytes())
            }
            Text::Date(v) => {
                out.append(b"d")?;
                out.append(&v.unix_seconds().to_le_bytes())
            }
        }
    }

    fn bytes(out: &mut Output, value: &[u8]) -> Result<()> {
        sized(out, b'b', value)
    }

    fn open(out: &mut Output, kind: Container, hint: Option<usize>) -> Result<usize> {
        binary_length(hint.unwrap_or(0))?;
        let offset = out.bytes.len();
        out.append(&[kind.start()])?;
        out.append(&[0; 4])?;
        Ok(offset)
    }

    fn close(out: &mut Output, kind: Container, offset: usize, count: usize) -> Result<()> {
        let length = binary_length(count)?;
        out.bytes[offset + 1..offset + 5].copy_from_slice(&length);
        out.append(&[kind.end()])
    }

    fn key(out: &mut Output, key: &str) -> Result<()> {
        sized(out, b'k', key.as_bytes())
    }
}

fn sized(output: &mut Output, tag: u8, bytes: &[u8]) -> Result<()> {
    let length = binary_length(bytes.len())?;
    output.append(&[tag])?;
    output.append(&length)?;
    output.append(bytes)
}

fn binary_length(length: usize) -> Result<[u8; 4]> {
    u32::try_from(length).map(u32::to_be_bytes).map_err(|_| Error::LimitExceeded("encoded length"))
}
