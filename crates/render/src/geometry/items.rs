use std::{ops::Range, sync::Arc, time::Instant};

use assets::TextureHandle;
use math::{Bounds, Quat, Vec3};
use world::{Entity, FaceAppearance, Region, SurfaceAppearance};

use super::draw_list::{DrawKey, DrawList, FaceOrder};
use super::instance::parameter_part;
use super::{ShadingMode, TRANSFORM_BYTES};
use crate::{
    material::{FaceMaterial, FaceParams},
    pipeline::{COLOR, Pipeline},
    scene::{Camera, Object, SharedMesh, SurfaceKind},
};

pub(super) type ItemId = (u8, u128, u16);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct ItemIndex(u32);

pub(super) struct MeshInfo {
    pub(super) bounds: Bounds,
    pub(super) centers: Vec<Vec3>,
}

pub(super) struct CachedFace {
    pub(super) order: FaceOrder,
    pub(super) indices: Range<u32>,
    pub(super) center: Vec3,
    pub(super) base_params: FaceParams,
    pub(super) params: FaceParams,
    pub(super) texture: Option<TextureHandle>,
    pub(super) material_index: Option<usize>,
    pub(super) texture_entry_transform: math::UvTransform,
    pub(super) group: u32,
    pub(super) blended: bool,
    pub(super) parameter_bytes: [u8; super::instance::PARAMETER_BYTES],
    pub(super) slot: Option<u32>,
    pub(super) blended_index: Option<usize>,
}

pub(super) struct CachedItem {
    pub(super) id: ItemId,
    pub(super) entity: Option<Entity>,
    pub(super) region: Region,
    pub(super) mesh: SharedMesh,
    pub(super) info: Arc<MeshInfo>,
    pub(super) position: math::DVec3,
    pub(super) rotation: Quat,
    pub(super) scale: Vec3,
    pub(super) invert_normals: bool,
    pub(super) replacement: Option<Box<CachedReplacement>>,
    pub(super) transform: [u8; TRANSFORM_BYTES],
    pub(super) pickable: bool,
    pub(super) shading: ShadingMode,
    pub(super) terrain_textures: Option<[Option<TextureHandle>; 4]>,
    pub(super) terrain_composition: Option<Arc<[f32]>>,
    pub(super) surface_kind: Option<SurfaceKind>,
    pub(super) source: Option<Object>,
    pub(super) faces: Vec<CachedFace>,
    pub(super) animated_faces: Vec<usize>,
    pub(super) animation_deadline: Option<Instant>,
    pub(super) parameters_dirty: bool,
}

pub(super) struct CachedReplacement {
    pub(super) item: CachedItem,
    pub(super) retry_state: Option<super::MeshState>,
}

#[derive(Default)]
pub(super) struct ItemSlab {
    slots: Vec<Option<CachedItem>>,
    free: Vec<ItemIndex>,
}

impl ItemSlab {
    pub(super) fn insert(&mut self, item: CachedItem) -> Option<ItemIndex> {
        let index = match self.free.pop() {
            Some(index) => index,
            None => {
                let index = ItemIndex(u32::try_from(self.slots.len()).ok()?);
                self.slots.push(None);
                index
            }
        };
        let slot = self.slots.get_mut(index.0 as usize)?;
        if slot.is_some() {
            self.free.push(index);
            return None;
        }
        *slot = Some(item);
        Some(index)
    }

    pub(super) fn remove(&mut self, index: ItemIndex) -> Option<CachedItem> {
        let slot = self.slots.get_mut(index.0 as usize)?;
        let item = slot.take()?;
        self.free.push(index);
        Some(item)
    }

    pub(super) fn get(&self, index: ItemIndex) -> Option<&CachedItem> {
        self.slots.get(index.0 as usize)?.as_ref()
    }

