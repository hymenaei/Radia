//! LLUDP message types and packet codecs for the Second Life wire protocol.
//!
//! Decode datagrams with [`lludp::Packet::decode`], inspect their typed [`lludp::Message`], and encode replies with [`lludp::Packet::encode`].
//! Object messages expose checked semantic fields and retain sections this crate does not interpret.
//!
//! # Examples
//!
//! ```
//! use base::UUID;
//! use protocol::{messages::{RegionHandleRequest, region_handle_request::RequestBlock}, lludp::{Message, Packet}};
//!
//! let message: Message = RegionHandleRequest { request_block: RequestBlock { region_id: UUID::from_bytes([0; 16]) } }.into();
//! let packet = Packet::new(1, message);
//! let wire = packet.encode()?;
//! let decoded = Packet::decode(&wire)?;
//! assert_eq!(decoded.message(), packet.message());
//! # Ok::<(), protocol::LLUdpError>(())
//! ```

mod codec;
mod error;
/// LLUDP packet, header, flag, and message types.
pub mod lludp;
mod message;
/// Typed message structs generated from the checked-in Second Life message template.
pub mod messages;
mod packet;
pub mod schema;
mod schema_message;
mod types;
mod zero_code;

pub use error::LLUdpError;
pub use schema_message::{BlockInstance, BlockValues, FieldValue, SchemaMessage};
pub use types::{Audible, ChatSourceType, ChatType, Frequency};
