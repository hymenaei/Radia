pub mod capabilities;
mod circuit;
mod coverage;
mod error;
mod event;
#[cfg(test)]
mod fixtures;
mod http;
mod login;
mod session;

#[cfg(feature = "testing")]
#[doc(hidden)]
pub mod testing {
    use crate::{Error, LoginCredentials, Session, SimulatorId};

    pub fn start_session_at(credentials: LoginCredentials, login_url: String) -> Result<Session, Error> {
        Session::start_with_login_url_for_test(credentials, login_url)
    }

    pub fn simulator_id() -> SimulatorId {
        SimulatorId::allocate().expect("simulator ID space is available during tests")
    }
}
pub use circuit::lludp::Delivery;
pub use coverage::{
    CoverageLoss, CoverageLossLedger, CoverageLossReason, CoverageLossRecord, CoverageSnapshot, CoverageSource, CoverageWatermark,
};
pub use error::Error;
pub use event::{CapabilityEvent, Event, EventQueueBatchOrder, PacketOrder, SimulatorId};
pub use login::{AgentSession, Grid, LoginCredentials, StartLocation};
pub use session::{DEFAULT_UPSTREAM_BYTES_PER_SECOND, MfaChallenge, OutboundMessage, Session, SessionHandle, SessionOptions};
