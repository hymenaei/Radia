//! LLUDP message types and packet codecs for the Second Life wire protocol.
//!
//! Decode datagrams with [`udp::Packet::decode`], inspect their typed [`udp::Message`], and encode replies with [`udp::Packet::encode`].
//! Object messages expose checked semantic fields and retain sections this crate does not interpret.
//!
//! # Examples
//!
//! ```
//! use base::UUID;
//! use protocol::{messages::{RegionHandleRequest, region_handle_request::RequestBlock}, udp::{Message, Packet}};
//!
//! let message: Message = RegionHandleRequest { request_block: RequestBlock { region_id: UUID::from_bytes([0; 16]) } }.into();
//! let packet = Packet::new(1, message);
//! let wire = packet.encode()?;
//! let decoded = Packet::decode(&wire)?;
//! assert_eq!(decoded.message(), packet.message());
//! # Ok::<(), protocol::UdpError>(())
//! ```

mod codec;
mod error;
mod message;
/// Typed message structs generated from the checked-in Second Life message template.
pub mod messages;
mod packet;
pub mod schema;
mod schema_message;
mod types;
/// LLUDP packet, header, flag, and message types.
pub mod udp;
mod zero_code;

pub use error::UdpError;
pub use schema_message::{BlockInstance, BlockValues, FieldValue, SchemaMessage};
pub use types::{Audible, ChatSourceType, ChatType, Frequency, Quaternion, Vector3};