    pub(super) fn get_mut(&mut self, index: ItemIndex) -> Option<&mut CachedItem> {
        self.slots.get_mut(index.0 as usize)?.as_mut()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (ItemIndex, &CachedItem)> {
        self.slots.iter().enumerate().filter_map(|(index, item)| item.as_ref().map(|item| (ItemIndex(index as u32), item)))
    }

    pub(super) fn iter_mut(&mut self) -> impl Iterator<Item = (ItemIndex, &mut CachedItem)> {
        self.slots.iter_mut().enumerate().filter_map(|(index, item)| item.as_mut().map(|item| (ItemIndex(index as u32), item)))
    }

    pub(super) fn clear(&mut self) {
        self.slots.clear();
        self.free.clear();
    }
}

pub(super) struct CachedFaceContext<'a> {
    pub(super) id: ItemId,
    pub(super) mesh: &'a SharedMesh,
    pub(super) info: &'a MeshInfo,
    pub(super) materials: Option<&'a [FaceMaterial]>,
    pub(super) appearance: Option<&'a SurfaceAppearance>,
    pub(super) scale: Vec3,
    pub(super) invert_normals: bool,
    pub(super) shading: ShadingMode,
    pub(super) terrain: Option<Region>,
    pub(super) terrain_textures: Option<&'a [Option<TextureHandle>; 4]>,
}

pub(super) fn mesh_info(mesh: &SharedMesh) -> Arc<MeshInfo> {
    let mesh = mesh.mesh();
    let centers = mesh
        .faces()
        .iter()
        .map(|face| {
            let mut min = Vec3::splat(f32::INFINITY);
            let mut max = Vec3::splat(f32::NEG_INFINITY);
            let start = usize::try_from(face.indices.start).unwrap_or(usize::MAX);
            let end = usize::try_from(face.indices.end).unwrap_or(usize::MAX);
            for index in mesh.indices().get(start..end).into_iter().flatten() {
                if let Some(vertex) = mesh.vertices().get(*index as usize) {
                    min = min.min(vertex.position);
                    max = max.max(vertex.position);
                }
            }
            if min.is_finite() && max.is_finite() { min * 0.5 + max * 0.5 } else { Vec3::ZERO }
        })
        .collect();
    Arc::new(MeshInfo { bounds: mesh.bounds(), centers })
}

pub(super) fn cached_faces(context: CachedFaceContext<'_>, draw_list: &mut DrawList) -> Vec<CachedFace> {
    let CachedFaceContext { id, mesh, info, materials, appearance, scale, invert_normals, shading, terrain, terrain_textures } = context;
    let mut material_slots = [None; 256];
    if let Some(materials) = materials {
        for (index, material) in materials.iter().enumerate() {
            material_slots[usize::from(material.slot)].get_or_insert(index);
        }
    }
    let faces = mesh.mesh().faces();
    let mut cached = Vec::with_capacity(faces.len());
    for (index, face) in faces.iter().enumerate() {
        if face.indices.is_empty() {
            continue;
        }
        let material_index = material_slots[usize::from(face.slot)];
        let material = materials.and_then(|materials| material_index.and_then(|index| materials.get(index)));
        let mut params = FaceParams::new(material);
        match shading {
            ShadingMode::Terrain => {
                params.color = [0.18, 0.23, 0.13, 1.0];
                params.params[0] = 1.0;
            }
            ShadingMode::Water => {
                params.color = COLOR.to_array();
                params.params[0] = 1.0;
            }
            ShadingMode::Studio => {}
        }
        if params.blend && params.color[3] <= 0.0 {
            continue;
        }
        let texture = material.and_then(|material| material.base_color_texture.as_ref()).and_then(|binding| binding.image()).cloned();
        let pipeline = match shading {
            ShadingMode::Terrain => Pipeline::Terrain,
            ShadingMode::Water => Pipeline::Water,
            ShadingMode::Studio => surface_pipeline(params.blend, params.double_sided, scale, invert_normals),
        };
        let key = DrawKey { mesh: mesh.id(), start: face.indices.start, end: face.indices.end, pipeline, texture: params.texture, terrain };
        let Some(group) = draw_list.intern_face(key, texture.as_ref(), terrain_textures) else { continue };
        cached.push(CachedFace {
            order: (id.0, id.1, id.2, face.slot),
            indices: face.indices.clone(),
            center: info.centers.get(index).copied().unwrap_or(Vec3::ZERO),
            base_params: params,
            params,
            texture,
            material_index,
            texture_entry_transform: appearance
                .and_then(|appearance| appearance.face(face.slot))
                .map_or_else(FaceAppearance::default, |face| *face)
                .transform,
            group,
            blended: key.blended(),
            parameter_bytes: parameter_part(&params, scale),
            slot: None,
            blended_index: None,
        });
    }
    cached
}

