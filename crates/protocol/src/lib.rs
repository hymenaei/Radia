//! Login XML-RPC and LLUDP types and codecs for the Second Life wire protocol.
//!
//! The vendored Linden Lab template generates message IDs and ordered schema metadata. The supported typed message structs and codecs are handwritten.
//!
//! Packet decoding uses typed codecs for supported messages. Other known IDs return [`SchemaMessage`], while unknown IDs and payloads remain in [`lludp::Message::Unknown`].
//!
//! Packet decoding normalizes supported typed IDs to their typed variant, even when the packet was encoded from a [`SchemaMessage`]. Direct schema-message decoding remains generic.
//!
//! Schema-backed decoding zero-fills missing fixed fields, rejects truncated variable prefixes and bodies, and accepts a missing final count for a variable block after earlier fields.
//!
//! Typed codecs handle semantic and packed fields that the template cannot describe.
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

/// LLUDP packet, header, flag, and message types.
pub mod lludp;
pub mod login;
/// Handwritten typed message structs and codecs for supported LLUDP messages.
pub mod messages;
pub mod schema;
/// Typed decoders for payload values shared across LLUDP messages.
pub mod types;

pub use lludp::{Audible, ChatSourceType, ChatType, Frequency, LLUdpError};
pub use lludp::{BlockInstance, BlockValues, FieldValue, SchemaMessage};
