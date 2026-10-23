pub mod capabilities;
pub mod coverage;
mod error;
mod event;
mod login;
mod session;
mod simulator;
mod status;
#[cfg(test)]
mod test_support;
mod transport;
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

pub(crate) use transport::http;

pub use coverage::{
    CoverageLoss, CoverageLossLedger, CoverageLossReason, CoverageLossRecord, CoverageSnapshot, CoverageSource, CoverageWatermark,
};
pub use error::Error;
pub use event::{CapabilityEvent, Event, EventQueueBatchOrder, PacketOrder, SimulatorId};
pub use login::{AgentSession, Grid, LoginCredentials, StartLocation};
pub use session::{DEFAULT_UPSTREAM_BYTES_PER_SECOND, MfaChallenge, OutboundMessage, Session, SessionHandle, SessionOptions};
pub use status::{NetworkLifecycle, NetworkStatusHandle, NetworkStatusSnapshot};
pub use transport::lludp::Delivery;
