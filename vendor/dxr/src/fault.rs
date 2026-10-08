/// XML-RPC server fault (consisting of a numeric error code and a message)
///
/// *Note*: There are no standardized numeric error codes, and they will likely be
/// specific to the server application.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Server Fault {}: {}", .code, .string)]
pub struct Fault {
    code: i32,
    string: String,
}

impl Fault {
    /// Construct a new [`Fault`] from numeric error code and an error message.
    #[must_use]
    pub const fn new(code: i32, string: String) -> Fault {
        Fault { code, string }
    }

    /// Retrieve the numeric error code from the [`Fault`].
    #[must_use]
    pub const fn code(&self) -> i32 {
        self.code
    }

    /// Retrieve the error message from the [`Fault`].
    #[must_use]
    pub fn string(&self) -> &str {
        self.string.as_str()
    }

    pub(crate) fn into_inner(self) -> (i32, String) {
        (self.code, self.string)
    }
}
