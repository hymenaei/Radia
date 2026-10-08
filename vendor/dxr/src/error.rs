use std::borrow::Cow;

use crate::fault::Fault;

#[derive(Debug, PartialEq, thiserror::Error)]
#[non_exhaustive]
/// Error type representing conversion errors between XML-RPC values and Rust values.
pub enum Error {
    /// Error variant for XML parser errors.
    #[error("Failed to parse XML data: {}", .error)]
    InvalidData {
        /// description of the parsing error
        error: String,
    },
    /// Error variant for invalid dateTime.iso8601 values.
    #[error("Invalid format for dateTime.iso8601 value: \n{}", .error)]
    InvalidDateTime {
        /// description of the parsing error
        error: String,
    },
    /// Error variant for a missing struct field.
    #[error("Struct '{}' missing field: {}", .name, .field)]
    MissingField {
        /// name of the struct that has a missing field
        name: Cow<'static, str>,
        /// name of the missing struct field
        field: Cow<'static, str>,
    },
    #[error("Parameter mismatch: got {} values, expected {}", .argument, .expected)]
    /// Error variant for mismatch with an expected number of values.
    ParameterMismatch {
        /// number of received values
        argument: usize,
        /// number of expected values
        expected: usize,
    },
    /// Error variant for mismatch with an expected value type.
    #[error("Type mismatch: got {}, expected {}", .argument, .expected)]
    WrongType {
        /// mismatched input type
        argument: Cow<'static, str>,
        /// expected input type
        expected: Cow<'static, str>,
    },
}

impl Error {
    /// Construct a [`Error`] for invalid input data.
    #[must_use]
    pub const fn invalid_data(error: String) -> Error {
        Error::InvalidData { error }
    }

    /// Check if a given [`Error`] was raised for invalid data.
    #[must_use]
    pub const fn is_invalid_data(&self) -> bool {
        matches!(self, Error::InvalidData { .. })
    }

    /// Check for [`Error::InvalidData`] and return the inner error in case of a match.
    ///
    /// The returned string describes the XML (de)serialization issue.
    #[must_use]
    pub fn as_invalid_data(&self) -> Option<&str> {
        if let Error::InvalidData { error } = self {
            Some(error)
        } else {
            None
        }
    }

    /// Construct an [`Error`] for invalid datetime values.
    #[must_use]
    pub const fn invalid_datetime(error: String) -> Error {
        Error::InvalidDateTime { error }
    }

    /// Check if a given [`Error`] was raised for an invalid datetime value.
    #[must_use]
    pub const fn is_invalid_datetime(&self) -> bool {
        matches!(self, Error::InvalidDateTime { .. })
    }

    /// Check for [`Error::InvalidDateTime`] and return the inner error in case of a match.
    ///
    /// The returned string describes the parsing failure.
    #[must_use]
    pub fn as_invalid_datetime(&self) -> Option<&str> {
        if let Error::InvalidDateTime { error } = self {
            Some(error)
        } else {
            None
        }
    }

    /// Construct a [`Error`] for a missing struct field.
    #[must_use]
    pub const fn missing_field(name: &'static str, field: &'static str) -> Error {
        Error::MissingField {
            name: Cow::Borrowed(name),
            field: Cow::Borrowed(field),
        }
    }

    /// Check if a given [`Error`] was raised for a missing struct field.
    #[must_use]
    pub const fn is_missing_field(&self) -> bool {
        matches!(self, Error::MissingField { .. })
    }

    /// Check for [`Error::MissingField`] and return the inner error in case of a match.
    ///
    /// The returned value is a tuple of (struct name, missing field name).
    #[must_use]
    pub fn as_missing_field(&self) -> Option<(&str, &str)> {
        if let Error::MissingField { name, field } = self {
            Some((name, field))
        } else {
            None
        }
    }

    /// Construct a [`Error`] for a parameter number mismatch.
    #[must_use]
    pub const fn parameter_mismatch(argument: usize, expected: usize) -> Error {
        Error::ParameterMismatch { argument, expected }
    }

    /// Check if a given [`Error`] was raised for unexpected number of return values.
    #[must_use]
    pub const fn is_parameter_mismatch(&self) -> bool {
        matches!(self, Error::ParameterMismatch { .. })
    }

    /// Check for [`Error::ParameterMismatch`] and return the inner error in case of a match.
    ///
    /// The returned value is a tuple of the numbers of (received arguments, expected arguments).
    #[must_use]
    pub const fn as_parameter_mismatch(&self) -> Option<(usize, usize)> {
        if let Error::ParameterMismatch { argument, expected } = self {
            Some((*argument, *expected))
        } else {
            None
        }
    }

    /// Construct a [`Error`] for a type mismatch.
    #[must_use]
    pub const fn wrong_type(argument: &'static str, expected: &'static str) -> Error {
        Error::WrongType {
            argument: Cow::Borrowed(argument),
            expected: Cow::Borrowed(expected),
        }
    }

    /// Check if a given [`Error`] was raised for a type mismatch.
    #[must_use]
    pub const fn is_wrong_type(&self) -> bool {
        matches!(self, Error::WrongType { .. })
    }

    /// Check for [`Error::WrongType`] and return the inner error in case of a match.
    ///
    /// The returned value is a tuple of the names of (received type, expected type).
    #[must_use]
    pub fn as_wrong_type(&self) -> Option<(&str, &str)> {
        if let Error::WrongType { argument, expected } = self {
            Some((argument, expected))
        } else {
            None
        }
    }
}

impl From<Error> for Fault {
    fn from(error: Error) -> Self {
        Fault::new(400, error.to_string())
    }
}