pub(super) fn same_surface_geometry(left: &CachedItem, right: &CachedItem) -> bool {
    left.mesh.id() == right.mesh.id()
        && left.position == right.position
        && left.rotation == right.rotation
        && left.scale == right.scale
        && left.invert_normals == right.invert_normals
        && left.pickable == right.pickable
        && left.shading == right.shading
        && left.terrain_composition == right.terrain_composition
}

pub(super) fn draw_scale(scale: Vec3, mirror: bool) -> Vec3 {
    if mirror { scale * Vec3::splat(-1.0) } else { scale }
}

pub(super) fn object_draw_scale(object: &Object, scale: Vec3) -> Vec3 {
    draw_scale(scale, object.mesh_flags().unwrap_or_default().mirror)
}

pub(super) fn draw_priority(item: &CachedItem, camera: Camera) -> (f64, f64) {
    let center = (item.info.bounds.min.as_dvec3() + item.info.bounds.max.as_dvec3()) * 0.5 * item.scale.as_dvec3();
    let offset = item.position - camera.position() + (item.rotation * center.as_vec3()).as_dvec3();
    let radius = ((item.info.bounds.max.as_dvec3() - item.info.bounds.min.as_dvec3()).abs() * item.scale.as_dvec3().abs() * 0.5).length();
    let distance = offset.length();
    (radius / distance.max(1.0), distance)
}

pub(super) fn surface_pipeline(blend: bool, double_sided: bool, scale: Vec3, invert_normals: bool) -> Pipeline {
    match (blend, double_sided, math::flips_winding(scale) ^ invert_normals) {
        (true, true, _) => Pipeline::BlendDoubleSided,
        (true, false, true) => Pipeline::BlendFlipped,
        (true, false, false) => Pipeline::Blend,
        (false, true, _) => Pipeline::DoubleSided,
        (false, false, true) => Pipeline::FlippedWinding,
        (false, false, false) => Pipeline::Surface,
    }
}

#[cfg(test)]
mod tests {
    use math::{DVec3, Quat};

    use super::*;
    use crate::{
        geometry::{draw_list::DrawList, instance::instance_base},
        pipeline::Pipeline,
    };

    #[test]
    fn first_face_pipeline_tracks_mirror_and_invert() {
        let (_scene, view, _) = crate::geometry::fixtures::scene(255);
        let object = view.objects().next().expect("mesh fixture object");
        let (_, mesh) = object.mesh().expect("mesh fixture geometry");
        let info = mesh_info(mesh);
        for (mirror, invert, expected_pipeline, expected_normal) in [
            (false, false, Pipeline::Surface, 1.0),
            (true, false, Pipeline::FlippedWinding, -1.0),
            (false, true, Pipeline::FlippedWinding, -1.0),
            (true, true, Pipeline::Surface, 1.0),
        ] {
            let scale = draw_scale(Vec3::ONE, mirror);
            let mut draw_list = DrawList::default();
            let faces = cached_faces(
                CachedFaceContext {
                    id: (2, object.detail().summary.id.as_u128(), 0),
                    mesh,
                    info: &info,
                    materials: object.materials(),
                    appearance: object.detail().surface_appearance.as_ref(),
                    scale,
                    invert_normals: invert,
                    shading: ShadingMode::Studio,
                    terrain: None,
                    terrain_textures: None,
                },
                &mut draw_list,
            );
            let face = faces.first().expect("fixture face");
            assert_eq!(draw_list.face_key(face.group).map(|key| key.pipeline), Some(expected_pipeline));
            let transform = instance_base(DVec3::ZERO, Quat::IDENTITY, scale, invert, DVec3::ZERO);
            let normals = transform
                .get(64..)
                .expect("instance contains a normal matrix")
                .as_chunks::<4>()
                .0
                .iter()
                .map(|bytes| f32::from_le_bytes(*bytes))
                .collect::<Vec<_>>();
            assert_eq!(normals.first(), Some(&expected_normal));
            assert_eq!(normals.get(5), Some(&expected_normal));
            assert_eq!(normals.get(10), Some(&expected_normal));
        }
    }
}
