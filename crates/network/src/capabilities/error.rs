use std::{error::Error as StdError, fmt, sync::Arc};

use crate::http;

#[derive(Clone, Debug)]
pub(crate) enum SetupError {
    InvalidSeed(Arc<http::Error>),
    RequestRuntimeUnavailable,
    EventQueueRuntimeUnavailable,
    HttpClient(Arc<reqwest::Error>),
}

impl fmt::Display for SetupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSeed(error) => error.fmt(formatter),
            Self::RequestRuntimeUnavailable => formatter.write_str("A Tokio runtime is required for capability requests."),
            Self::EventQueueRuntimeUnavailable => formatter.write_str("The simulator event queue requires an active Tokio runtime."),
            Self::HttpClient(error) => write!(formatter, "Could not prepare the capability client: {error}"),
        }
    }
}

impl StdError for SetupError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::InvalidSeed(error) => Some(error.as_ref()),
            Self::HttpClient(error) => Some(error.as_ref()),
            Self::RequestRuntimeUnavailable | Self::EventQueueRuntimeUnavailable => None,
        }
    }
}
