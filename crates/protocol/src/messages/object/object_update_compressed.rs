use math::{Quat, Vec3};
use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    messages::object::{
        ExtraParameter, ObjectSound, ObjectText, RegionData, add_preflight_capacity, decode_extra_parameters, encode_extra_parameters,
        preflight_capacity, rotation_from_vector3, skip_extra_parameters,
    },
    schema::ids::OBJECT_UPDATE_COMPRESSED,
    types::ObjectShape,
};

const SPECIAL_HAS_SCRATCH_PAD: u32 = 0x001;
const SPECIAL_HAS_TREE_DATA: u32 = 0x002;
const SPECIAL_HAS_TEXT: u32 = 0x004;
const SPECIAL_HAS_LEGACY_PARTICLE_SYSTEM: u32 = 0x008;
const SPECIAL_HAS_SOUND: u32 = 0x010;
const SPECIAL_HAS_PARENT: u32 = 0x020;
const SPECIAL_HAS_TEXTURE_ANIMATION: u32 = 0x040;
const SPECIAL_HAS_ANGULAR_VELOCITY: u32 = 0x080;
const SPECIAL_HAS_NAME_VALUE: u32 = 0x100;
const SPECIAL_HAS_MEDIA_URL: u32 = 0x200;
const SPECIAL_HAS_PARTICLE_SYSTEM: u32 = 0x400;
const SPECIAL_OBJECT_FIELDS: u32 = 0x7ff;
const VOLUME_PCODE: u8 = 9;
const LEGACY_PARTICLE_SYSTEM_SIZE: usize = 86;

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectUpdateCompressed {
    pub region_data: RegionData,
    pub objects: Vec<ObjectData>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectData {
    pub update_flags: u32,
    pub local_id: u32,
    pub id: Uuid,
    pub pcode: u8,
    pub state: u8,
    pub crc: u32,
    pub material: u8,
    pub click_action: u8,
    pub scale: Vec3,
    pub position: Vec3,
    pub rotation: Quat,
    pub angular_velocity: Option<Vec3>,
    pub special_code: u32,
    pub owner_id: Uuid,
    pub parent_local_id: u32,
    pub data: Option<ObjectSpecificData>,
    pub text: Option<ObjectText>,
    pub media_url: Option<String>,
    pub legacy_particle_system: Option<Vec<u8>>,
    pub extra_parameters: Vec<ExtraParameter>,
    pub sound: Option<ObjectSound>,
    pub name_value: Option<String>,
    pub shape: Option<ObjectShape>,
    pub texture_entry: Option<Vec<u8>>,
    pub texture_animation: Option<Vec<u8>>,
    pub particle_system: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObjectSpecificData {
    Tree { species: u8 },
    ScratchPad(Vec<u8>),
}

impl TypedMessage for ObjectUpdateCompressed {
    const ID: MessageId = OBJECT_UPDATE_COMPRESSED;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::Specialized;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        self::decode(reader)
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        self.region_data.encode(writer);
        let count = u8::try_from(self.objects.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
        writer.u8(count);
        for object in &self.objects {
            writer.u32(object.update_flags);
            encode_fields(object, writer)?;
        }
        Ok(())
    }

    fn preflight_allocation_bound(payload: &[u8]) -> Result<usize, LLUdpError> {
        crate::lludp::message::allocation::with_payload(payload, preflight_allocation_bytes(payload)?)
    }

    fn retained_capacity_bytes(&self) -> usize {
        let mut bytes = crate::lludp::message::allocation::capacity_bytes(self.objects.capacity(), std::mem::size_of::<ObjectData>());
        for object in &self.objects {
            crate::lludp::message::allocation::add_object_update_compressed_capacity(&mut bytes, object);
        }
        bytes
    }
}

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<ObjectUpdateCompressed, LLUdpError> {
    let region_data = RegionData::decode(reader)?;
    let count = reader.terminal_variable_count()?;
    let mut objects = Vec::with_capacity(count);

    for _ in 0..count {
        let update_flags = reader.u32()?;
        let packed_data = reader.variable2()?;
        let mut data = Reader::new(packed_data);
        let id = data.uuid()?;
        let local_id = data.u32()?;
        let pcode = data.u8()?;
        let state = data.u8()?;
        let crc = data.u32()?;
        let material = data.u8()?;
        let click_action = data.u8()?;
        let scale = data.vector3()?;
        let position = data.vector3()?;
        let rotation = rotation_from_vector3(data.vector3()?);
        let special_code = data.u32()?;
        let owner_id = data.uuid()?;
        let angular_velocity = if special_code & SPECIAL_HAS_ANGULAR_VELOCITY != 0 { Some(data.vector3()?) } else { None };
        let parent_local_id = if special_code & SPECIAL_HAS_PARENT != 0 { data.u32()? } else { 0 };
        let object_data = if special_code & SPECIAL_HAS_TREE_DATA != 0 {
            Some(ObjectSpecificData::Tree { species: data.u8()? })
        } else if special_code & SPECIAL_HAS_SCRATCH_PAD != 0 {
            let size = usize::try_from(data.u32()?).map_err(|_| LLUdpError::InvalidMessage)?;
            let bytes = read_binary_data(&mut data)?;
            if bytes.len() > size {
                return Err(LLUdpError::InvalidMessage);
            }
            Some(ObjectSpecificData::ScratchPad(bytes.to_vec()))
        } else {
            None
        };
        let text = if special_code & SPECIAL_HAS_TEXT != 0 {
            Some(ObjectText { text: read_c_string(&mut data)?, color: data.take(4)?.try_into().expect("four bytes requested") })
        } else {
            None
        };
        let media_url = if special_code & SPECIAL_HAS_MEDIA_URL != 0 { Some(read_c_string(&mut data)?) } else { None };
        let legacy_particle_system = if special_code & SPECIAL_HAS_LEGACY_PARTICLE_SYSTEM != 0 {
            Some(data.take(LEGACY_PARTICLE_SYSTEM_SIZE)?.to_vec())
        } else {
            None
        };
        let extra_parameters = decode_extra_parameters(&mut data)?;
        let sound = if special_code & SPECIAL_HAS_SOUND != 0 {
            let sound_id = data.uuid()?;
            let gain = data.f32()?;
            let flags = data.u8()?;
            let radius = data.f32()?;
            Some(ObjectSound { sound_id, gain, flags, radius })
        } else {
            None
        };
        let name_value = if special_code & SPECIAL_HAS_NAME_VALUE != 0 { Some(read_c_string(&mut data)?) } else { None };

        let (shape, texture_entry, texture_animation, particle_system) = if pcode == VOLUME_PCODE {
            let shape = Some(ObjectShape::decode_compressed(&mut data)?);
            let texture_entry = Some(read_binary_data(&mut data)?.to_vec());
            let texture_animation =
                if special_code & SPECIAL_HAS_TEXTURE_ANIMATION != 0 { Some(read_binary_data(&mut data)?.to_vec()) } else { None };
            let particle_system =
                if special_code & SPECIAL_HAS_PARTICLE_SYSTEM != 0 { Some(read_particle_system(&mut data)?.to_vec()) } else { None };
            (shape, texture_entry, texture_animation, particle_system)
        } else {
            (None, None, None, None)
        };

        objects.push(ObjectData {
            update_flags,
            local_id,
            id,
            pcode,
            state,
            crc,
            material,
            click_action,
            scale,
            position,
            rotation,
            angular_velocity,
            special_code,
            owner_id,
            parent_local_id,
            data: object_data,
            text,
            media_url,
            legacy_particle_system,
            extra_parameters,
            sound,
            name_value,
            shape,
            texture_entry,
            texture_animation,
            particle_system,
        });
    }

    Ok(ObjectUpdateCompressed { region_data, objects })
}

pub(crate) fn preflight_allocation_bytes(payload: &[u8]) -> Result<usize, LLUdpError> {
    let mut reader = Reader::new(payload);
    RegionData::decode(&mut reader)?;
    let count = reader.terminal_variable_count()?;
    let mut allocation_bytes = preflight_capacity::<ObjectData>(count)?;

    for _ in 0..count {
        reader.take(4)?;
        let packed_data = reader.variable2()?;
        let mut data = Reader::new(packed_data);
        data.take(16)?;
        data.take(4)?;
        let pcode = data.u8()?;
        data.take(1 + 4 + 1 + 1 + 36)?;
        let special_code = data.u32()?;
        data.take(16)?;

        if special_code & SPECIAL_HAS_ANGULAR_VELOCITY != 0 {
            data.take(12)?;
        }
        if special_code & SPECIAL_HAS_PARENT != 0 {
            data.take(4)?;
        }
        if special_code & SPECIAL_HAS_TREE_DATA != 0 {
            data.take(1)?;
        } else if special_code & SPECIAL_HAS_SCRATCH_PAD != 0 {
            let declared_size = usize::try_from(data.u32()?).map_err(|_| LLUdpError::InvalidMessage)?;
            let bytes = read_binary_data(&mut data)?;
            if bytes.len() > declared_size {
                return Err(LLUdpError::InvalidMessage);
            }
        }
        if special_code & SPECIAL_HAS_TEXT != 0 {
            skip_c_string(&mut data)?;
            data.take(4)?;
        }
        if special_code & SPECIAL_HAS_MEDIA_URL != 0 {
            skip_c_string(&mut data)?;
        }
        if special_code & SPECIAL_HAS_LEGACY_PARTICLE_SYSTEM != 0 {
            data.take(LEGACY_PARTICLE_SYSTEM_SIZE)?;
        }

        let extra_parameter_count = skip_extra_parameters(&mut data)?;
        add_preflight_capacity::<ExtraParameter>(&mut allocation_bytes, extra_parameter_count)?;

        if special_code & SPECIAL_HAS_SOUND != 0 {
            data.take(16 + 4 + 1 + 4)?;
        }
        if special_code & SPECIAL_HAS_NAME_VALUE != 0 {
            skip_c_string(&mut data)?;
        }
        if pcode == VOLUME_PCODE {
            ObjectShape::decode_compressed(&mut data)?;
            read_binary_data(&mut data)?;
            if special_code & SPECIAL_HAS_TEXTURE_ANIMATION != 0 {
                read_binary_data(&mut data)?;
            }
            if special_code & SPECIAL_HAS_PARTICLE_SYSTEM != 0 {
                skip_particle_system(&mut data)?;
            }
        }
    }

    Ok(allocation_bytes)
}

fn skip_c_string(reader: &mut Reader<'_>) -> Result<(), LLUdpError> {
    let len = reader.remaining().iter().position(|byte| *byte == 0).ok_or(LLUdpError::TruncatedPacket)?;
    reader.take(len.checked_add(1).ok_or(LLUdpError::AllocationBoundOverflow)?)?;
    Ok(())
}

fn skip_particle_system(reader: &mut Reader<'_>) -> Result<(), LLUdpError> {
    let mut data = Reader::new(reader.remaining());
    let system_size = usize::try_from(data.i32()?).map_err(|_| LLUdpError::InvalidMessage)?;
    data.take(system_size)?;
    let particle_size = usize::try_from(data.i32()?).map_err(|_| LLUdpError::InvalidMessage)?;
    data.take(particle_size)?;
    reader.take(data.position())?;
    Ok(())
}

pub(crate) fn encode_fields(object: &ObjectData, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
    let mut packed_data = Vec::new();
    {
        let mut data = Writer::with_limit(&mut packed_data, usize::from(u16::MAX));
        encode_object_data(object, &mut data)?;
        data.finish().map_err(|_| LLUdpError::VariableFieldTooLong)?;
    }
    writer.variable2(&packed_data)
}

fn encode_object_data(object: &ObjectData, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
    if object.pcode == VOLUME_PCODE {
        if object.shape.is_none() || object.texture_entry.is_none() {
            return Err(LLUdpError::InvalidMessage);
        }
    } else if object.shape.is_some()
        || object.texture_entry.is_some()
        || object.texture_animation.is_some()
        || object.particle_system.is_some()
    {
        return Err(LLUdpError::InvalidMessage);
    }
    if object.legacy_particle_system.as_ref().is_some_and(|value| value.len() != LEGACY_PARTICLE_SYSTEM_SIZE) {
        return Err(LLUdpError::InvalidMessage);
    }
    if object.pcode == VOLUME_PCODE {
        if let Some(particle_system) = &object.particle_system {
            let mut particle_data = Reader::new(particle_system);
            read_particle_system(&mut particle_data)?;
            if !particle_data.is_empty() {
                return Err(LLUdpError::InvalidMessage);
            }
        }
        if object.texture_animation.is_some() != (object.special_code & SPECIAL_HAS_TEXTURE_ANIMATION != 0)
            || object.particle_system.is_some() != (object.special_code & SPECIAL_HAS_PARTICLE_SYSTEM != 0)
        {
            return Err(LLUdpError::InvalidMessage);
        }
    }

    writer.uuid(object.id);
    writer.u32(object.local_id);
    writer.u8(object.pcode);
    writer.u8(object.state);
    writer.u32(object.crc);
    writer.u8(object.material);
    writer.u8(object.click_action);
    writer.finite_vector3(object.scale)?;
    writer.finite_vector3(object.position)?;
    writer.quaternion_xyz(object.rotation)?;

    let special_code = special_code_for(object);
    writer.u32(special_code);
    writer.uuid(object.owner_id);
    if let Some(angular_velocity) = object.angular_velocity {
        writer.finite_vector3(angular_velocity)?;
    }
    if special_code & SPECIAL_HAS_PARENT != 0 {
        writer.u32(object.parent_local_id);
    }
    match &object.data {
        Some(ObjectSpecificData::Tree { species }) => writer.u8(*species),
        Some(ObjectSpecificData::ScratchPad(bytes)) => {
            writer.u32(u32::try_from(bytes.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
            write_binary_data(writer, bytes)?;
        }
        None => {}
    }
    if let Some(text) = &object.text {
        write_c_string(writer, &text.text)?;
        writer.bytes(&text.color);
    }
    if let Some(media_url) = &object.media_url {
        write_c_string(writer, media_url)?;
    }
    if let Some(particle_system) = &object.legacy_particle_system {
        writer.bytes(particle_system);
    }
    encode_extra_parameters(&object.extra_parameters, writer)?;
    if let Some(sound) = object.sound {
        if !sound.gain.is_finite() || !sound.radius.is_finite() {
            return Err(LLUdpError::InvalidMessage);
        }
        writer.uuid(sound.sound_id);
        writer.f32(sound.gain);
        writer.u8(sound.flags);
        writer.f32(sound.radius);
    }
    if let Some(name_value) = &object.name_value {
        write_c_string(writer, name_value)?;
    }
    if object.pcode == VOLUME_PCODE {
        object.shape.ok_or(LLUdpError::InvalidMessage)?.encode_compressed(writer);
        write_binary_data(writer, object.texture_entry.as_deref().expect("volume texture entry was checked above"))?;
        if let Some(texture_animation) = &object.texture_animation {
            write_binary_data(writer, texture_animation)?;
        }
        if let Some(particle_system) = &object.particle_system {
            writer.bytes(particle_system);
        }
    }
    Ok(())
}

fn special_code_for(object: &ObjectData) -> u32 {
    let mut special_code = object.special_code & !SPECIAL_OBJECT_FIELDS;
    if let Some(ObjectSpecificData::ScratchPad(_)) = &object.data {
        special_code |= SPECIAL_HAS_SCRATCH_PAD;
    }
    if let Some(ObjectSpecificData::Tree { .. }) = &object.data {
        special_code |= SPECIAL_HAS_TREE_DATA;
    }
    if object.text.is_some() {
        special_code |= SPECIAL_HAS_TEXT;
    }
    if object.legacy_particle_system.is_some() {
        special_code |= SPECIAL_HAS_LEGACY_PARTICLE_SYSTEM;
    }
    if object.sound.is_some() {
        special_code |= SPECIAL_HAS_SOUND;
    }
    if object.parent_local_id != 0 || object.special_code & SPECIAL_HAS_PARENT != 0 {
        special_code |= SPECIAL_HAS_PARENT;
    }
    if object.pcode == VOLUME_PCODE {
        if object.texture_animation.is_some() {
            special_code |= SPECIAL_HAS_TEXTURE_ANIMATION;
        }
        if object.particle_system.is_some() {
            special_code |= SPECIAL_HAS_PARTICLE_SYSTEM;
        }
    } else {
        special_code |= object.special_code & (SPECIAL_HAS_TEXTURE_ANIMATION | SPECIAL_HAS_PARTICLE_SYSTEM);
    }
    if object.angular_velocity.is_some() {
        special_code |= SPECIAL_HAS_ANGULAR_VELOCITY;
    }
    if object.name_value.is_some() {
        special_code |= SPECIAL_HAS_NAME_VALUE;
    }
    if object.media_url.is_some() {
        special_code |= SPECIAL_HAS_MEDIA_URL;
    }
    special_code
}

fn read_binary_data<'a>(reader: &mut Reader<'a>) -> Result<&'a [u8], LLUdpError> {
    let len = usize::try_from(reader.i32()?).map_err(|_| LLUdpError::InvalidMessage)?;
    reader.take(len)
}

fn write_binary_data(writer: &mut Writer<'_>, bytes: &[u8]) -> Result<(), LLUdpError> {
    writer.i32(i32::try_from(bytes.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
    writer.bytes(bytes);
    Ok(())
}

fn read_c_string(reader: &mut Reader<'_>) -> Result<String, LLUdpError> {
    let len = reader.remaining().iter().position(|byte| *byte == 0).ok_or(LLUdpError::TruncatedPacket)?;
    let bytes = reader.take(len + 1)?;
    std::str::from_utf8(&bytes[..len]).map(str::to_owned).map_err(|_| LLUdpError::InvalidUtf8)
}

fn write_c_string(writer: &mut Writer<'_>, value: &str) -> Result<(), LLUdpError> {
    if value.contains('\0') {
        return Err(LLUdpError::InvalidMessage);
    }
    writer.bytes(value.as_bytes());
    writer.u8(0);
    Ok(())
}

fn read_particle_system<'a>(reader: &mut Reader<'a>) -> Result<&'a [u8], LLUdpError> {
    let len = {
        let mut data = Reader::new(reader.remaining());
        let system_size = usize::try_from(data.i32()?).map_err(|_| LLUdpError::InvalidMessage)?;
        data.take(system_size)?;
        let part_size = usize::try_from(data.i32()?).map_err(|_| LLUdpError::InvalidMessage)?;
        data.take(part_size)?;
        data.position()
    };
    reader.take(len)
}
