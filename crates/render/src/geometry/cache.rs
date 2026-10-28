use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ops::Range,
    sync::Arc,
};

use assets::{TextureHandle, TextureId};
use math::{Bounds, Vec3};
use world::{Entity, Region};

use super::MeshState;
use super::{Instance, ShadingMode};
use crate::{
    geometry::batching::{DrawKey, FaceOrder},
    material::FaceParams,
    scene::{Camera, MeshId, Object, SharedMesh, ViewVersion},
    texture::{TextureKey, TextureState},
};

pub(super) type ItemId = (u8, u128, u16);

pub(super) struct MeshInfo {
    pub bounds: Bounds,
    pub centers: Vec<Vec3>,
}

pub(super) struct CachedFace {
    pub order: FaceOrder,
    pub indices: Range<u32>,
    pub center: Vec3,
    pub base_params: FaceParams,
    pub params: FaceParams,
    pub texture: Option<TextureHandle>,
    pub material_index: Option<usize>,
    pub texture_entry_transform: math::UvTransform,
    pub draw: DrawKey,
    pub slot: Option<u32>,
    pub included: bool,
}

pub(super) struct CachedItem {
    pub id: ItemId,
    pub entity: Option<Entity>,
    pub region: Region,
    pub mesh: SharedMesh,
    pub info: Arc<MeshInfo>,
    pub position: math::DVec3,
    pub rotation: math::Quat,
    pub scale: Vec3,
    pub pickable: bool,
    pub shading: ShadingMode,
    pub terrain_textures: Option<[Option<TextureHandle>; 4]>,
    pub terrain_composition: Option<Arc<[f32]>>,
    pub source: Option<Object>,
    pub faces: Vec<CachedFace>,
    pub visible_index: Option<usize>,
    pub parameters_dirty: bool,
}

impl CachedItem {
    pub fn instance(&self, with_faces: bool) -> Instance {
        Instance {
            mesh: self.mesh.clone(),
            position: self.position,
            rotation: self.rotation,
            scale: self.scale,
            pickable: self.pickable,
            faces: if with_faces {
                self.faces
                    .iter()
                    .filter(|face| face.included)
                    .map(|face| super::PickFace { indices: face.indices.clone(), double_sided: face.params.double_sided })
                    .collect()
            } else {
                Vec::new()
            },
        }
    }
}

#[derive(Default)]
pub(super) struct FrameCache {
    pub items: BTreeMap<ItemId, CachedItem>,
    pub objects: HashMap<Entity, ItemId>,
    pub mesh_info: HashMap<MeshId, Arc<MeshInfo>>,
    pub candidates: Vec<ItemId>,
    pub next_candidates: Vec<ItemId>,
    pub visible: Vec<ItemId>,
    pub next_visible: Vec<ItemId>,
    pub priority_items: Vec<ItemId>,
    pub slots: HashMap<FaceOrder, u32>,
    pub mesh_ids: Vec<MeshId>,
    pub mesh_set: HashSet<MeshId>,
    pub mesh_priorities: HashMap<MeshId, (f64, f64)>,
    pub mesh_ready: HashSet<MeshId>,
    pub mesh_representatives: HashMap<MeshId, SharedMesh>,
    pub mesh_compositions: HashMap<MeshId, Option<Arc<[f32]>>>,
    pub retained_meshes: HashSet<MeshId>,
    pub mesh_state: MeshState,
    pub visible_textures: Vec<TextureHandle>,
    pub visible_texture_ids: HashSet<TextureId>,
    pub texture_keys: Vec<TextureKey>,
    pub texture_set: HashSet<TextureKey>,
    pub resident_textures: HashMap<TextureKey, Option<TextureKey>>,
    pub update_items: Vec<ItemId>,
    pub update_set: HashSet<ItemId>,
    pub pose_set: HashSet<ItemId>,
    pub animated_items: HashSet<ItemId>,
    pub changed_objects: HashSet<Entity>,
    pub changed_surfaces: HashSet<(Region, crate::scene::SurfaceKind)>,
    pub retired_regions: HashSet<Region>,
    pub version: Option<ViewVersion>,
    pub camera: Option<Camera>,
    pub origin: math::DVec3,
    pub size: [u32; 2],
    pub texture_state: TextureState,
    pub batches_valid: bool,
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
