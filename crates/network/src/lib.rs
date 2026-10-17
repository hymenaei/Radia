//! Bounded login, simulator connections, and capability access for a viewer session.
//!
//! [`Session`] owns the event stream; [`SessionHandle`] provides shared send and lifecycle controls. Dropping the session requests worker shutdown.
//!
//! # Examples
//!
//! ```no_run
//! use network::{Event, Session};
//!
//! async fn receive_connections(mut session: Session) {
//!     while let Some(event) = session.recv_event().await {
//!         if let Event::SimulatorConnected { simulator_id, endpoint, .. } = event {
//!             println!("Simulator {simulator_id:?} connected at {endpoint}");
//!         }
//!     }
//! }
//! ```

pub mod capabilities;
pub mod capture;
pub mod coverage;
mod error;
mod event;
mod login;
mod session;
mod simulator;
mod status;
mod transport;

pub(crate) use transport::http;

pub use capture::CaptureConfig;
pub use coverage::{
    CoverageLoss, CoverageLossLedger, CoverageLossReason, CoverageLossRecord, CoverageSnapshot, CoverageSource, CoverageWatermark,
};
pub use error::Error;
pub use event::{CapabilityEvent, Event, EventQueueBatchOrder, PacketOrder, SimulatorId};
pub use login::{AgentSession, Grid, LoginCredentials, StartLocation};
pub use session::{DEFAULT_UPSTREAM_BYTES_PER_SECOND, MfaChallenge, OutboundMessage, Session, SessionHandle, SessionOptions};
pub use status::{NetworkLifecycle, NetworkStatusHandle, NetworkStatusSnapshot};
pub use transport::lludp::Delivery;

#[cfg(feature = "testing")]
#[doc(hidden)]
pub mod testing {
    use crate::{Error, LoginCredentials, Session, SimulatorId};

    /// Starts the production network worker against a caller-supplied login endpoint.
    ///
    /// This is for deterministic local integration tests; normal callers should use [`Session::start`].
    pub fn start_session_at(credentials: LoginCredentials, login_url: String) -> Result<Session, Error> {
        Session::start_with_login_url_for_test(credentials, login_url)
    }

    pub fn simulator_id() -> SimulatorId {
        SimulatorId::allocate().expect("simulator ID space is available during tests")
    }
}
