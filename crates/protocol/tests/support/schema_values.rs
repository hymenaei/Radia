use std::array::from_fn;

use base::UUID;
use math::{DVec3, Quat, Vec3, Vec4};
use protocol::{
    BlockInstance, BlockValues, FieldValue,
    schema::{BlockMode, FieldKind, MessageSchema},
};

pub(crate) fn generated_schema_blocks(message_schema: &MessageSchema, seed: u64) -> Vec<BlockValues> {
    message_schema
        .blocks
        .iter()
        .enumerate()
        .map(|(block_index, block_schema)| {
            let count = match block_schema.mode {
                BlockMode::Single => 1,
                BlockMode::Variable => usize::try_from(seed.wrapping_add(block_index as u64) % 3).expect("small block count fits usize"),
                BlockMode::Multiple(count) => usize::try_from(count).expect("template block count fits usize"),
            };
            let instances = (0..count)
                .map(|instance_index| {
                    let mut fields = block_schema
                        .fields
                        .iter()
                        .enumerate()
                        .map(|(field_index, field)| {
                            let field_seed = seed
                                .wrapping_add((block_index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))
                                .wrapping_add((instance_index as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9))
                                .wrapping_add(field_index as u64);
                            generated_field_value(field.kind, field.name, field_seed)
                        })
                        .collect::<Vec<_>>();
                    if message_schema.name == "ObjectExtraParams" && block_schema.name == "ObjectData" {
                        let size_index = block_schema.fields.iter().position(|field| field.name == "ParamSize").expect("ParamSize field");
                        let data_index = block_schema.fields.iter().position(|field| field.name == "ParamData").expect("ParamData field");
                        let FieldValue::Bytes(data) = &fields[data_index] else {
                            panic!("generated ParamData is variable bytes");
                        };
                        fields[size_index] = FieldValue::U32(u32::try_from(data.len()).expect("generated ParamData length fits"));
                    }
                    BlockInstance::new(fields)
                })
                .collect();
            BlockValues::new(instances)
        })
        .collect()
}

fn generated_field_value(kind: FieldKind, name: &str, seed: u64) -> FieldValue {
    let component = |offset: u64| ((seed.wrapping_add(offset) % 201) as f32) - 100.0;
    match kind {
        FieldKind::Bool => FieldValue::Bool(seed & 1 != 0),
        FieldKind::F32 => FieldValue::F32((seed % 10_000) as f32 / 8.0 + 1.0),
        FieldKind::F64 => FieldValue::F64((seed % 10_000) as f64 / 8.0 + 1.0),
        FieldKind::IpAddr => {
            let bytes = seed.to_le_bytes();
            FieldValue::IpAddr([bytes[0], bytes[1], bytes[2], bytes[3]])
        }
        FieldKind::IpPort => FieldValue::IpPort((seed as u16).to_be_bytes()),
        FieldKind::Null => FieldValue::Null,
        FieldKind::Quaternion => {
            let rotation = if name == "BodyRotation" { Quat::IDENTITY } else { Quat::from_xyzw(1.0, 0.0, 0.0, 0.0) };
            FieldValue::Quaternion(rotation)
        }
        FieldKind::UUID => {
            let bytes = from_fn(|index| seed.wrapping_add(index as u64 * 31) as u8);
            FieldValue::UUID(UUID::from_bytes(bytes))
        }
        FieldKind::Vector3 => FieldValue::Vector3(Vec3::new(component(0), component(67), component(134))),
        FieldKind::Vector3d => {
            let component = |offset: u64| (seed.wrapping_add(offset) % 201) as f64 - 100.0;
            FieldValue::Vector3d(DVec3::new(component(0), component(67), component(134)))
        }
        FieldKind::Vector4 => FieldValue::Vector4(Vec4::new(component(0), component(50), component(100), component(150))),
        FieldKind::S8 => FieldValue::S8(seed as i8),
        FieldKind::S16 => FieldValue::S16(seed as i16),
        FieldKind::S32 => FieldValue::S32(seed as i32),
        FieldKind::S64 => FieldValue::S64(seed as i64),
        FieldKind::U8 if name == "CacheMissType" => FieldValue::U8((seed % 2) as u8),
        FieldKind::U8 => FieldValue::U8(seed as u8),
        FieldKind::U16 => FieldValue::U16(seed as u16),
        FieldKind::U32 => FieldValue::U32(seed as u32),
        FieldKind::U64 => FieldValue::U64(seed),
        FieldKind::Fixed(size) => FieldValue::Bytes(vec![seed as u8; usize::try_from(size).expect("field size fits usize")]),
        FieldKind::Variable(_) if name == "ChannelVersion" => FieldValue::Bytes(b"v2\0".to_vec()),
        FieldKind::Variable(_) if name == "Throttles" => FieldValue::Bytes(
            from_fn::<_, 7, _>(|index| ((seed.wrapping_add(index as u64) % 10_000) as f32 + 1.0).to_le_bytes())
                .into_iter()
                .flatten()
                .collect(),
        ),
        FieldKind::Variable(_) => FieldValue::Bytes(vec![b'a' + (seed % 26) as u8]),
    }
}
