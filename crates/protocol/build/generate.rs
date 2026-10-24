use std::fmt::Write as _;

use super::template::{BlockMode, FieldKind, Frequency, Template, rust_constant_name};

pub fn generate(template: &Template) -> String {
    let mut output = String::new();
    output.push_str("use crate::lludp::types::{Frequency, MessageId};\n");
    output.push_str("use crate::lludp::types::Frequency::{Fixed, High, Low, Medium};\n\n");
    output.push_str("/// Trust classification declared by an LLUDP message template.\n");
    output.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    output.push_str(
        "pub enum MessageTrust {\n    /// The message requires a trusted circuit.\n    Trusted,\n    /// The message does not require a trusted circuit.\n    NotTrusted,\n}\n\n",
    );
    output.push_str("/// Payload encoding declared by an LLUDP message template.\n");
    output.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    output.push_str(
        "pub enum MessageEncoding {\n    /// The datagram's message bytes use LLUDP zero coding.\n    Zerocoded,\n    /// The datagram's message bytes are sent without zero coding.\n    Unencoded,\n}\n\n",
    );
    output.push_str("/// Flags describing template status or LLUDP policy for a message.\n");
    output.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    output.push_str(
        "pub enum MessageFlag {\n    /// The template marks the message as deprecated.\n    Deprecated,\n    /// The template blacklists the message for LLUDP.\n    LLUdpBlacklisted,\n    /// The template marks LLUDP use of the message as deprecated.\n    LLUdpDeprecated,\n}\n\n",
    );
    output.push_str("/// Repetition mode of a message block in the wire payload.\n");
    output.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    output.push_str(
        "pub enum BlockMode {\n    /// Exactly one block instance, with no count prefix.\n    Single,\n    /// A block count followed by that many instances; the count is one byte.\n    Variable,\n    /// A fixed number of instances, with the count stored in the schema rather than the payload.\n    Multiple(\n        /// Number of block instances.\n        u32,\n    ),\n}\n\n",
    );
    output.push_str(
        "/// Wire encoding of one field in an LLUDP message template. `S` and `U` variants are signed and unsigned integers named by bit width; multibyte numeric values use little-endian order.\n",
    );
    output.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    output.push_str("pub enum FieldKind {\n");
    output.push_str(
        "    /// Boolean encoded as one byte; any nonzero byte decodes as true.\n    Bool,\n    /// 32-bit IEEE-754 float in little-endian order.\n    F32,\n    /// 64-bit IEEE-754 float in little-endian order.\n    F64,\n    /// Four IPv4 address octets in wire order.\n    IpAddr,\n    /// Two port bytes in wire order.\n    IpPort,\n    /// Field with no payload bytes.\n    Null,\n    /// Three little-endian `f32` quaternion components.\n    Quaternion,\n    /// 16-byte UUID in the wire byte order.\n    Uuid,\n",
    );
    output.push_str(
        "    /// Three little-endian `f32` components.\n    Vector3,\n    /// Three little-endian `f64` components.\n    Vector3d,\n    /// Four little-endian `f32` components.\n    Vector4,\n    S8,\n    S16,\n    S32,\n    S64,\n    U8,\n    U16,\n    U32,\n    U64,\n",
    );
    output.push_str(
        "    /// Raw bytes whose size is fixed by the template.\n    Fixed(\n        /// Number of field bytes.\n        u32,\n    ),\n    /// Raw bytes with a length prefix.\n    Variable(\n        /// Prefix width in bytes; only 1 and 2 are supported.\n        u8,\n    ),\n}\n\n",
    );
    output.push_str("/// Name and wire encoding of one field in a message block.\n");
    output.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    output.push_str(
        "pub struct FieldSchema {\n    /// Field name from the message template.\n    pub name: &'static str,\n    /// Wire encoding for this field.\n    pub kind: FieldKind,\n}\n\n",
    );
    output.push_str("/// Field definitions and repetition mode for one message block.\n");
    output.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    output.push_str(
        "pub struct BlockSchema {\n    /// Block name from the message template.\n    pub name: &'static str,\n    /// Repetition count for this block.\n    pub mode: BlockMode,\n    /// Fields in wire order.\n    pub fields: &'static [FieldSchema],\n}\n\n",
    );
    output.push_str("/// Wire ID, policy, encoding, flags, and ordered blocks for one template message.\n");
    output.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    output.push_str("pub struct MessageSchema {\n");
    output.push_str(
        "    /// Message name from the template.\n    pub name: &'static str,\n    /// Frequency and number used by the wire protocol.\n    pub id: MessageId,\n    /// Trust classification from the template.\n    pub trust: MessageTrust,\n",
    );
    output.push_str(
        "    /// Default zero-coding mode from the template.\n    pub encoding: MessageEncoding,\n    /// Status and LLUDP-policy flags from the template.\n    pub flags: &'static [MessageFlag],\n",
    );
    output.push_str("    /// Blocks in wire order.\n    pub blocks: &'static [BlockSchema],\n}\n\n");
    output.push_str("/// Number of messages in the compiled template.\n");
    writeln!(&mut output, "pub const MESSAGE_COUNT: usize = {};\n", template.messages.len()).expect("write generated source");
    output.push_str("/// Messages in the order declared by the template.\npub const MESSAGES: &[MessageSchema] = &[\n");

    for message in &template.messages {
        output.push_str("    MessageSchema {\n");
        writeln!(&mut output, "        name: {:?},", message.name).expect("write generated source");
        writeln!(&mut output, "        id: MessageId::{}({}),", frequency_constructor(message.frequency), message.id)
            .expect("write generated source");
        writeln!(&mut output, "        trust: MessageTrust::{:?},", message.trust).expect("write generated source");
        writeln!(&mut output, "        encoding: MessageEncoding::{:?},", message.encoding).expect("write generated source");
        if message.flags.is_empty() {
            output.push_str("        flags: &[],\n");
        } else {
            output.push_str("        flags: &[\n");
            for flag in &message.flags {
                writeln!(&mut output, "            MessageFlag::{flag:?},").expect("write generated source");
            }
            output.push_str("        ],\n");
        }
        if message.blocks.is_empty() {
            output.push_str("        blocks: &[],\n");
        } else {
            output.push_str("        blocks: &[\n");
            for block in &message.blocks {
                output.push_str("            BlockSchema {\n");
                writeln!(&mut output, "                name: {:?},", block.name).expect("write generated source");
                writeln!(&mut output, "                mode: {},", block_mode(block.mode)).expect("write generated source");
                if block.fields.is_empty() {
                    output.push_str("                fields: &[],\n");
                } else {
                    output.push_str("                fields: &[\n");
                    for field in &block.fields {
                        output.push_str("                    FieldSchema {\n");
                        writeln!(&mut output, "                        name: {:?},", field.name).expect("write generated source");
                        writeln!(&mut output, "                        kind: {},", field_kind(field.kind)).expect("write generated source");
                        output.push_str("                    },\n");
                    }
                    output.push_str("                ],\n");
                }
                output.push_str("            },\n");
            }
            output.push_str("        ],\n");
        }
        output.push_str("    },\n");
    }
    let mut id_index = (0..template.messages.len()).collect::<Vec<_>>();
    id_index.sort_by_key(|index| {
        let message = &template.messages[*index];
        (frequency_rank(message.frequency), message.id)
    });
    output.push_str("];\n\n");
    output.push_str("// This sorted indirection keeps ID lookup logarithmic without reordering the readable schema list.\n");
    output.push_str("const MESSAGE_ID_INDEX: &[usize] = &[\n");
    for index in id_index {
        writeln!(&mut output, "    {index},").expect("write generated source");
    }
    output.push_str("];\n\n");
    output.push_str("fn frequency_rank(frequency: Frequency) -> u8 {\n");
    output.push_str("    match frequency {\n");
    output.push_str("        High => 0,\n");
    output.push_str("        Medium => 1,\n");
    output.push_str("        Low => 2,\n");
    output.push_str("        Fixed => 3,\n");
    output.push_str("    }\n}\n\n");
    output.push_str(
        "/// Find the schema for an exact message frequency and number.\n///\n/// Returns `None` when the template has no matching ID.\npub fn find_by_id(id: MessageId) -> Option<&'static MessageSchema> {\n",
    );
    output.push_str("    let key = (frequency_rank(id.frequency), id.number);\n");
    output.push_str("    let index = MESSAGE_ID_INDEX.binary_search_by_key(&key, |index| {\n");
    output.push_str("        let message = &MESSAGES[*index];\n");
    output.push_str("        (frequency_rank(message.id.frequency), message.id.number)\n");
    output.push_str("    }).ok()?;\n");
    output.push_str("    Some(&MESSAGES[MESSAGE_ID_INDEX[index]])\n}\n\n");
    output.push_str(
        "/// Find a schema by its exact, case-sensitive template message name.\n///\n/// Returns `None` when the template has no matching name.\npub fn find_by_name(name: &str) -> Option<&'static MessageSchema> {\n",
    );
    output.push_str("    MESSAGES.iter().find(|message| message.name == name)\n}\n\n");
    output.push_str(
        "/// Named wire identifiers for messages in the checked-in template.\n///\n/// Each value uses the message's template frequency and number.\npub mod ids {\n    use crate::lludp::types::MessageId;\n",
    );
    for message in &template.messages {
        writeln!(
            &mut output,
            "    pub const {}: MessageId = MessageId::{}({});",
            rust_constant_name(&message.name),
            frequency_constructor(message.frequency),
            message.id
        )
        .expect("write generated source");
    }
    output.push_str("}\n");
    output
}

