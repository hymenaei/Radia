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
mod circuit;
mod client;
mod error;
mod event;
mod http;
mod login;
mod simulator;
mod status;
mod udp;

pub use capture::CaptureConfig;
pub use circuit::Delivery;
pub use client::{MfaChallenge, OutboundMessage, Session, SessionHandle};
pub use error::Error;
pub use event::{Event, PacketOrder, SimulatorId};
pub use login::{AgentSession, Grid, LoginCredentials, LoginResponseData, LoginValue, StartLocation};
pub use status::{NetworkLifecycle, NetworkStatusHandle, NetworkStatusSnapshot};

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub mod test_support {
    use crate::SimulatorId;

    pub fn simulator_id() -> SimulatorId {
        SimulatorId::allocate().expect("simulator ID space is available during tests")
    }
}
