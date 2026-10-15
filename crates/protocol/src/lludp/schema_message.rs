use std::mem::size_of;

use base::UUID;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        types::MessageId,
    },
    schema::{self, BlockMode, FieldKind, FieldSchema, MessageSchema},
};
use math::{DVec3, Quat, Vec3, Vec4};

/// A typed value for one schema field.
///
/// Multi-byte numbers use little-endian encoding. IP addresses and ports keep wire byte order.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    /// Boolean encoded as one byte; any nonzero byte decodes as `true`.
    Bool(bool),
    F32(f32),
    F64(f64),
    /// Four IPv4 address octets in wire order.
    IpAddr([u8; 4]),
    /// Two port bytes in wire order.
    IpPort([u8; 2]),
    /// Encodes as a zero-length field with no payload bytes.
    Null,
    /// Encodes normalized XYZ as three little-endian `f32` values with canonical nonnegative W.
    /// Zero or non-finite quaternions fail encoding.
    Quaternion(Quat),
    /// UUID bytes in the order returned by `UUID::as_bytes`.
    UUID(UUID),
    Vector3(Vec3),
    Vector3d(DVec3),
    Vector4(Vec4),
    S8(i8),
    S16(i16),
    S32(i32),
    S64(i64),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    /// Raw bytes for a `Fixed` or `Variable` schema field.
    Bytes(Vec<u8>),
}

/// The field values for one instance of a schema block.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockInstance {
    /// Values in the order declared by the corresponding `BlockSchema::fields` slice.
    pub fields: Vec<FieldValue>,
}

impl BlockInstance {
    pub fn new(fields: Vec<FieldValue>) -> Self {
        Self { fields }
    }
}

/// Instances for one schema block, kept in wire order.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockValues {
    pub instances: Vec<BlockInstance>,
}

impl BlockValues {
    pub fn new(instances: Vec<BlockInstance>) -> Self {
        Self { instances }
    }

    pub fn single(fields: Vec<FieldValue>) -> Self {
        Self::new(vec![BlockInstance::new(fields)])
    }
}

/// A runtime wire message represented by compiled schema metadata and ordered values.
#[derive(Clone, Debug)]
pub struct SchemaMessage {
    schema: &'static MessageSchema,
    blocks: Vec<BlockValues>,
    encoded_schema_len: Option<usize>,
    field_presence: Vec<u8>,
    trailing_bytes: Vec<u8>,
}

impl PartialEq for SchemaMessage {
    fn eq(&self, other: &Self) -> bool {
        self.schema == other.schema
            && self.blocks == other.blocks
            && self.trailing_bytes == other.trailing_bytes
            && self.same_field_presence(other)
    }
}

impl SchemaMessage {
    /// Create a message from its compiled schema and ordered block values.
    ///
    /// # Errors
    ///
    /// Returns [`LLUdpError::InvalidSchemaMessage`] when the blocks, instance counts, or field
    /// values do not match the schema.
    pub fn new(schema: &'static MessageSchema, blocks: Vec<BlockValues>) -> Result<Self, LLUdpError> {
        let message = Self { schema, blocks, encoded_schema_len: None, field_presence: Vec::new(), trailing_bytes: Vec::new() };
        message.validate()?;
        Ok(message)
    }