fn frequency_constructor(frequency: Frequency) -> &'static str {
    match frequency {
        Frequency::High => "high",
        Frequency::Medium => "medium",
        Frequency::Low => "low",
        Frequency::Fixed => "fixed",
    }
}

fn frequency_rank(frequency: Frequency) -> u8 {
    match frequency {
        Frequency::High => 0,
        Frequency::Medium => 1,
        Frequency::Low => 2,
        Frequency::Fixed => 3,
    }
}

fn block_mode(mode: BlockMode) -> String {
    match mode {
        BlockMode::Single => "BlockMode::Single".into(),
        BlockMode::Variable => "BlockMode::Variable".into(),
        BlockMode::Multiple(count) => format!("BlockMode::Multiple({count})"),
    }
}

fn field_kind(kind: FieldKind) -> String {
    match kind {
        FieldKind::Bool => "FieldKind::Bool".into(),
        FieldKind::F32 => "FieldKind::F32".into(),
        FieldKind::F64 => "FieldKind::F64".into(),
        FieldKind::IpAddr => "FieldKind::IpAddr".into(),
        FieldKind::IpPort => "FieldKind::IpPort".into(),
        FieldKind::Null => "FieldKind::Null".into(),
        FieldKind::Quaternion => "FieldKind::Quaternion".into(),
        FieldKind::Uuid => "FieldKind::Uuid".into(),
        FieldKind::Vector3 => "FieldKind::Vector3".into(),
        FieldKind::Vector3d => "FieldKind::Vector3d".into(),
        FieldKind::Vector4 => "FieldKind::Vector4".into(),
        FieldKind::S8 => "FieldKind::S8".into(),
        FieldKind::S16 => "FieldKind::S16".into(),
        FieldKind::S32 => "FieldKind::S32".into(),
        FieldKind::S64 => "FieldKind::S64".into(),
        FieldKind::U8 => "FieldKind::U8".into(),
        FieldKind::U16 => "FieldKind::U16".into(),
        FieldKind::U32 => "FieldKind::U32".into(),
        FieldKind::U64 => "FieldKind::U64".into(),
        FieldKind::Fixed(size) => format!("FieldKind::Fixed({size})"),
        FieldKind::Variable(size) => format!("FieldKind::Variable({size})"),
    }
}
