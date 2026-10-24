use math::Vec3;
use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::AVATAR_APPEARANCE,
};

#[derive(Clone, Debug, PartialEq)]
pub struct AvatarAppearance {
    pub sender: Sender,
    pub object_data: ObjectData,
    pub visual_params: Vec<VisualParam>,
    pub appearance_data: Vec<AppearanceData>,
    pub appearance_hover: Vec<AppearanceHover>,
    pub attachment_blocks: Option<Vec<AttachmentBlock>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sender {
    pub id: Uuid,
    pub is_trial: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectData {
    pub texture_entry: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VisualParam {
    pub param_value: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppearanceData {
    pub appearance_version: u8,
    pub cof_version: i32,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppearanceHover {
    pub hover_height: Vec3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttachmentBlock {
    pub id: Uuid,
    pub attachment_point: u8,
}

impl TypedMessage for AvatarAppearance {
    const ID: MessageId = AVATAR_APPEARANCE;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let sender = Sender { id: reader.uuid()?, is_trial: reader.u8()? != 0 };
        let object_data = ObjectData { texture_entry: reader.variable2()?.to_vec() };
        let visual_count = usize::from(reader.u8()?);
        let mut visual_params = Vec::with_capacity(visual_count);
        for _ in 0..visual_count {
            visual_params.push(VisualParam { param_value: reader.u8()? });
        }
        let appearance_count = usize::from(reader.u8()?);
        let mut appearance_data = Vec::with_capacity(appearance_count);
        for _ in 0..appearance_count {
            appearance_data.push(AppearanceData { appearance_version: reader.u8()?, cof_version: reader.i32()?, flags: reader.u32()? });
        }
        let hover_count = usize::from(reader.u8()?);
        let mut appearance_hover = Vec::with_capacity(hover_count);
        for _ in 0..hover_count {
            appearance_hover.push(AppearanceHover { hover_height: reader.vector3()? });
        }
        let attachment_blocks = if reader.is_empty() {
            None
        } else {
            let count = usize::from(reader.u8()?);
            let mut blocks = Vec::with_capacity(count);
            for _ in 0..count {
                blocks.push(AttachmentBlock { id: reader.uuid()?, attachment_point: reader.u8()? });
            }
            Some(blocks)
        };
        Ok(Self { sender, object_data, visual_params, appearance_data, appearance_hover, attachment_blocks })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.sender.id);
        writer.u8(u8::from(self.sender.is_trial));
        writer.variable2(&self.object_data.texture_entry)?;
        writer.u8(u8::try_from(self.visual_params.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
        for value in &self.visual_params {
            writer.u8(value.param_value);
        }
        writer.u8(u8::try_from(self.appearance_data.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
        for value in &self.appearance_data {
            writer.u8(value.appearance_version);
            writer.i32(value.cof_version);
            writer.u32(value.flags);
        }
        writer.u8(u8::try_from(self.appearance_hover.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
        for value in &self.appearance_hover {
            writer.vector3(value.hover_height);
        }
        if let Some(blocks) = &self.attachment_blocks {
            writer.u8(u8::try_from(blocks.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
            for block in blocks {
                writer.uuid(block.id);
                writer.u8(block.attachment_point);
            }
        }
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        let mut bytes =
            crate::lludp::message::allocation::capacity_bytes(self.object_data.texture_entry.capacity(), std::mem::size_of::<u8>());
        crate::lludp::message::allocation::add_capacity(&mut bytes, self.visual_params.capacity(), std::mem::size_of::<VisualParam>());
        crate::lludp::message::allocation::add_capacity(&mut bytes, self.appearance_data.capacity(), std::mem::size_of::<AppearanceData>());
        crate::lludp::message::allocation::add_capacity(
            &mut bytes,
            self.appearance_hover.capacity(),
            std::mem::size_of::<AppearanceHover>(),
        );
        if let Some(blocks) = &self.attachment_blocks {
            crate::lludp::message::allocation::add_capacity(&mut bytes, blocks.capacity(), std::mem::size_of::<AttachmentBlock>());
        }
        bytes
    }
}
