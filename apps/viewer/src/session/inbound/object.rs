use std::num::{NonZeroU8, NonZeroU32};

use protocol::messages::object as object_messages;
use protocol::{
    lludp::{
        Message,
        Message::{
            GenericStreamingMessage, ImprovedTerseObjectUpdate, KillObject, ObjectExtraParams, ObjectUpdate, ObjectUpdateCached,
            ObjectUpdateCompressed,
        },
    },
    messages::{
        CachedObjectReference, ExtraParamUpdate, ExtraParameter, ObjectData, object::improved_terse_object_update,
        object::object_update_compressed,
    },
    types::{ObjectShape, TextureEntry, TextureFace, decode_render_material_bindings},
};
use utils::AssetId;
use uuid::Uuid;
use world::{
    Appearance, AttachmentPoint, FaceAppearance, FieldUpdate, Kind, LinearMotion, LocalAddress, MotionPatch, ObjectObservation,
    ObjectPatch, OrderEvidence, ParentUpdate, Pose, PrimShape, Region, RegionHandle, RenderMaterials, SculptFlags, SculptType, Shape,
    SurfaceAppearance, UpdateSource,
};

use super::super::object_media::media_version;

const VOLUME_PCODE: u8 = 9;
const LEGACY_AVATAR_PCODE: u8 = 47;
const SCULPT_EXTRA_PARAM_TYPE: u16 = 0x0030;
const MESH_EXTRA_PARAM_TYPE: u16 = 0x0060;
const RENDER_MATERIAL_EXTRA_PARAM_TYPE: u16 = 0x0080;
const SCULPT_EXTRA_PARAM_LEN: usize = 17;
const SCULPT_TYPE_MASK: u8 = 0x07;
const SCULPT_RESERVED_TYPE_MASK: u8 = 0x38;
const SCULPT_FLAG_INVERT: u8 = 0x40;
const SCULPT_FLAG_MIRROR: u8 = 0x80;
const SCULPT_DEFAULT_TEXTURE: u128 = 0xbe29_3869_d0d9_0a69_5989_ad27_f194_6fd4;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ObjectBatch {
    pub(crate) region: Region,
    pub(crate) region_handle: Option<RegionHandle>,
    pub(crate) blocks: Vec<Option<ObjectOperation>>,
    pub(crate) invalid_blocks: usize,
    pub(crate) invalid_fields: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ObjectOperation {
    Observe(ObjectObservation),
    Patch(ObjectPatch),
    RenderMaterialsPatch(ObjectPatch),
    Remove { source: UpdateSource, local_id: NonZeroU32 },
    Cached { source: UpdateSource, local_id: NonZeroU32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::session) struct ObjectMediaRequest {
    pub(in crate::session) source: UpdateSource,
    pub(in crate::session) object_id: Uuid,
    pub(in crate::session) local_id: NonZeroU32,
    pub(in crate::session) version: u32,
}

impl ObjectOperation {
    pub(crate) fn source(&self) -> UpdateSource {
        match self {
            Self::Observe(observation) => observation.source,
            Self::Patch(patch) | Self::RenderMaterialsPatch(patch) => patch.source,
            Self::Remove { source, .. } | Self::Cached { source, .. } => *source,
        }
    }

    pub(crate) fn address(&self) -> LocalAddress {
        let (source, local_id) = match self {
            Self::Observe(observation) => (observation.source, observation.local_id),
            Self::Patch(patch) | Self::RenderMaterialsPatch(patch) => (patch.source, patch.local_id),
            Self::Remove { source, local_id } | Self::Cached { source, local_id } => (*source, *local_id),
        };
        LocalAddress::new(source.region(), local_id)
    }
}

pub(crate) fn decode_object_message(region: Region, sequence: u32, order: OrderEvidence, message: &Message) -> Option<ObjectBatch> {
    let mut invalid_blocks = 0;
    let mut invalid_fields = 0;
    let blocks = match message {
        ObjectUpdate(update) => {
            map_blocks(region, sequence, order, &update.objects, full_observation, &mut invalid_blocks, &mut invalid_fields)
        }
        ObjectUpdateCompressed(update) => {
            map_blocks(region, sequence, order, &update.objects, compressed_observation, &mut invalid_blocks, &mut invalid_fields)
        }
        ImprovedTerseObjectUpdate(update) => {
            map_terse_blocks(region, sequence, order, &update.objects, &mut invalid_blocks, &mut invalid_fields)
        }
        ObjectUpdateCached(update) => {
            map_blocks(region, sequence, order, &update.objects, cached_patch, &mut invalid_blocks, &mut invalid_fields)
        }
        ObjectExtraParams(update) => map_blocks(
            region,
            sequence,
            order,
            update.object_data.iter().filter(|parameter| parameter.parameter_type == RENDER_MATERIAL_EXTRA_PARAM_TYPE),
            render_material_patch,
            &mut invalid_blocks,
            &mut invalid_fields,
        ),
        KillObject(update) => map_blocks(
            region,
            sequence,
            order,
            &update.object_data,
            |object, source, _| Ok(ObjectOperation::Remove { source, local_id: local_id(object.local_id)? }),
            &mut invalid_blocks,
            &mut invalid_fields,
        ),
        GenericStreamingMessage(message) => match super::gltf::decode_message(message) {
            Some(Ok(decoded)) => {
                let source = UpdateSource::new(region, sequence, order, 0);
                let mut patch = ObjectPatch::new(source, decoded.local_id);
                patch.gltf_material_overrides = Some(decoded.overrides);
                vec![Some(ObjectOperation::Patch(patch))]
            }
            Some(Err(())) => {
                invalid_blocks = 1;
                vec![None]
            }
            None => return None,
        },
        _ => return None,
    };

    Some(ObjectBatch { region, region_handle: object_region_handle(message), blocks, invalid_blocks, invalid_fields })
}

pub(in crate::session) fn object_media_requests(
    region: Region,
    sequence: u32,
    order: OrderEvidence,
    message: &Message,
) -> Vec<ObjectMediaRequest> {
    match message {
        ObjectUpdate(update) => update
            .objects
            .iter()
            .enumerate()
            .filter_map(|(index, object)| {
                media_request(region, sequence, order, index, object.pcode, object.id, object.local_id, &object.media_url)
            })
            .collect(),
        ObjectUpdateCompressed(update) => update
            .objects
            .iter()
            .enumerate()
            .filter_map(|(index, object)| {
                media_request(
                    region,
                    sequence,
                    order,
                    index,
                    object.pcode,
                    object.id,
                    object.local_id,
                    object.media_url.as_deref().unwrap_or_default(),
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn media_request(
    region: Region,
    sequence: u32,
    order: OrderEvidence,
    index: usize,
    pcode: u8,
    object_id: Uuid,
    raw_local_id: u32,
    media_url: &str,
) -> Option<ObjectMediaRequest> {
    if pcode != VOLUME_PCODE || object_id.is_nil() {
        return None;
    }
    let version = media_version(media_url)?;
    let local_id = local_id(raw_local_id).ok()?;
    let source = UpdateSource::new(region, sequence, order, u32::try_from(index).ok()?);
    if !matches!(order, OrderEvidence::Ordered(_)) {
        return None;
    }
    Some(ObjectMediaRequest { source, object_id, local_id, version })
}

fn object_region_handle(message: &Message) -> Option<RegionHandle> {
    let raw_handle = match message {
        ObjectUpdate(update) => update.region_data.region_handle,
        ObjectUpdateCompressed(update) => update.region_data.region_handle,
        ImprovedTerseObjectUpdate(update) => update.region_data.region_handle,
        ObjectUpdateCached(update) => update.region_data.region_handle,
        _ => return None,
    };
    Some(RegionHandle::new(raw_handle))
}

fn map_blocks<'a, T: 'a>(
    region: Region,
    sequence: u32,
    order: OrderEvidence,
    blocks: impl IntoIterator<Item = &'a T>,
    mut to_operation: impl FnMut(&T, UpdateSource, &mut usize) -> Result<ObjectOperation, InvalidBlock>,
    invalid_blocks: &mut usize,
    invalid_fields: &mut usize,
) -> Vec<Option<ObjectOperation>> {
    blocks
        .into_iter()
        .enumerate()
        .map(|(index, block)| {
            let source = u32::try_from(index).ok().map(|ordinal| UpdateSource::new(region, sequence, order, ordinal));
            let operation = source.and_then(|source| to_operation(block, source, invalid_fields).ok());
            if operation.is_none() {
                *invalid_blocks += 1;
            }
            operation
        })
        .collect()
}

fn map_terse_blocks(
    region: Region,
    sequence: u32,
    order: OrderEvidence,
    blocks: &[improved_terse_object_update::ObjectBlock],
    invalid_blocks: &mut usize,
    invalid_fields: &mut usize,
) -> Vec<Option<ObjectOperation>> {
    blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            let operation = match block {
                improved_terse_object_update::ObjectBlock::Decoded(block) => {
                    let source = u32::try_from(index).ok().map(|ordinal| UpdateSource::new(region, sequence, order, ordinal));
                    source.and_then(|source| terse_patch(block, source, invalid_fields).ok())
                }
                improved_terse_object_update::ObjectBlock::Malformed { .. } => None,
            };
            if operation.is_none() {
                *invalid_blocks += 1;
            }
            operation
        })
        .collect()
}

#[derive(Clone, Copy)]
struct InvalidBlock;

fn full_observation(block: &ObjectData, source: UpdateSource, invalid_fields: &mut usize) -> Result<ObjectOperation, InvalidBlock> {
    let id = non_nil_uuid(block.id)?;
    let local_id = local_id(block.local_id)?;
    let classification = object_classification(block.pcode, Some(block.shape), &block.extra_parameters, invalid_fields);
    let surface_appearance = if block.pcode == VOLUME_PCODE {
        let (value, invalid) = decoded_surface_appearance(object_messages::decode_texture_entry(&block.texture_entry));
        *invalid_fields += usize::from(invalid);
        value
    } else {
        None
    };
    let render_materials = render_materials(block.pcode, &block.extra_parameters, invalid_fields);
    Ok(ObjectOperation::Observe(ObjectObservation {
        source,
        id,
        local_id,
        kind: classification.kind,
        parent: parent(block.parent_local_id)?,
        pose: Some(Pose { position: Some(block.motion.position), rotation: Some(block.motion.rotation) }),
        linear_motion: Some(LinearMotion { velocity: Some(block.motion.velocity), acceleration: Some(block.motion.acceleration) }),
        angular_velocity: Some(block.motion.angular_velocity),
        scale: Some(block.scale),
        attachment_point: attachment_point(block.state),
        shape: classification.shape,
        appearance: classification.appearance,
        surface_appearance,
        render_materials,
    }))
}

fn compressed_observation(
    block: &object_update_compressed::ObjectData,
    source: UpdateSource,
    invalid_fields: &mut usize,
) -> Result<ObjectOperation, InvalidBlock> {
    let id = non_nil_uuid(block.id)?;
    let local_id = local_id(block.local_id)?;
    let parent = parent(block.parent_local_id)?;
    let classification = object_classification(block.pcode, block.shape, &block.extra_parameters, invalid_fields);
    let surface_appearance = if block.pcode == VOLUME_PCODE {
        let decoded = block.texture_entry.as_deref().map(object_messages::decode_texture_entry).unwrap_or(Ok(None));
        let (value, invalid) = decoded_surface_appearance(decoded);
        *invalid_fields += usize::from(invalid);
        value
    } else {
        None
    };
    let render_materials = render_materials(block.pcode, &block.extra_parameters, invalid_fields);
    Ok(ObjectOperation::Observe(ObjectObservation {
        source,
        id,
        local_id,
        kind: classification.kind,
        parent,
        pose: Some(Pose { position: Some(block.position), rotation: Some(block.rotation) }),
        linear_motion: None,
        angular_velocity: block.angular_velocity,
        scale: Some(block.scale),
        attachment_point: attachment_point(block.state),
        shape: classification.shape,
        appearance: classification.appearance,
        surface_appearance,
        render_materials,
    }))
}

fn terse_patch(
    block: &improved_terse_object_update::ObjectData,
    source: UpdateSource,
    invalid_fields: &mut usize,
) -> Result<ObjectOperation, InvalidBlock> {
    let mut patch = ObjectPatch::new(source, local_id(block.local_id)?);
    patch.pose = Some(Pose { position: Some(block.motion.position), rotation: Some(block.motion.rotation) });
    patch.linear_motion = MotionPatch { velocity: Some(block.motion.velocity), acceleration: Some(block.motion.acceleration) };
    patch.angular_velocity = Some(block.motion.angular_velocity);
    patch.attachment_point = attachment_point(block.state);
    if !block.avatar {
        let (value, invalid) = decoded_surface_appearance(improved_terse_object_update::decode_texture_entry(&block.texture_entry));
        *invalid_fields += usize::from(invalid);
        patch.surface_appearance = value;
    }
    Ok(ObjectOperation::Patch(patch))
}

fn decoded_surface_appearance<E>(decoded: Result<Option<TextureEntry>, E>) -> (Option<SurfaceAppearance>, bool) {
    match decoded {
        Ok(Some(entry)) => {
            let value = surface_appearance(entry);
            let invalid = value.is_none();
            (value, invalid)
        }
        Ok(None) => (None, false),
        Err(_) => (None, true),
    }
}

fn surface_appearance(entry: TextureEntry) -> Option<SurfaceAppearance> {
    let default_face = face_appearance(entry.default_face());
    let overrides = entry.overrides().map(|(index, face)| (index, face_appearance(face))).collect();
    SurfaceAppearance::new(default_face, overrides).ok()
}

fn render_materials(pcode: u8, extra_parameters: &[ExtraParameter], invalid_fields: &mut usize) -> Option<RenderMaterials> {
    if pcode != VOLUME_PCODE {
        return (object_kind(pcode) == Kind::Object).then(RenderMaterials::default);
    }

    let mut materials = None;
    let mut found = false;
    for extra_parameter in extra_parameters.iter().filter(|p| p.parameter_type == RENDER_MATERIAL_EXTRA_PARAM_TYPE) {
        found = true;
        match decode_render_material_bindings(&extra_parameter.data).map_err(|_| ()).and_then(|bindings| {
            RenderMaterials::new(bindings.into_iter().map(|binding| (binding.face_index, AssetId::new(binding.material_id))).collect())
                .map_err(|_| ())
        }) {
            Ok(value) => materials = Some(value),
            Err(()) => {
                *invalid_fields += 1;
                materials = None;
            }
        }
    }

    if found { materials } else { Some(RenderMaterials::default()) }
}

fn render_material_patch(block: &ExtraParamUpdate, source: UpdateSource, _: &mut usize) -> Result<ObjectOperation, InvalidBlock> {
    let materials = if block.in_use {
        let bindings = decode_render_material_bindings(&block.data).map_err(|_| InvalidBlock)?;
        RenderMaterials::new(bindings.into_iter().map(|binding| (binding.face_index, AssetId::new(binding.material_id))).collect())
            .map_err(|_| InvalidBlock)?
    } else {
        RenderMaterials::default()
    };
    let mut patch = ObjectPatch::new(source, local_id(block.local_id)?);
    patch.render_materials = Some(materials);
    Ok(ObjectOperation::RenderMaterialsPatch(patch))
}

fn face_appearance(face: &TextureFace) -> FaceAppearance {
    FaceAppearance {
        texture_id: face.texture_id,
        color: face.color,
        scale_s: face.scale_s,
        scale_t: face.scale_t,
        offset_s: face.offset_s,
        offset_t: face.offset_t,
        rotation: face.rotation,
        bump_shiny_fullbright: face.bump_shiny_fullbright,
        media_texgen: face.media_texgen,
        glow: face.glow,
        material_id: face.material_id,
    }
}

fn cached_patch(block: &CachedObjectReference, source: UpdateSource, _: &mut usize) -> Result<ObjectOperation, InvalidBlock> {
    Ok(ObjectOperation::Cached { source, local_id: local_id(block.local_id)? })
}

fn local_id(value: u32) -> Result<NonZeroU32, InvalidBlock> {
    NonZeroU32::new(value).ok_or(InvalidBlock)
}

fn non_nil_uuid(value: Uuid) -> Result<Uuid, InvalidBlock> {
    if value.is_nil() { Err(InvalidBlock) } else { Ok(value) }
}

fn attachment_point(state: u8) -> FieldUpdate<AttachmentPoint> {
    let point = state.rotate_left(4) & !0x80;
    NonZeroU8::new(point).map_or(FieldUpdate::Clear, |value| FieldUpdate::Set(AttachmentPoint::new(value)))
}

fn parent(raw_local_id: u32) -> Result<ParentUpdate, InvalidBlock> {
    if raw_local_id == 0 { Ok(ParentUpdate::Root) } else { Ok(ParentUpdate::Local(local_id(raw_local_id)?)) }
}

fn object_kind(pcode: u8) -> Kind {
    match pcode {
        LEGACY_AVATAR_PCODE => Kind::Avatar,
        1..=63 if matches!(pcode & 0x0f, 1..=9) => Kind::Object,
        14 | 15 | 95 | 111 | 143 | 159 | 239 | 255 => Kind::Object,
        _ => Kind::Unknown(pcode),
    }
}

#[derive(Clone, Copy)]
struct ObjectClassification {
    kind: Kind,
    shape: FieldUpdate<Shape>,
    appearance: FieldUpdate<Appearance>,
}

fn prim_shape(shape: ObjectShape) -> PrimShape {
    PrimShape {
        path_curve: shape.path_curve,
        profile_curve: shape.profile_curve,
        path_begin: shape.path_begin,
        path_end: shape.path_end,
        path_scale_x: shape.path_scale_x,
        path_scale_y: shape.path_scale_y,
        path_shear_x: i8::from_ne_bytes([shape.path_shear_x]),
        path_shear_y: i8::from_ne_bytes([shape.path_shear_y]),
        path_twist: shape.path_twist,
        path_twist_begin: shape.path_twist_begin,
        path_radius_offset: shape.path_radius_offset,
        path_taper_x: shape.path_taper_x,
        path_taper_y: shape.path_taper_y,
        path_revolutions: shape.path_revolutions,
        path_skew: shape.path_skew,
        profile_begin: shape.profile_begin,
        profile_end: shape.profile_end,
        profile_hollow: shape.profile_hollow,
    }
}

fn object_classification(
    pcode: u8,
    prim: Option<ObjectShape>,
    extra_parameters: &[ExtraParameter],
    invalid_fields: &mut usize,
) -> ObjectClassification {
    match pcode {
        LEGACY_AVATAR_PCODE => ObjectClassification {
            kind: Kind::Avatar,
            shape: FieldUpdate::Set(Shape::Avatar),
            appearance: FieldUpdate::Set(Appearance::Avatar),
        },
        VOLUME_PCODE => {
            let mut sculpt_parameter = None;
            for parameter in extra_parameters {
                if matches!(parameter.parameter_type, SCULPT_EXTRA_PARAM_TYPE | MESH_EXTRA_PARAM_TYPE) {
                    if parameter.data.len() != SCULPT_EXTRA_PARAM_LEN {
                        *invalid_fields += 1;
                    }
                    sculpt_parameter = Some(parameter);
                }
            }

            let shape = match sculpt_parameter {
                Some(parameter) if parameter.data.len() != SCULPT_EXTRA_PARAM_LEN => FieldUpdate::Keep,
                Some(parameter) => {
                    let raw_type = parameter.data[16];
                    let sculpt_type = raw_type & SCULPT_TYPE_MASK;
                    if raw_type & SCULPT_RESERVED_TYPE_MASK != 0 || sculpt_type > 6 {
                        FieldUpdate::Set(Shape::Sculpt {
                            asset_id: AssetId::new(Uuid::from_u128(SCULPT_DEFAULT_TEXTURE)),
                            sculpt_type: SculptType::Sphere,
                            flags: SculptFlags::default(),
                        })
                    } else {
                        let flags = SculptFlags { invert: raw_type & SCULPT_FLAG_INVERT != 0, mirror: raw_type & SCULPT_FLAG_MIRROR != 0 };
                        let asset_id =
                            AssetId::new(Uuid::from_slice(&parameter.data[..16]).expect("sculpt parameter contains a complete asset UUID"));
                        match sculpt_type {
                            0 => prim.map_or(FieldUpdate::Keep, |shape| FieldUpdate::Set(Shape::Prim(prim_shape(shape)))),
                            1 => FieldUpdate::Set(Shape::Sculpt { asset_id, sculpt_type: SculptType::Sphere, flags }),
                            2 => FieldUpdate::Set(Shape::Sculpt { asset_id, sculpt_type: SculptType::Torus, flags }),
                            3 => FieldUpdate::Set(Shape::Sculpt { asset_id, sculpt_type: SculptType::Plane, flags }),
                            4 => FieldUpdate::Set(Shape::Sculpt { asset_id, sculpt_type: SculptType::Cylinder, flags }),
                            5 => FieldUpdate::Set(Shape::Mesh { asset_id, flags }),
                            6 => FieldUpdate::Clear,
                            _ => FieldUpdate::Keep,
                        }
                    }
                }
                None => prim.map_or(FieldUpdate::Keep, |shape| FieldUpdate::Set(Shape::Prim(prim_shape(shape)))),
            };

            ObjectClassification { kind: Kind::Object, shape, appearance: FieldUpdate::Set(Appearance::Surface) }
        }
        _ => ObjectClassification { kind: object_kind(pcode), shape: FieldUpdate::Clear, appearance: FieldUpdate::Clear },
    }
}
#[cfg(test)]
#[path = "object_tests.rs"]
mod tests;
