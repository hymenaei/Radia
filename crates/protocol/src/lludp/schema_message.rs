use std::mem::size_of;

use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        types::MessageId,
    },
    schema::{self, BlockMode, FieldKind, FieldSchema, MessageSchema},
};
use math::{DVec3, Quat, Vec3, Vec4};

#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    Bool(bool),
    F32(f32),
    F64(f64),

    IpAddr([u8; 4]),

    IpPort([u8; 2]),

    Null,

    Quaternion(Quat),

    Uuid(Uuid),
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

    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockInstance {
    pub fields: Vec<FieldValue>,
}

impl BlockInstance {
    pub fn new(fields: Vec<FieldValue>) -> Self {
        Self { fields }
    }
}

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

    pub fn blocks(&self) -> &[BlockValues] {
        &self.blocks
    }

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
        | (FieldKind::Uuid, FieldValue::Uuid(_))
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
        (FieldKind::Uuid, FieldValue::Uuid(value)) => writer.uuid(*value),
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
        FieldKind::Uuid => FieldValue::Uuid(reader.uuid()?),
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
        FieldKind::Uuid => 16,
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
        FieldKind::Uuid => FieldValue::Uuid(Uuid::from_bytes([0; 16])),
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
#[path = "schema_message_tests.rs"]
mod tests;
