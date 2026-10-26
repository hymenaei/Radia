mod ais;
mod decode;
mod legacy;
mod scheduler;
mod state;

use std::{fmt, time::Duration};

use ::inventory::{CategoryKey, InventoryChild, InventoryError, ServerVersion};
use tokio::sync::oneshot;

pub(crate) use scheduler::InventoryFetchController;
pub use scheduler::{InventoryFetchHandle, InventoryFetchOutcome, InventoryFetchRequestError, InventoryLoadingStatus};
pub(super) use scheduler::{fetch_single_folder, fetch_single_items};

const MAX_FETCH_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_AIS_DEPTH: u8 = 16;
const MAX_FALLBACK_FOLDERS: usize = 10;
const MAX_FETCH_RETRIES: u32 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestFailure {
    Forbidden,
    Missing,
    FolderTooLarge,
    Transient(Option<Duration>),
    InvalidResponse,
    Failed,
}

fn classify_request_error(error: network::Error) -> RequestFailure {
    match error {
        network::Error::CapabilityHttp(error) if error.status.is_some_and(|status| status.as_u16() == 403) => RequestFailure::Forbidden,
        network::Error::CapabilityHttp(error) if error.status.is_some_and(|status| matches!(status.as_u16(), 404 | 410)) => {
            RequestFailure::Missing
        }
        network::Error::CapabilityHttp(error)
            if error.kind == network::capabilities::HttpErrorKind::Transport
                || error.kind == network::capabilities::HttpErrorKind::Timeout
                || error.status.is_some_and(|status| status.is_server_error() || status.as_u16() == 429) =>
        {
            RequestFailure::Transient(error.retry_after)
        }
        network::Error::CapabilityHttp(error) if error.kind == network::capabilities::HttpErrorKind::ResourceExhausted => {
            RequestFailure::FolderTooLarge
        }
        network::Error::Capability(_) => RequestFailure::InvalidResponse,
        _ => RequestFailure::Failed,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InventoryFetchError {
    CapabilityUnavailable,
    FolderTooLarge,
    InvalidResponse,
    RetryExhausted,
    RequestFailed,
    Model(InventoryError),
}

impl fmt::Display for InventoryFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::CapabilityUnavailable => "inventory fetch capability is unavailable",
            Self::FolderTooLarge => "inventory folder exceeds the server fetch limit",
            Self::InvalidResponse => "inventory server returned an invalid folder response",
            Self::RetryExhausted => "inventory folder fetch retries were exhausted",
            Self::RequestFailed => "inventory folder request failed",
            Self::Model(error) => return write!(formatter, "{error}"),
        })
    }
}

impl std::error::Error for InventoryFetchError {}

impl From<InventoryError> for InventoryFetchError {
    fn from(error: InventoryError) -> Self {
        Self::Model(error)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FolderFetch {
    pub(crate) folder: CategoryKey,
    pub(crate) version: ServerVersion,
    pub(crate) descendent_count: Option<u32>,
    pub(crate) children: Vec<InventoryChild>,
}

#[derive(Debug)]
pub(crate) struct FolderApplyOutcome {
    pub(crate) rejected: bool,
}

#[derive(Debug)]
pub(crate) enum InventoryFetchEvent {
    Folder { fetch: FolderFetch, applied: oneshot::Sender<FolderApplyOutcome> },
    Failed { folder: CategoryKey, error: InventoryFetchError },
    BackgroundDone,
}
