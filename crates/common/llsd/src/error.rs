use std::{fmt, io};

pub type Result<T> = std::result::Result<T, Error>;

/// A malformed LLSD value, I/O failure, or configured resource limit.
#[derive(Debug)]
pub enum Error {
    /// The underlying reader or writer failed.
    Io(io::Error),
    /// Input was malformed or could not represent the requested Serde value.
    InvalidValue(String),
    /// Text input is not valid UTF-8.
    InvalidUtf8,
    /// Input ended before a complete value was available.
    UnexpectedEof,
    /// Input contains bytes after its value.
    TrailingData,
    /// A configured input, nesting, value-count, container-item, string, map-key, binary,
    /// or encoded-output limit was exceeded.
    LimitExceeded(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "LLSD I/O error: {error}"),
            Self::InvalidValue(error) => write!(formatter, "invalid LLSD value: {error}"),
            Self::InvalidUtf8 => formatter.write_str("LLSD text is not valid UTF-8"),
            Self::UnexpectedEof => formatter.write_str("unexpected end of LLSD input"),
            Self::TrailingData => formatter.write_str("trailing data after the LLSD value"),
            Self::LimitExceeded(limit) => write!(formatter, "LLSD {limit} limit exceeded"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl serde::ser::Error for Error {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self::InvalidValue(message.to_string())
    }
}

impl serde::de::Error for Error {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self::InvalidValue(message.to_string())
    }
}