    pub fn schema_by_id(id: MessageId) -> Option<&'static MessageSchema> {
        schema::find_by_id(id)
    }

    pub fn schema_by_name(name: &str) -> Option<&'static MessageSchema> {
        schema::find_by_name(name)
    }

    pub fn schema(&self) -> &'static MessageSchema {
        self.schema
    }

    pub fn id(&self) -> MessageId {
        self.schema.id
    }

    /// Blocks follow schema order; missing legacy fixed fields are zero-filled.
    pub fn blocks(&self) -> &[BlockValues] {
        &self.blocks
    }

    /// Reports whether one schema field was decoded as present rather than compatibility zero-fill.
    ///
    /// Block, instance, and field indexes use the schema's declared order. Authored messages
    /// created by [`Self::new`] report all existing fields as supplied. Returns `None` for an
    /// invalid index.
    pub fn field_was_supplied(&self, block_index: usize, instance_index: usize, field_index: usize) -> Option<bool> {
        let block_schema = self.schema.blocks.get(block_index)?;
        self.blocks.get(block_index)?.instances.get(instance_index)?.fields.get(field_index)?;
        if self.encoded_schema_len.is_none() {
            return Some(true);
        }

        let mut offset = 0usize;
        for (index, (schema, values)) in self.schema.blocks.iter().zip(&self.blocks).enumerate() {
            if index == block_index {
                offset = offset.checked_add(instance_index.checked_mul(block_schema.fields.len())?)?.checked_add(field_index)?;
                return self.field_presence.get(offset).map(|present| *present != 0);
            }
            offset = offset.checked_add(values.instances.len().checked_mul(schema.fields.len())?)?;
        }
        None
    }

    /// Bytes after the fields known to this template version.
    pub fn trailing_bytes(&self) -> &[u8] {
        &self.trailing_bytes
    }

    pub(crate) fn retained_capacity_bytes(&self) -> usize {
        let mut bytes = self.blocks.capacity().saturating_mul(size_of::<BlockValues>());
        for block in &self.blocks {
            bytes = bytes.saturating_add(block.instances.capacity().saturating_mul(size_of::<BlockInstance>()));
            for instance in &block.instances {
                bytes = bytes.saturating_add(instance.fields.capacity().saturating_mul(size_of::<FieldValue>()));
                for field in &instance.fields {
                    if let FieldValue::Bytes(value) = field {
                        bytes = bytes.saturating_add(value.capacity());
                    }
                }
            }
        }
        bytes.saturating_add(self.field_presence.capacity()).saturating_add(self.trailing_bytes.capacity())
    }

    /// Preserve bytes appended by a newer template version.
    pub fn with_trailing_bytes(mut self, bytes: Vec<u8>) -> Self {
        self.trailing_bytes = bytes;
        self
    }

    pub fn requires_trusted_circuit(&self) -> bool {
        self.schema.trust == schema::MessageTrust::Trusted
    }

    pub(crate) fn encode_payload(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        self.validate()?;
        let start = writer.len();

        for (block_schema, block_values) in self.schema.blocks.iter().zip(&self.blocks) {
            if matches!(block_schema.mode, BlockMode::Variable) {
                let count = u8::try_from(block_values.instances.len()).map_err(|_| LLUdpError::InvalidSchemaMessage)?;
                writer.u8(count);
            }

            for instance in &block_values.instances {
                for (field_schema, value) in block_schema.fields.iter().zip(&instance.fields) {
                    encode_field(writer, field_schema, value)?;
                }
            }
        }
        if let Some(len) = self.encoded_schema_len {
            writer.truncate(start.saturating_add(len));
        }
        writer.bytes(&self.trailing_bytes);
        Ok(())
    }

    pub(crate) fn decode(schema: &'static MessageSchema, payload: &[u8]) -> Result<Self, LLUdpError> {
        let (_, field_count) = allocation_requirements(schema, payload)?;
        let mut reader = Reader::new(payload);
        let mut blocks = Vec::with_capacity(schema.blocks.len());
        let mut field_presence = Vec::with_capacity(field_count);
        let mut encoded_schema_len = None;

        for (block_index, block_schema) in schema.blocks.iter().enumerate() {
            let missing_variable_count = matches!(block_schema.mode, BlockMode::Variable) && reader.is_empty();
            let block_is_final = block_index + 1 == schema.blocks.len();
            if missing_variable_count && block_is_final {
                encoded_schema_len.get_or_insert_with(|| reader.position());
                blocks.push(BlockValues::new(Vec::new()));
                if block_is_final {
                    break;
                }
                continue;
            }

            let count = match block_schema.mode {
                BlockMode::Single => 1,
                BlockMode::Multiple(count) => usize::try_from(count).map_err(|_| LLUdpError::InvalidSchemaMessage)?,
                BlockMode::Variable => usize::from(reader.u8()?),
            };

            let mut instances = Vec::with_capacity(count);
            for _ in 0..count {
                let mut fields = Vec::with_capacity(block_schema.fields.len());
                for field_schema in block_schema.fields {
                    let field_start = reader.position();
                    let (value, present) = decode_field(&mut reader, field_schema)?;
                    fields.push(value);
                    field_presence.push(u8::from(present));
                    if !present {
                        encoded_schema_len.get_or_insert(field_start);
                    }
                }
                instances.push(BlockInstance::new(fields));
            }
            blocks.push(BlockValues::new(instances));
        }

        let encoded_schema_len = Some(encoded_schema_len.unwrap_or_else(|| reader.position()));
        Ok(Self { schema, blocks, encoded_schema_len, field_presence, trailing_bytes: reader.remaining().to_vec() })
    }

    pub(crate) fn allocation_bound(schema: &'static MessageSchema, payload: &[u8]) -> Result<usize, LLUdpError> {
        allocation_requirements(schema, payload).map(|(bytes, _)| bytes)
    }

    fn same_field_presence(&self, other: &Self) -> bool {
        for (block_index, (schema, block)) in self.schema.blocks.iter().zip(&self.blocks).enumerate() {
            for instance_index in 0..block.instances.len() {
                for field_index in 0..schema.fields.len() {
                    if self.field_was_supplied(block_index, instance_index, field_index)
                        != other.field_was_supplied(block_index, instance_index, field_index)
                    {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn validate(&self) -> Result<(), LLUdpError> {
        if self.blocks.len() != self.schema.blocks.len() {
            return Err(LLUdpError::InvalidSchemaMessage);
        }

        for (block_schema, block_values) in self.schema.blocks.iter().zip(&self.blocks) {
            let expected_count = match block_schema.mode {
                BlockMode::Single => Some(1),
                BlockMode::Multiple(count) => Some(usize::try_from(count).map_err(|_| LLUdpError::InvalidSchemaMessage)?),
                BlockMode::Variable if block_values.instances.len() <= usize::from(u8::MAX) => None,
                BlockMode::Variable => return Err(LLUdpError::InvalidSchemaMessage),
            };
            if expected_count.is_some_and(|count| count != block_values.instances.len()) {
                return Err(LLUdpError::InvalidSchemaMessage);
            }

            for instance in &block_values.instances {
                if instance.fields.len() != block_schema.fields.len() {
                    return Err(LLUdpError::InvalidSchemaMessage);
                }
                for (field_schema, value) in block_schema.fields.iter().zip(&instance.fields) {
                    validate_field(field_schema, value)?;
                }
            }
        }
        Ok(())
    }
}

fn allocation_requirements(schema: &'static MessageSchema, payload: &[u8]) -> Result<(usize, usize), LLUdpError> {
    let mut reader = Reader::new(payload);
    let mut bytes = checked_product(schema.blocks.len(), size_of::<BlockValues>())?;
    let mut field_count = 0usize;

    for (block_index, block_schema) in schema.blocks.iter().enumerate() {
        let block_is_final = block_index + 1 == schema.blocks.len();
        let missing_final_count = matches!(block_schema.mode, BlockMode::Variable) && reader.is_empty() && block_is_final;
        let count = if missing_final_count {
            0
        } else {
            match block_schema.mode {
                BlockMode::Single => 1,
                BlockMode::Multiple(count) => usize::try_from(count).map_err(|_| LLUdpError::AllocationBoundOverflow)?,
                BlockMode::Variable => usize::from(reader.u8()?),
            }
        };

        bytes = checked_add(bytes, checked_product(count, size_of::<BlockInstance>())?)?;
        let block_field_count = checked_product(count, block_schema.fields.len())?;
        bytes = checked_add(bytes, checked_product(block_field_count, size_of::<FieldValue>())?)?;
        field_count = checked_add(field_count, block_field_count)?;

        for _ in 0..count {
            for field_schema in block_schema.fields {
                match field_schema.kind {
                    FieldKind::Variable(prefix) => {
                        let length = match prefix {
                            1 => usize::from(reader.u8()?),
                            2 => usize::from(reader.u16()?),
                            _ => return Err(LLUdpError::InvalidSchemaMessage),
                        };
                        reader.take(length)?;
                        bytes = checked_add(bytes, length)?;
                    }
                    kind => {
                        let Some(size) = fixed_field_size(kind)? else {
                            return Err(LLUdpError::InvalidSchemaMessage);
                        };
                        if reader.remaining().len() < size {
                            reader.skip_remaining();
                            if let FieldKind::Fixed(_) = kind {
                                bytes = checked_add(bytes, size)?;
                            }
                        } else {
                            reader.take(size)?;
                            if let FieldKind::Fixed(_) = kind {
                                bytes = checked_add(bytes, size)?;
                            }
                        }
                    }
                }
            }
        }
    }

    bytes = checked_add(bytes, field_count)?;
    Ok((checked_add(bytes, reader.remaining().len())?, field_count))
}

fn checked_add(left: usize, right: usize) -> Result<usize, LLUdpError> {
    left.checked_add(right).ok_or(LLUdpError::AllocationBoundOverflow)
}

fn checked_product(left: usize, right: usize) -> Result<usize, LLUdpError> {
    left.checked_mul(right).ok_or(LLUdpError::AllocationBoundOverflow)
}

fn validate_field(schema: &FieldSchema, value: &FieldValue) -> Result<(), LLUdpError> {
    let valid = match (schema.kind, value) {
        (FieldKind::Bool, FieldValue::Bool(_))
        | (FieldKind::F32, FieldValue::F32(_))
        | (FieldKind::F64, FieldValue::F64(_))
        | (FieldKind::IpAddr, FieldValue::IpAddr(_))
        | (FieldKind::IpPort, FieldValue::IpPort(_))
        | (FieldKind::Null, FieldValue::Null)
        | (FieldKind::Quaternion, FieldValue::Quaternion(_))
        | (FieldKind::UUID, FieldValue::UUID(_))
        | (FieldKind::Vector3, FieldValue::Vector3(_))
        | (FieldKind::Vector3d, FieldValue::Vector3d(_))
        | (FieldKind::Vector4, FieldValue::Vector4(_))
        | (FieldKind::S8, FieldValue::S8(_))
        | (FieldKind::S16, FieldValue::S16(_))
        | (FieldKind::S32, FieldValue::S32(_))
        | (FieldKind::S64, FieldValue::S64(_))
        | (FieldKind::U8, FieldValue::U8(_))
        | (FieldKind::U16, FieldValue::U16(_))
        | (FieldKind::U32, FieldValue::U32(_))
        | (FieldKind::U64, FieldValue::U64(_)) => true,
        (FieldKind::Fixed(size), FieldValue::Bytes(bytes)) => usize::try_from(size).ok() == Some(bytes.len()),
        (FieldKind::Variable(prefix), FieldValue::Bytes(bytes)) => match prefix {
            1 => u8::try_from(bytes.len()).is_ok(),
            2 => u16::try_from(bytes.len()).is_ok(),
            _ => false,
        },
        _ => false,
    };

    if valid { Ok(()) } else { Err(LLUdpError::InvalidSchemaMessage) }
}

fn encode_field(writer: &mut Writer<'_>, schema: &FieldSchema, value: &FieldValue) -> Result<(), LLUdpError> {
    validate_field(schema, value)?;
    match (schema.kind, value) {
        (FieldKind::Bool, FieldValue::Bool(value)) => writer.u8(u8::from(*value)),
        (FieldKind::F32, FieldValue::F32(value)) => writer.f32(*value),
        (FieldKind::F64, FieldValue::F64(value)) => writer.f64(*value),
        (FieldKind::IpAddr, FieldValue::IpAddr(value)) => writer.bytes(value),
        (FieldKind::IpPort, FieldValue::IpPort(value)) => writer.bytes(value),
        (FieldKind::Null, FieldValue::Null) => {}
        (FieldKind::Quaternion, FieldValue::Quaternion(value)) => writer.quaternion_xyz(*value)?,
        (FieldKind::UUID, FieldValue::UUID(value)) => writer.uuid(*value),
        (FieldKind::Vector3, FieldValue::Vector3(value)) => writer.vector3(*value),
        (FieldKind::Vector3d, FieldValue::Vector3d(value)) => writer.vector3d(*value),
        (FieldKind::Vector4, FieldValue::Vector4(value)) => {
            writer.f32(value.x);
            writer.f32(value.y);
            writer.f32(value.z);
            writer.f32(value.w);
        }
        (FieldKind::S8, FieldValue::S8(value)) => writer.i8(*value),
        (FieldKind::S16, FieldValue::S16(value)) => writer.i16(*value),
        (FieldKind::S32, FieldValue::S32(value)) => writer.i32(*value),
        (FieldKind::S64, FieldValue::S64(value)) => writer.i64(*value),
        (FieldKind::U8, FieldValue::U8(value)) => writer.u8(*value),
        (FieldKind::U16, FieldValue::U16(value)) => writer.u16(*value),
        (FieldKind::U32, FieldValue::U32(value)) => writer.u32(*value),
        (FieldKind::U64, FieldValue::U64(value)) => writer.u64(*value),
        (FieldKind::Fixed(_), FieldValue::Bytes(value)) => writer.bytes(value),
        (FieldKind::Variable(prefix), FieldValue::Bytes(value)) => {
            match prefix {
                1 => writer.u8(u8::try_from(value.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?),
                2 => writer.u16(u16::try_from(value.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?),
                _ => return Err(LLUdpError::InvalidSchemaMessage),
            }
            writer.bytes(value);
        }
        _ => return Err(LLUdpError::InvalidSchemaMessage),
    }
    Ok(())
}

fn decode_field(reader: &mut Reader<'_>, schema: &FieldSchema) -> Result<(FieldValue, bool), LLUdpError> {
    let incomplete_fixed_field = fixed_field_size(schema.kind)?.is_some_and(|size| reader.remaining().len() < size);
    if incomplete_fixed_field {
        reader.skip_remaining();
        return Ok((zero_field(schema.kind)?, false));
    }

    let value = match schema.kind {
        FieldKind::Bool => FieldValue::Bool(reader.u8()? != 0),
        FieldKind::F32 => FieldValue::F32(reader.f32()?),
        FieldKind::F64 => FieldValue::F64(reader.f64()?),
        FieldKind::IpAddr => FieldValue::IpAddr(reader.take(4)?.try_into().map_err(|_| LLUdpError::TruncatedPacket)?),
        FieldKind::IpPort => FieldValue::IpPort(reader.take(2)?.try_into().map_err(|_| LLUdpError::TruncatedPacket)?),
        FieldKind::Null => FieldValue::Null,
        FieldKind::Quaternion => FieldValue::Quaternion(reader.quaternion()?),
        FieldKind::UUID => FieldValue::UUID(reader.uuid()?),
        FieldKind::Vector3 => FieldValue::Vector3(reader.vector3()?),
        FieldKind::Vector3d => FieldValue::Vector3d(reader.vector3d()?),
        FieldKind::Vector4 => FieldValue::Vector4(Vec4::new(reader.f32()?, reader.f32()?, reader.f32()?, reader.f32()?)),
        FieldKind::S8 => FieldValue::S8(reader.i8()?),
        FieldKind::S16 => FieldValue::S16(reader.i16()?),
        FieldKind::S32 => FieldValue::S32(reader.i32()?),
        FieldKind::S64 => FieldValue::S64(reader.i64()?),
        FieldKind::U8 => FieldValue::U8(reader.u8()?),
        FieldKind::U16 => FieldValue::U16(reader.u16()?),
        FieldKind::U32 => FieldValue::U32(reader.u32()?),
        FieldKind::U64 => FieldValue::U64(reader.u64()?),
        FieldKind::Fixed(size) => {
            let size = usize::try_from(size).map_err(|_| LLUdpError::InvalidSchemaMessage)?;
            FieldValue::Bytes(reader.take(size)?.to_vec())
        }
        FieldKind::Variable(prefix) => {
            let prefix_size = match prefix {
                1 => 1,
                2 => 2,
                _ => return Err(LLUdpError::InvalidSchemaMessage),
            };
            if reader.remaining().len() < prefix_size {
                return Err(LLUdpError::TruncatedPacket);
            }
            let len = match prefix {
                1 => usize::from(reader.u8()?),
                2 => usize::from(reader.u16()?),
                _ => return Err(LLUdpError::InvalidSchemaMessage),
            };
            FieldValue::Bytes(reader.take(len)?.to_vec())
        }
    };
    Ok((value, true))
}

fn fixed_field_size(kind: FieldKind) -> Result<Option<usize>, LLUdpError> {
    let size = match kind {
        FieldKind::Bool | FieldKind::S8 | FieldKind::U8 => 1,
        FieldKind::IpPort | FieldKind::S16 | FieldKind::U16 => 2,
        FieldKind::F32 | FieldKind::IpAddr | FieldKind::S32 | FieldKind::U32 => 4,
        FieldKind::F64 | FieldKind::S64 | FieldKind::U64 => 8,
        FieldKind::Quaternion | FieldKind::Vector3 => 12,
        FieldKind::UUID => 16,
        FieldKind::Vector3d => 24,
        FieldKind::Vector4 => 16,
        FieldKind::Null => 0,
        FieldKind::Fixed(size) => usize::try_from(size).map_err(|_| LLUdpError::InvalidSchemaMessage)?,
        FieldKind::Variable(_) => return Ok(None),
    };
    Ok(Some(size))
}

fn zero_field(kind: FieldKind) -> Result<FieldValue, LLUdpError> {
    Ok(match kind {
        FieldKind::Bool => FieldValue::Bool(false),
        FieldKind::F32 => FieldValue::F32(0.0),
        FieldKind::F64 => FieldValue::F64(0.0),
        FieldKind::IpAddr => FieldValue::IpAddr([0; 4]),
        FieldKind::IpPort => FieldValue::IpPort([0; 2]),
        FieldKind::Null => FieldValue::Null,
        FieldKind::Quaternion => FieldValue::Quaternion(Quat::IDENTITY),
        FieldKind::UUID => FieldValue::UUID(UUID::from_bytes([0; 16])),
        FieldKind::Vector3 => FieldValue::Vector3(Vec3::ZERO),
        FieldKind::Vector3d => FieldValue::Vector3d(DVec3::ZERO),
        FieldKind::Vector4 => FieldValue::Vector4(Vec4::ZERO),
        FieldKind::S8 => FieldValue::S8(0),
        FieldKind::S16 => FieldValue::S16(0),
        FieldKind::S32 => FieldValue::S32(0),
        FieldKind::S64 => FieldValue::S64(0),
        FieldKind::U8 => FieldValue::U8(0),
        FieldKind::U16 => FieldValue::U16(0),
        FieldKind::U32 => FieldValue::U32(0),
        FieldKind::U64 => FieldValue::U64(0),
        FieldKind::Fixed(size) => FieldValue::Bytes(vec![0; usize::try_from(size).map_err(|_| LLUdpError::InvalidSchemaMessage)?]),
        FieldKind::Variable(_) => return Err(LLUdpError::InvalidSchemaMessage),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{BlockSchema, MessageEncoding, MessageTrust};
    use proptest::prelude::*;

    static PREFLIGHT_FIXED_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Data", kind: FieldKind::Fixed(32) }];
    static PREFLIGHT_FIXED_BLOCKS: &[BlockSchema] =
        &[BlockSchema { name: "Data", mode: BlockMode::Variable, fields: PREFLIGHT_FIXED_FIELDS }];
    static PREFLIGHT_FIXED_SCHEMA: MessageSchema = MessageSchema {
        name: "RepeatedFixedPreflight",
        id: MessageId::high(0x76),
        trust: MessageTrust::NotTrusted,
        encoding: MessageEncoding::Unencoded,
        flags: &[],
        blocks: PREFLIGHT_FIXED_BLOCKS,
    };
    static PREFLIGHT_VARIABLE_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Data", kind: FieldKind::Variable(2) }];
    static PREFLIGHT_VARIABLE_BLOCKS: &[BlockSchema] =
        &[BlockSchema { name: "Data", mode: BlockMode::Single, fields: PREFLIGHT_VARIABLE_FIELDS }];
    static PREFLIGHT_VARIABLE_SCHEMA: MessageSchema = MessageSchema {
        name: "VariablePreflight",
        id: MessageId::high(0x77),
        trust: MessageTrust::NotTrusted,
        encoding: MessageEncoding::Unencoded,
        flags: &[],
        blocks: PREFLIGHT_VARIABLE_BLOCKS,
    };

    static ALL_FIELD_SCHEMAS: &[FieldSchema] = &[
        FieldSchema { name: "Bool", kind: FieldKind::Bool },
        FieldSchema { name: "F32", kind: FieldKind::F32 },
        FieldSchema { name: "F64", kind: FieldKind::F64 },
        FieldSchema { name: "IpAddr", kind: FieldKind::IpAddr },
        FieldSchema { name: "IpPort", kind: FieldKind::IpPort },
        FieldSchema { name: "Null", kind: FieldKind::Null },
        FieldSchema { name: "Quaternion", kind: FieldKind::Quaternion },
        FieldSchema { name: "Uuid", kind: FieldKind::UUID },
        FieldSchema { name: "Vector3", kind: FieldKind::Vector3 },
        FieldSchema { name: "Vector3d", kind: FieldKind::Vector3d },
        FieldSchema { name: "Vector4", kind: FieldKind::Vector4 },
        FieldSchema { name: "S8", kind: FieldKind::S8 },
        FieldSchema { name: "S16", kind: FieldKind::S16 },
        FieldSchema { name: "S32", kind: FieldKind::S32 },
        FieldSchema { name: "S64", kind: FieldKind::S64 },
        FieldSchema { name: "U8", kind: FieldKind::U8 },
        FieldSchema { name: "U16", kind: FieldKind::U16 },
        FieldSchema { name: "U32", kind: FieldKind::U32 },
        FieldSchema { name: "U64", kind: FieldKind::U64 },
        FieldSchema { name: "Fixed", kind: FieldKind::Fixed(4) },
        FieldSchema { name: "Variable1", kind: FieldKind::Variable(1) },
        FieldSchema { name: "Variable2", kind: FieldKind::Variable(2) },
    ];
    static ALL_FIELD_BLOCKS: &[BlockSchema] = &[BlockSchema { name: "Data", mode: BlockMode::Single, fields: ALL_FIELD_SCHEMAS }];
    static ALL_FIELDS_SCHEMA: MessageSchema = MessageSchema {
        name: "AllFieldKinds",
        id: MessageId::high(0x70),
        trust: MessageTrust::NotTrusted,
        encoding: MessageEncoding::Unencoded,
        flags: &[],
        blocks: ALL_FIELD_BLOCKS,
    };

    static BLOCK_MODE_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Value", kind: FieldKind::U8 }];
    static MULTIPLE_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Value", kind: FieldKind::U16 }];
    static VARIABLE_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Value", kind: FieldKind::U32 }];
    static BLOCK_MODE_SCHEMAS: &[BlockSchema] = &[
        BlockSchema { name: "Single", mode: BlockMode::Single, fields: BLOCK_MODE_FIELDS },
        BlockSchema { name: "Multiple", mode: BlockMode::Multiple(2), fields: MULTIPLE_FIELDS },
        BlockSchema { name: "Variable", mode: BlockMode::Variable, fields: VARIABLE_FIELDS },
    ];
    static BLOCK_MODES_SCHEMA: MessageSchema = MessageSchema {
        name: "BlockModes",
        id: MessageId::high(0x71),
        trust: MessageTrust::NotTrusted,
        encoding: MessageEncoding::Unencoded,
        flags: &[],
        blocks: BLOCK_MODE_SCHEMAS,
    };

    static COMPAT_BASE_FIELDS: &[FieldSchema] =
        &[FieldSchema { name: "Base", kind: FieldKind::U8 }, FieldSchema { name: "Added", kind: FieldKind::U16 }];
    static COMPAT_LATER_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Later", kind: FieldKind::U32 }];
    static COMPAT_BLOCKS: &[BlockSchema] = &[
        BlockSchema { name: "Base", mode: BlockMode::Single, fields: COMPAT_BASE_FIELDS },
        BlockSchema { name: "Later", mode: BlockMode::Multiple(2), fields: COMPAT_LATER_FIELDS },
    ];
    static COMPAT_SCHEMA: MessageSchema = MessageSchema {
        name: "AppendedBlocks",
        id: MessageId::high(0x73),
        trust: MessageTrust::NotTrusted,
        encoding: MessageEncoding::Unencoded,
        flags: &[],
        blocks: COMPAT_BLOCKS,
    };
    static VARIABLE_SUFFIX_BLOCKS: &[BlockSchema] = &[
        BlockSchema { name: "Base", mode: BlockMode::Single, fields: BLOCK_MODE_FIELDS },
        BlockSchema { name: "Optional", mode: BlockMode::Variable, fields: VARIABLE_FIELDS },
        BlockSchema { name: "Added", mode: BlockMode::Single, fields: BLOCK_MODE_FIELDS },
    ];
    static VARIABLE_SUFFIX_SCHEMA: MessageSchema = MessageSchema {
        name: "VariableSuffix",
        id: MessageId::high(0x74),
        trust: MessageTrust::NotTrusted,
        encoding: MessageEncoding::Unencoded,
        flags: &[],
        blocks: VARIABLE_SUFFIX_BLOCKS,
    };

    fn all_field_values() -> Vec<FieldValue> {
        vec![
            FieldValue::Bool(true),
            FieldValue::F32(1.0),
            FieldValue::F64(-1.0),
            FieldValue::IpAddr([1, 2, 3, 4]),
            FieldValue::IpPort([0x1f, 0x90]),
            FieldValue::Null,
            FieldValue::Quaternion(Quat::from_xyzw(1.0, 0.0, 0.0, 0.0)),
            FieldValue::UUID(UUID::from_bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15])),
            FieldValue::Vector3(Vec3::new(1.0, -2.0, 0.5)),
            FieldValue::Vector3d(DVec3::new(1.0, -2.0, 0.5)),
            FieldValue::Vector4(Vec4::new(1.0, -2.0, 0.5, 4.0)),
            FieldValue::S8(-2),
            FieldValue::S16(-3),
            FieldValue::S32(-4),
            FieldValue::S64(-5),
            FieldValue::U8(5),
            FieldValue::U16(0x0607),
            FieldValue::U32(0x0809_0a0b),
            FieldValue::U64(0x0809_0a0b_0c0d_0e0f),
            FieldValue::Bytes(vec![0xde, 0xad, 0xbe, 0xef]),
            FieldValue::Bytes(vec![0xaa, 0xbb]),
            FieldValue::Bytes(vec![0xcc, 0xdd, 0xee]),
        ]
    }

    #[test]
    fn field_kinds_match_bytes() {
        let message = SchemaMessage::new(&ALL_FIELDS_SCHEMA, vec![BlockValues::single(all_field_values())]).expect("valid message");
        let mut actual = Vec::new();
        message.encode_payload(&mut Writer::new(&mut actual)).expect("encode all field kinds");

        let expected = vec![
            0x01, // BOOL
            0x00, 0x00, 0x80, 0x3f, // F32
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf0, 0xbf, // F64
            0x01, 0x02, 0x03, 0x04, // IPADDR
            0x1f, 0x90, // IPPORT
            // NULL: no bytes
            0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Quaternion
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, // UUID
            0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0xc0, 0x00, 0x00, 0x00, 0x3f, // Vector3
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf0, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0xe0, 0x3f, // Vector3d
            0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0xc0, 0x00, 0x00, 0x00, 0x3f, 0x00, 0x00, 0x80, 0x40, // Vector4
            0xfe, // S8
            0xfd, 0xff, // S16
            0xfc, 0xff, 0xff, 0xff, // S32
            0xfb, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // S64
            0x05, // U8
            0x07, 0x06, // U16
            0x0b, 0x0a, 0x09, 0x08, // U32
            0x0f, 0x0e, 0x0d, 0x0c, 0x0b, 0x0a, 0x09, 0x08, // U64
            0xde, 0xad, 0xbe, 0xef, // Fixed(4)
            0x02, 0xaa, 0xbb, // Variable(1)
            0x03, 0x00, 0xcc, 0xdd, 0xee, // Variable(2), little-endian U16 length
        ];
        assert_eq!(actual, expected);
        assert_eq!(
            SchemaMessage::decode(&ALL_FIELDS_SCHEMA, &expected).expect("decode all fields").blocks[0].instances[0].fields,
            all_field_values()
        );
    }

    #[test]
    fn variable_2_little_endian() {
        use std::iter::repeat_n;

        let payload = vec![0x02, 0x01].into_iter().chain(repeat_n(0xaa, 258)).collect::<Vec<_>>();
        let schema = FieldSchema { name: "Data", kind: FieldKind::Variable(2) };
        let (decoded, complete) = decode_field(&mut Reader::new(&payload), &schema).expect("decode 258-byte Variable(2)");
        assert!(complete);
        assert_eq!(decoded, FieldValue::Bytes(vec![0xaa; 258]));
    }

    #[test]
    fn block_modes_match_bytes() {
        let message = SchemaMessage::new(
            &BLOCK_MODES_SCHEMA,
            vec![
                BlockValues::single(vec![FieldValue::U8(0x11)]),
                BlockValues::new(vec![
                    BlockInstance::new(vec![FieldValue::U16(0x2222)]),
                    BlockInstance::new(vec![FieldValue::U16(0x3333)]),
                ]),
                BlockValues::new(vec![
                    BlockInstance::new(vec![FieldValue::U32(0x4444_4444)]),
                    BlockInstance::new(vec![FieldValue::U32(0x5555_5555)]),
                ]),
            ],
        )
        .expect("valid block counts");
        let mut actual = Vec::new();
        message.encode_payload(&mut Writer::new(&mut actual)).expect("encode block modes");
        assert_eq!(actual, [0x11, 0x22, 0x22, 0x33, 0x33, 0x02, 0x44, 0x44, 0x44, 0x44, 0x55, 0x55, 0x55, 0x55]);
    }

    #[test]
    fn empty_final_block_count() {
        let payload = [0x11, 0x22, 0x22, 0x33, 0x33];
        let decoded = SchemaMessage::decode(&BLOCK_MODES_SCHEMA, &payload).expect("legacy empty final block");
        assert!(decoded.blocks[2].instances.is_empty());
        let mut encoded = Vec::new();
        decoded.encode_payload(&mut Writer::new(&mut encoded)).expect("re-encode legacy empty final block");
        assert_eq!(encoded, payload);

        static NONTERMINAL_BLOCKS: &[BlockSchema] = &[
            BlockSchema { name: "Variable", mode: BlockMode::Variable, fields: VARIABLE_FIELDS },
            BlockSchema { name: "Single", mode: BlockMode::Single, fields: BLOCK_MODE_FIELDS },
        ];
        static NONTERMINAL_SCHEMA: MessageSchema = MessageSchema {
            name: "NonterminalVariable",
            id: MessageId::high(0x72),
            trust: MessageTrust::NotTrusted,
            encoding: MessageEncoding::Unencoded,
            flags: &[],
            blocks: NONTERMINAL_BLOCKS,
        };
        assert_eq!(SchemaMessage::decode(&NONTERMINAL_SCHEMA, &[]).err(), Some(LLUdpError::TruncatedPacket));
    }

    #[test]
    fn missing_fields_zero_fill() {
        let payload = [0x11];
        let decoded = SchemaMessage::decode(&COMPAT_SCHEMA, &payload).expect("decode older message");

        assert_eq!(decoded.blocks[0].instances[0].fields, [FieldValue::U8(0x11), FieldValue::U16(0)]);
        assert_eq!(decoded.blocks[1].instances[0].fields, [FieldValue::U32(0)]);
        assert_eq!(decoded.blocks[1].instances[1].fields, [FieldValue::U32(0)]);

        let mut encoded = Vec::new();
        decoded.encode_payload(&mut Writer::new(&mut encoded)).expect("re-encode older message");
        assert_eq!(encoded, payload);
    }

    #[test]
    fn explicit_zero_differs_from_compatibility_fill() {
        let supplied = SchemaMessage::decode(&COMPAT_SCHEMA, &[0x11, 0, 0]).expect("decode explicitly supplied zero");
        let omitted = SchemaMessage::decode(&COMPAT_SCHEMA, &[0x11]).expect("decode omitted compatibility fields");

        assert_eq!(supplied.blocks[0].instances[0].fields[1], FieldValue::U16(0));
        assert_eq!(omitted.blocks[0].instances[0].fields[1], FieldValue::U16(0));
        assert_eq!(supplied.field_was_supplied(0, 0, 1), Some(true));
        assert_eq!(omitted.field_was_supplied(0, 0, 1), Some(false));
        assert_eq!(omitted.field_was_supplied(1, 0, 0), Some(false));
        assert_eq!(omitted.field_was_supplied(4, 0, 0), None);
        assert_ne!(supplied, omitted, "wire presence is part of the schema message's observable value");
    }

    #[test]
    fn counts_repeated_padding() {
        static FIXED_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Data", kind: FieldKind::Fixed(32) }];
        static BLOCKS: &[BlockSchema] = &[BlockSchema { name: "Data", mode: BlockMode::Variable, fields: FIXED_FIELDS }];
        static SCHEMA: MessageSchema = MessageSchema {
            name: "RepeatedFixedPadding",
            id: MessageId::high(0x73),
            trust: MessageTrust::NotTrusted,
            encoding: MessageEncoding::Unencoded,
            flags: &[],
            blocks: BLOCKS,
        };

        let decoded = SchemaMessage::decode(&SCHEMA, &[u8::MAX]).expect("decode omitted repeated fields");
        let expected = size_of::<BlockValues>()
            + usize::from(u8::MAX) * size_of::<BlockInstance>()
            + usize::from(u8::MAX) * size_of::<FieldValue>()
            + usize::from(u8::MAX) * 32;

        assert_eq!(decoded.blocks[0].instances.len(), usize::from(u8::MAX));
        assert!(decoded.retained_capacity_bytes() >= expected);
    }

    #[test]
    fn preflights_padding_and_truncated_variables() {
        static FIXED_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Data", kind: FieldKind::Fixed(32) }];
        static FIXED_BLOCKS: &[BlockSchema] = &[BlockSchema { name: "Data", mode: BlockMode::Variable, fields: FIXED_FIELDS }];
        static FIXED_SCHEMA: MessageSchema = MessageSchema {
            name: "RepeatedFixedPaddingBound",
            id: MessageId::high(0x74),
            trust: MessageTrust::NotTrusted,
            encoding: MessageEncoding::Unencoded,
            flags: &[],
            blocks: FIXED_BLOCKS,
        };
        static VARIABLE_FIELDS: &[FieldSchema] = &[FieldSchema { name: "Data", kind: FieldKind::Variable(1) }];
        static VARIABLE_BLOCKS: &[BlockSchema] = &[BlockSchema { name: "Data", mode: BlockMode::Single, fields: VARIABLE_FIELDS }];
        static VARIABLE_SCHEMA: MessageSchema = MessageSchema {
            name: "TruncatedVariableBound",
            id: MessageId::high(0x75),
            trust: MessageTrust::NotTrusted,
            encoding: MessageEncoding::Unencoded,
            flags: &[],
            blocks: VARIABLE_BLOCKS,
        };

        let payload = [u8::MAX];
        let bound = SchemaMessage::allocation_bound(&FIXED_SCHEMA, &payload).expect("bound repeated compatibility padding");
        let decoded = SchemaMessage::decode(&FIXED_SCHEMA, &payload).expect("decode repeated compatibility padding");
        assert!(bound >= decoded.retained_capacity_bytes());

        assert_eq!(SchemaMessage::allocation_bound(&VARIABLE_SCHEMA, &[2, 1]), Err(LLUdpError::TruncatedPacket));
    }

    #[test]
    fn rejects_allocation_bound_overflow() {
        assert_eq!(checked_add(usize::MAX, 1), Err(LLUdpError::AllocationBoundOverflow));
        assert_eq!(checked_product(usize::MAX, 2), Err(LLUdpError::AllocationBoundOverflow));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn repeated_fixed_field_preflight_covers_schema_expansion(count in any::<u8>(), available in 0usize..=8192) {
            let mut payload = Vec::with_capacity(1 + available);
            payload.push(count);
            payload.resize(1 + available, 0x5a);

            let bound = SchemaMessage::allocation_bound(&PREFLIGHT_FIXED_SCHEMA, &payload)
                .expect("checked repeated fixed-field bound");
            let decoded = SchemaMessage::decode(&PREFLIGHT_FIXED_SCHEMA, &payload)
                .expect("fixed compatibility fields zero-fill safely");

            prop_assert!(bound >= decoded.retained_capacity_bytes());
        }

        #[test]
        fn variable_field_preflight_checks_declared_lengths(declared in 0u16..=u16::MAX, available in 0usize..=1024) {
            let mut payload = declared.to_le_bytes().to_vec();
            payload.resize(2 + available, 0xa5);

            if usize::from(declared) > available {
                prop_assert_eq!(
                    SchemaMessage::allocation_bound(&PREFLIGHT_VARIABLE_SCHEMA, &payload),
                    Err(LLUdpError::TruncatedPacket)
                );
            } else {
                let bound = SchemaMessage::allocation_bound(&PREFLIGHT_VARIABLE_SCHEMA, &payload)
                    .expect("complete variable field has a checked bound");
                let decoded = SchemaMessage::decode(&PREFLIGHT_VARIABLE_SCHEMA, &payload)
                    .expect("complete variable field decodes");
                prop_assert!(bound >= decoded.retained_capacity_bytes());
            }
        }
    }

    #[test]
    fn generated_schema_bounds_fit_packet_limit() {
        for schema in schema::MESSAGES {
            let mut bytes = schema.blocks.len() * size_of::<BlockValues>() + 64 * 1024;
            for block in schema.blocks {
                let count = match block.mode {
                    BlockMode::Single => 1,
                    BlockMode::Multiple(count) => usize::try_from(count).expect("template count fits usize"),
                    BlockMode::Variable => usize::from(u8::MAX),
                };
                let fixed_bytes = block.fields.iter().fold(0usize, |total, field| match field.kind {
                    FieldKind::Fixed(size) => total.saturating_add(usize::try_from(size).expect("fixed field fits usize")),
                    _ => total,
                });
                let instance_capacity = size_of::<BlockInstance>()
                    .saturating_add(block.fields.len().saturating_mul(size_of::<FieldValue>()))
                    .saturating_add(fixed_bytes);
                bytes = bytes.saturating_add(count.saturating_mul(instance_capacity));
            }
            assert!(bytes <= crate::lludp::MAX_PACKET_RETAINED_CAPACITY_BYTES, "{} requires {bytes} bytes", schema.name);
        }
    }

    #[test]
    fn missing_middle_count_fails() {
        assert_eq!(SchemaMessage::decode(&VARIABLE_SUFFIX_SCHEMA, &[0x11]).err(), Some(LLUdpError::TruncatedPacket));
    }

    #[test]
    fn middle_variable_block() {
        let payload = [0x11, 0x01, 0x22, 0x33, 0x44, 0x55, 0x66];
        let decoded = SchemaMessage::decode(&VARIABLE_SUFFIX_SCHEMA, &payload).expect("decode middle variable block");

        assert_eq!(decoded.blocks[1].instances.len(), 1);
        assert_eq!(decoded.blocks[1].instances[0].fields, [FieldValue::U32(0x5544_3322)]);
        assert_eq!(decoded.blocks[2].instances[0].fields, [FieldValue::U8(0x66)]);

        let mut encoded = Vec::new();
        decoded.encode_payload(&mut Writer::new(&mut encoded)).expect("re-encode middle variable block");
        assert_eq!(encoded, payload);
    }

    #[test]
    fn partial_field_zero_fills() {
        let decoded = SchemaMessage::decode(&COMPAT_SCHEMA, &[0x11, 0x22]).expect("decode partial fixed field");

        assert_eq!(decoded.blocks[0].instances[0].fields, [FieldValue::U8(0x11), FieldValue::U16(0)]);
        assert_eq!(decoded.blocks[1].instances[0].fields, [FieldValue::U32(0)]);
        assert_eq!(decoded.blocks[1].instances[1].fields, [FieldValue::U32(0)]);
        assert!(decoded.trailing_bytes().is_empty());

        let mut encoded = Vec::new();
        decoded.encode_payload(&mut Writer::new(&mut encoded)).expect("re-encode partial field");
        assert_eq!(encoded, [0x11]);
    }

    #[test]
    fn variable_truncation_fails() {
        let one_byte_prefix = FieldSchema { name: "Data", kind: FieldKind::Variable(1) };
        assert_eq!(decode_field(&mut Reader::new(&[0x02, 0xaa]), &one_byte_prefix).err(), Some(LLUdpError::TruncatedPacket));

        let two_byte_prefix = FieldSchema { name: "Data", kind: FieldKind::Variable(2) };
        assert_eq!(decode_field(&mut Reader::new(&[0x01]), &two_byte_prefix).err(), Some(LLUdpError::TruncatedPacket));
    }

    #[test]
    fn bad_shapes_rejected() {
        assert_eq!(SchemaMessage::new(&BLOCK_MODES_SCHEMA, Vec::new()).err(), Some(LLUdpError::InvalidSchemaMessage));
        assert_eq!(
            SchemaMessage::new(
                &BLOCK_MODES_SCHEMA,
                vec![
                    BlockValues::single(vec![FieldValue::U16(1)]),
                    BlockValues::new(vec![BlockInstance::new(vec![FieldValue::U16(1)]); 2]),
                    BlockValues::new(Vec::new()),
                ],
            )
            .err(),
            Some(LLUdpError::InvalidSchemaMessage)
        );
    }

    #[test]
    fn length_overflow_rejected() {
        let mut fields = all_field_values();
        fields[17] = FieldValue::Bytes(vec![0; 3]);
        assert_eq!(SchemaMessage::new(&ALL_FIELDS_SCHEMA, vec![BlockValues::single(fields)]).err(), Some(LLUdpError::InvalidSchemaMessage));

        let mut fields = all_field_values();
        fields[18] = FieldValue::Bytes(vec![0; 256]);
        assert_eq!(SchemaMessage::new(&ALL_FIELDS_SCHEMA, vec![BlockValues::single(fields)]).err(), Some(LLUdpError::InvalidSchemaMessage));

        let variable_instances = (0..=u8::MAX).map(|_| BlockInstance::new(vec![FieldValue::U32(1)])).collect();
        let blocks = vec![
            BlockValues::single(vec![FieldValue::U8(1)]),
            BlockValues::new(vec![BlockInstance::new(vec![FieldValue::U16(1)]), BlockInstance::new(vec![FieldValue::U16(2)])]),
            BlockValues::new(variable_instances),
        ];
        assert_eq!(SchemaMessage::new(&BLOCK_MODES_SCHEMA, blocks).err(), Some(LLUdpError::InvalidSchemaMessage));
    }

    fn sample_value(kind: FieldKind) -> FieldValue {
        match kind {
            FieldKind::Bool => FieldValue::Bool(true),
            FieldKind::F32 => FieldValue::F32(1.5),
            FieldKind::F64 => FieldValue::F64(-2.25),
            FieldKind::IpAddr => FieldValue::IpAddr([192, 0, 2, 1]),
            FieldKind::IpPort => FieldValue::IpPort([0x12, 0x34]),
            FieldKind::Null => FieldValue::Null,
            FieldKind::Quaternion => FieldValue::Quaternion(Quat::from_xyzw(1.0, 0.0, 0.0, 0.0)),
            FieldKind::UUID => FieldValue::UUID(UUID::from_bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15])),
            FieldKind::Vector3 => FieldValue::Vector3(Vec3::new(1.0, -2.0, 0.5)),
            FieldKind::Vector3d => FieldValue::Vector3d(DVec3::new(1.0, -2.0, 0.5)),
            FieldKind::Vector4 => FieldValue::Vector4(Vec4::new(1.0, -2.0, 0.5, 4.0)),
            FieldKind::S8 => FieldValue::S8(-2),
            FieldKind::S16 => FieldValue::S16(-3),
            FieldKind::S32 => FieldValue::S32(-4),
            FieldKind::S64 => FieldValue::S64(-5),
            FieldKind::U8 => FieldValue::U8(5),
            FieldKind::U16 => FieldValue::U16(0x0607),
            FieldKind::U32 => FieldValue::U32(0x0809_0a0b),
            FieldKind::U64 => FieldValue::U64(0x0809_0a0b_0c0d_0e0f),
            FieldKind::Fixed(size) => FieldValue::Bytes(vec![0x5a; size as usize]),
            FieldKind::Variable(_) => FieldValue::Bytes(vec![0xaa, 0xbb]),
        }
    }

    #[test]
    fn schemas_round_trip() {
        for schema in schema::MESSAGES {
            let blocks = schema
                .blocks
                .iter()
                .map(|block| {
                    let count = match block.mode {
                        BlockMode::Single => 1,
                        BlockMode::Multiple(count) => count as usize,
                        BlockMode::Variable => 2,
                    };
                    let instances = (0..count)
                        .map(|_| BlockInstance::new(block.fields.iter().map(|field| sample_value(field.kind)).collect()))
                        .collect();
                    BlockValues::new(instances)
                })
                .collect();
            let message = SchemaMessage::new(schema, blocks).unwrap_or_else(|error| panic!("{}: {error}", schema.name));
            let mut expected = Vec::new();
            message.encode_payload(&mut Writer::new(&mut expected)).unwrap_or_else(|error| panic!("{}: {error}", schema.name));

            let decoded = SchemaMessage::decode(schema, &expected).unwrap_or_else(|error| panic!("{}: {error}", schema.name));
            let mut actual = Vec::new();
            decoded.encode_payload(&mut Writer::new(&mut actual)).unwrap_or_else(|error| panic!("{}: {error}", schema.name));
            assert_eq!(actual, expected, "schema {}", schema.name);
        }
    }
}
