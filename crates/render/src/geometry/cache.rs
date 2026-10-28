use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ops::Range,
    sync::Arc,
    time::Instant,
};

use assets::{TextureHandle, TextureId};
use math::{Bounds, Frustum, Quat, Vec3};
use world::{Entity, FaceAppearance, Region};

use super::instance::is_visible;
use super::{GeometryRenderer, Instance, MeshState, ShadingMode};
use crate::{
    geometry::batching::{DrawKey, FaceOrder},
    material::{FaceMaterial, FaceParams},
    pipeline::{COLOR, Pipeline},
    scene::{Camera, MeshId, Object, Placement, PreparedMesh, SceneView, SharedMesh, Surface, SurfaceKind, ViewVersion, texture_animation},
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
    pub surface_kind: Option<crate::scene::SurfaceKind>,
    pub source: Option<Object>,
    pub faces: Vec<CachedFace>,
    pub animated_faces: Vec<usize>,
    pub animation_deadline: Option<Instant>,
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
    pub candidate_added: Vec<ItemId>,
    pub candidate_removed: Vec<ItemId>,
    pub visible: Vec<ItemId>,
    pub next_visible: Vec<ItemId>,
    pub visible_added: Vec<ItemId>,
    pub visible_removed: Vec<ItemId>,
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
    pub texture_keys: Vec<TextureKey>,
    pub texture_set: HashSet<TextureKey>,
    pub texture_face_counts: HashMap<TextureKey, usize>,
    pub visible_texture_counts: HashMap<TextureId, usize>,
    pub visible_texture_handles: HashMap<TextureId, TextureHandle>,
    pub update_items: Vec<ItemId>,
    pub visibility_slots: Vec<u32>,
    pub update_set: HashSet<ItemId>,
    pub pose_set: HashSet<ItemId>,
    pub animated_items: HashSet<ItemId>,
    pub blended_items: HashSet<ItemId>,
    pub changed_objects: HashSet<Entity>,
    pub changed_surfaces: HashSet<(Region, crate::scene::SurfaceKind)>,
    pub retired_regions: HashSet<Region>,
    pub changed_terrain_regions: HashSet<Region>,
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

impl GeometryRenderer {
    pub(super) fn update_cache(&mut self, view: &SceneView) -> CacheUpdate {
        let version = self.cache.version.clone();
        let changes = version.as_ref().and_then(|version| view.changes_since(version));
        let mut full = changes.is_none();
        let mut reconcile_all = false;
        let mut regions = Vec::new();
        self.cache.changed_objects.clear();
        self.cache.changed_surfaces.clear();
        self.cache.retired_regions.clear();
        self.cache.candidate_added.clear();
        self.cache.candidate_removed.clear();
        let mut region_invalidations = 0;
        if let Some(changes) = changes {
            for change in changes {
                full |= change.full;
                reconcile_all |= change.reconcile_all;
                region_invalidations += change.regions.len();
                regions.extend(change.regions.iter().copied());
                self.cache.changed_objects.extend(change.objects.iter().copied());
                self.cache.changed_surfaces.extend(change.surfaces.iter().copied());
                self.cache.retired_regions.extend(change.retired.iter().copied());
            }
        }
        if full {
            self.cache.items.clear();
            self.cache.objects.clear();
            self.cache.mesh_info.clear();
            self.cache.candidates.clear();
            self.cache.visible.clear();
            self.cache.next_candidates.clear();
            self.cache.next_visible.clear();
            self.cache.changed_objects.clear();
            self.cache.changed_surfaces.clear();
            self.cache.retired_regions.clear();
            self.cache.mesh_ids.clear();
            self.cache.mesh_representatives.clear();
            self.cache.mesh_compositions.clear();
            self.cache.mesh_priorities.clear();
            self.cache.mesh_ready.clear();
            self.cache.visible_texture_counts.clear();
            self.cache.visible_texture_handles.clear();
            self.cache.animated_items.clear();
            self.cache.blended_items.clear();
            self.cache.batches_valid = false;
            self.pending_meshes.clear();
            self.pending_textures.clear();
            self.cache.mesh_state = self.memory.mesh_state();
            let mut update = CacheUpdate {
                full_rebuilds: true,
                region_invalidations,
                dirtied_items: view.object_count() + view.surfaces().count(),
                full_visibility: true,
                batch_dirty: true,
                mesh_dirty: true,
                ..CacheUpdate::default()
            };
            for surface in view.surfaces() {
                if let Some(item) = self.surface_item(view, surface) {
                    self.cache.items.insert(item.id, item);
                }
            }
            for object in view.objects() {
                update.background.push(object.detail().summary.entity);
                if let Some(item) = self.object_item(object) {
                    update.objects_reprepared += 1;
                    if !item.animated_faces.is_empty() {
                        self.cache.animated_items.insert(item.id);
                    }
                    self.cache.objects.insert(object.detail().summary.entity, item.id);
                    self.cache.items.insert(item.id, item);
                }
            }
            self.cache.version = Some(view.version());
            return update;
        }

        let mut update = CacheUpdate { region_invalidations, ..CacheUpdate::default() };
        if reconcile_all {
            self.reconcile_snapshot(view, &mut update);
        } else {
            regions.sort_unstable();
            regions.dedup();
            for region in regions {
                let textures =
                    view.region(region).map(|region| region.terrain_textures().clone()).unwrap_or_else(|| std::array::from_fn(|_| None));
                for surface in view.surfaces().filter(|surface| surface.region == region && matches!(surface.kind, SurfaceKind::Terrain(_)))
                {
                    let id = surface_item_id(region, surface.kind);
                    if let Some(item) = self.cache.items.get_mut(&id)
                        && item.terrain_textures.as_ref() != Some(&textures)
                    {
                        item.terrain_textures = Some(textures.clone());
                        update.dirtied_items += 1;
                        update.texture_items.push(id);
                        if !update.terrain_regions.contains(&region) {
                            update.terrain_regions.push(region);
                        }
                    }
                }
            }
            while let Some(region) = self.cache.retired_regions.iter().next().copied() {
                self.cache.retired_regions.remove(&region);
                let removed = self
                    .cache
                    .items
                    .iter()
                    .filter(|(_, item)| item.region == region)
                    .map(|(id, item)| (*id, item.entity))
                    .collect::<Vec<_>>();
                if !removed.is_empty() {
                    update.full_visibility = true;
                    update.batch_dirty = true;
                }
                for (id, entity) in removed {
                    self.cache.items.remove(&id);
                    self.cache.animated_items.remove(&id);
                    self.cache.blended_items.remove(&id);
                    if let Some(entity) = entity {
                        self.cache.objects.remove(&entity);
                    }
                }
            }
            while let Some(entity) = self.cache.changed_objects.iter().next().copied() {
                self.cache.changed_objects.remove(&entity);
                update.background.push(entity);
                self.update_object(view.object(entity), entity, &mut update);
            }
            while let Some((region, kind)) = self.cache.changed_surfaces.iter().next().copied() {
                self.cache.changed_surfaces.remove(&(region, kind));
                self.update_surface(view, view.surface(region, kind), region, kind, &mut update);
            }
        }
        if update.full_visibility || update.batch_dirty {
            self.prune_mesh_info();
        }
        self.cache.version = Some(view.version());
        update
    }

    fn reconcile_snapshot(&mut self, view: &SceneView, update: &mut CacheUpdate) {
        let current_objects = view.objects().map(|object| object.detail().summary.entity).collect::<HashSet<_>>();
        let removed_objects = self.cache.objects.keys().filter(|entity| !current_objects.contains(entity)).copied().collect::<Vec<_>>();
        for entity in removed_objects {
            self.update_object(None, entity, update);
        }
        for object in view.objects() {
            self.update_object(Some(object), object.detail().summary.entity, update);
        }

        let current_surfaces = view.surfaces().map(|surface| surface_item_id(surface.region, surface.kind)).collect::<HashSet<_>>();
        let removed_surfaces = self
            .cache
            .items
            .iter()
            .filter_map(|(id, item)| {
                (item.surface_kind.is_some() && !current_surfaces.contains(id)).then_some((item.region, item.surface_kind?))
            })
            .collect::<Vec<_>>();
        for (region, kind) in removed_surfaces {
            self.update_surface(view, None, region, kind, update);
        }
        for surface in view.surfaces() {
            self.update_surface(view, Some(surface), surface.region, surface.kind, update);
        }
    }

    fn update_object(&mut self, object: Option<&Object>, entity: Entity, update: &mut CacheUpdate) {
        let old_id = self.cache.objects.get(&entity).copied();
        let Some(object) = object else {
            if let Some(id) = old_id {
                update.dirtied_items += 1;
                self.cache.items.remove(&id);
                self.cache.animated_items.remove(&id);
                self.cache.blended_items.remove(&id);
                self.cache.objects.remove(&entity);
                update.full_visibility = true;
                update.batch_dirty = true;
            }
            return;
        };
        let Some((_, mesh)) = object.mesh() else {
            if let Some(id) = old_id {
                update.dirtied_items += 1;
                self.cache.items.remove(&id);
                self.cache.animated_items.remove(&id);
                self.cache.blended_items.remove(&id);
                self.cache.objects.remove(&entity);
                update.full_visibility = true;
                update.batch_dirty = true;
            }
            return;
        };
        let Placement::Ready(transform) = object.placement() else {
            if let Some(id) = old_id {
                update.dirtied_items += 1;
                self.cache.items.remove(&id);
                self.cache.animated_items.remove(&id);
                self.cache.blended_items.remove(&id);
                self.cache.objects.remove(&entity);
                update.full_visibility = true;
                update.batch_dirty = true;
            }
            return;
        };
        let previous_mesh = old_id.and_then(|id| self.cache.items.get(&id).map(|cached| cached.mesh.id()));
        if let Some(id) = old_id
            && id == object_item_id(object)
            && let Some(cached) = self.cache.items.get_mut(&id)
            && cached.mesh.id() == mesh.id()
            && cached.source.as_ref().is_some_and(|previous| previous.same_render_appearance(object))
        {
            let pose_changed =
                cached.position != transform.pose.position || cached.rotation != transform.pose.rotation || cached.scale != transform.scale;
            let parameters_dirty =
                cached.source.as_ref().and_then(|previous| previous.detail().texture_animation) != object.detail().texture_animation;
            update.dirtied_items += usize::from(pose_changed || parameters_dirty);
            if parameters_dirty {
                cached.animated_faces = animated_face_indices(object, &cached.faces);
                cached.animation_deadline = (!cached.animated_faces.is_empty()).then(Instant::now);
            }
            let winding_changed = refresh_winding_pipeline(&mut cached.faces, cached.scale, transform.scale);
            cached.position = transform.pose.position;
            cached.rotation = transform.pose.rotation;
            cached.scale = transform.scale;
            update.batch_dirty |= winding_changed;
            cached.parameters_dirty |= parameters_dirty;
            cached.source = Some(object.clone());
            if !cached.animated_faces.is_empty() {
                self.cache.animated_items.insert(id);
            } else {
                self.cache.animated_items.remove(&id);
            }
            if pose_changed || parameters_dirty {
                update.pose_items.push(id);
            }
            return;
        }
        let item = self.object_item(object);
        update.dirtied_items += usize::from(item.is_some() || old_id.is_some());
        update.objects_reprepared += usize::from(item.is_some());
        if let Some(id) = old_id {
            self.cache.items.remove(&id);
            self.cache.animated_items.remove(&id);
            self.cache.blended_items.remove(&id);
        }
        if let Some(item) = item {
            let id = item.id;
            update.mesh_dirty |= previous_mesh != Some(item.mesh.id());
            if !item.animated_faces.is_empty() {
                self.cache.animated_items.insert(id);
            }
            self.cache.objects.insert(entity, id);
            self.cache.items.insert(id, item);
            update.pose_items.push(id);
            update.texture_items.push(id);
            update.batch_dirty = true;
            if old_id != Some(id) {
                update.full_visibility = true;
            }
        } else {
            self.cache.objects.remove(&entity);
            update.full_visibility = true;
            update.batch_dirty = true;
        }
    }

    fn update_surface(&mut self, view: &SceneView, surface: Option<&Surface>, region: Region, kind: SurfaceKind, update: &mut CacheUpdate) {
        let id = surface_item_id(region, kind);
        let old = self.cache.items.contains_key(&id);
        if let Some(surface) = surface
            && let Some(item) = self.surface_item(view, surface)
        {
            if let Some(previous) = self.cache.items.get_mut(&id)
                && same_surface_geometry(previous, &item)
            {
                if previous.terrain_textures != item.terrain_textures {
                    previous.terrain_textures = item.terrain_textures;
                    update.dirtied_items += 1;
                    update.texture_items.push(id);
                    if !update.terrain_regions.contains(&region) {
                        update.terrain_regions.push(region);
                    }
                }
                return;
            }
            update.dirtied_items += 1;
            update.mesh_dirty |= !old || self.cache.items.get(&id).is_some_and(|previous| previous.mesh.id() != item.mesh.id());
            self.cache.items.insert(id, item);
            update.batch_dirty = true;
            update.pose_items.push(id);
            update.texture_items.push(id);
            if !old {
                update.full_visibility = true;
            }
        } else if self.cache.items.remove(&id).is_some() {
            self.cache.blended_items.remove(&id);
            update.dirtied_items += 1;
            update.full_visibility = true;
            update.batch_dirty = true;
        }
    }

    fn object_item(&mut self, object: &Object) -> Option<CachedItem> {
        let (_, mesh) = object.mesh()?;
        let Placement::Ready(transform) = object.placement() else { return None };
        let id = object_item_id(object);
        let region = object.detail().summary.address.region();
        let info = self.cached_mesh_info(mesh);
        let materials = object.materials();
        let appearance = object.detail().surface_appearance.as_ref();
        let faces = cached_faces(id, mesh, &info, materials, appearance, transform.scale, ShadingMode::Studio);
        let animated_faces = animated_face_indices(object, &faces);
        Some(CachedItem {
            id,
            entity: Some(object.detail().summary.entity),
            region,
            mesh: mesh.clone(),
            info,
            position: transform.pose.position,
            rotation: transform.pose.rotation,
            scale: transform.scale,
            pickable: true,
            shading: ShadingMode::Studio,
            terrain_textures: None,
            terrain_composition: None,
            surface_kind: None,
            source: Some(object.clone()),
            faces,
            animation_deadline: (!animated_faces.is_empty()).then(Instant::now),
            animated_faces,
            visible_index: None,
            parameters_dirty: false,
        })
    }

    fn surface_item(&mut self, view: &SceneView, surface: &Surface) -> Option<CachedItem> {
        let PreparedMesh::Ready(mesh) = &surface.mesh else { return None };
        let id = surface_item_id(surface.region, surface.kind);
        let (scale, rotation, pickable, shading) = match surface.kind {
            SurfaceKind::Water => (Vec3::ONE, Quat::IDENTITY, false, ShadingMode::Water),
            SurfaceKind::Terrain(_) => (Vec3::ONE, Quat::IDENTITY, true, ShadingMode::Terrain),
        };
        let terrain_textures = (shading == ShadingMode::Terrain)
            .then(|| view.region(surface.region).map_or_else(|| std::array::from_fn(|_| None), |region| region.terrain_textures().clone()));
        let info = self.cached_mesh_info(mesh);
        let mut faces = cached_faces(id, mesh, &info, None, None, scale, shading);
        if shading == ShadingMode::Terrain {
            for face in &mut faces {
                face.draw.terrain = Some(surface.region);
            }
        }
        Some(CachedItem {
            id,
            entity: None,
            region: surface.region,
            mesh: mesh.clone(),
            info,
            position: surface.position,
            rotation,
            scale,
            pickable,
            shading,
            terrain_textures,
            terrain_composition: surface.terrain_composition.clone(),
            surface_kind: Some(surface.kind),
            source: None,
            faces,
            animated_faces: Vec::new(),
            animation_deadline: None,
            visible_index: None,
            parameters_dirty: false,
        })
    }

    fn cached_mesh_info(&mut self, mesh: &crate::scene::SharedMesh) -> std::sync::Arc<MeshInfo> {
        self.cache.mesh_info.entry(mesh.id()).or_insert_with(|| mesh_info(mesh)).clone()
    }

    fn prune_mesh_info(&mut self) {
        self.cache.retained_meshes.clear();
        for item in self.cache.items.values() {
            self.cache.retained_meshes.insert(item.mesh.id());
        }
        self.cache.mesh_info.retain(|id, _| self.cache.retained_meshes.contains(id));
    }

    pub(super) fn rebuild_candidates(&mut self, camera: Camera, projection: math::Mat4) -> bool {
        self.cache.next_candidates.clear();
        let frustum = Frustum::from_clip_matrix(projection);
        for item in self.cache.items.values() {
            if self.is_item_visible(item, camera, &frustum) {
                self.cache.next_candidates.push(item.id);
            }
        }
        let changed = self.cache.candidates != self.cache.next_candidates;
        if changed {
            visible_difference(
                &self.cache.candidates,
                &self.cache.next_candidates,
                &mut self.cache.candidate_added,
                &mut self.cache.candidate_removed,
            );
            std::mem::swap(&mut self.cache.candidates, &mut self.cache.next_candidates);
        }
        self.cache.next_candidates.clear();
        changed
    }

    pub(super) fn update_candidates(&mut self, camera: Camera, projection: math::Mat4, ids: &[ItemId]) -> bool {
        let frustum = Frustum::from_clip_matrix(projection);
        let mut changed = false;
        for id in ids {
            let Some(item) = self.cache.items.get(id) else { continue };
            let visible = self.is_item_visible(item, camera, &frustum);
            match self.cache.candidates.binary_search(id) {
                Ok(index) if !visible => {
                    self.cache.candidates.remove(index);
                    self.cache.candidate_removed.push(*id);
                    changed = true;
                }
                Err(index) if visible => {
                    self.cache.candidates.insert(index, *id);
                    self.cache.candidate_added.push(*id);
                    changed = true;
                }
                _ => {}
            }
        }
        changed
    }

    fn is_item_visible(&self, item: &CachedItem, camera: Camera, frustum: &Frustum) -> bool {
        is_visible(item.position, item.rotation, item.scale, item.info.bounds, camera, frustum)
    }

    pub(super) fn rebuild_visible_meshes(&mut self, camera: Camera) -> bool {
        self.cache.mesh_ids.clear();
        self.cache.mesh_set.clear();
        self.cache.mesh_representatives.clear();
        self.cache.mesh_compositions.clear();
        self.cache.mesh_priorities.clear();
        let mut missing = false;
        for id in &self.cache.candidates {
            let Some(item) = self.cache.items.get(id) else { continue };
            let mesh_id = item.mesh.id();
            let priority = draw_priority(item, camera);
            if self.cache.mesh_set.insert(mesh_id) {
                missing |= !self.meshes.contains_key(&mesh_id);
                self.cache.mesh_ids.push(mesh_id);
                self.cache.mesh_representatives.insert(mesh_id, item.mesh.clone());
                self.cache.mesh_compositions.insert(mesh_id, item.terrain_composition.clone());
                self.cache.mesh_priorities.insert(mesh_id, priority);
            } else if let Some(current) = self.cache.mesh_priorities.get_mut(&mesh_id)
                && (priority.0 > current.0 || (priority.0 == current.0 && priority.1 < current.1))
            {
                *current = priority;
            }
        }
        self.cache.mesh_ids.sort_by(|a, b| {
            let a_priority = self.cache.mesh_priorities[a];
            let b_priority = self.cache.mesh_priorities[b];
            b_priority.0.total_cmp(&a_priority.0).then_with(|| a_priority.1.total_cmp(&b_priority.1))
        });
        missing
    }
}

#[derive(Default)]
pub(super) struct CacheUpdate {
    pub(super) full_rebuilds: bool,
    pub(super) region_invalidations: usize,
    pub(super) objects_reprepared: usize,
    pub(super) dirtied_items: usize,
    pub(super) terrain_regions: Vec<Region>,
    pub(super) texture_items: Vec<ItemId>,
    pub(super) full_visibility: bool,
    pub(super) batch_dirty: bool,
    pub(super) mesh_dirty: bool,
    pub(super) background: Vec<Entity>,
    pub(super) pose_items: Vec<ItemId>,
}

pub(super) fn object_item_id(object: &Object) -> ItemId {
    (2, object.detail().summary.id.as_u128(), 0)
}

pub(super) fn visible_difference(current: &[ItemId], next: &[ItemId], added: &mut Vec<ItemId>, removed: &mut Vec<ItemId>) {
    added.clear();
    removed.clear();
    let (mut current_index, mut next_index) = (0, 0);
    while current_index < current.len() && next_index < next.len() {
        match current[current_index].cmp(&next[next_index]) {
            std::cmp::Ordering::Less => {
                removed.push(current[current_index]);
                current_index += 1;
            }
            std::cmp::Ordering::Greater => {
                added.push(next[next_index]);
                next_index += 1;
            }
            std::cmp::Ordering::Equal => {
                current_index += 1;
                next_index += 1;
            }
        }
    }
    removed.extend_from_slice(&current[current_index..]);
    added.extend_from_slice(&next[next_index..]);
}

fn animated_face_indices(object: &Object, faces: &[CachedFace]) -> Vec<usize> {
    let Some(animation) =
        object.detail().texture_animation.filter(|animation| texture_animation::is_enabled(*animation) && animation.rate() != 0.0)
    else {
        return Vec::new();
    };
    faces
        .iter()
        .enumerate()
        .filter_map(|(index, face)| (animation.targets_face(face.order.3) && face.base_params.texture.is_some()).then_some(index))
        .collect()
}

fn surface_item_id(region: Region, kind: SurfaceKind) -> ItemId {
    match kind {
        SurfaceKind::Water => (0, u128::from(region.get()), 0),
        SurfaceKind::Terrain(patch) => {
            let (x, y) = patch.coordinates();
            (1, u128::from(region.get()), u16::from(y) * 16 + u16::from(x))
        }
    }
}

pub(super) fn cached_faces(
    id: ItemId,
    mesh: &crate::scene::SharedMesh,
    info: &MeshInfo,
    materials: Option<&[FaceMaterial]>,
    appearance: Option<&world::SurfaceAppearance>,
    scale: Vec3,
    shading: ShadingMode,
) -> Vec<CachedFace> {
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
            ShadingMode::Studio => surface_pipeline(params.blend, params.double_sided, scale),
        };
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
            draw: DrawKey {
                mesh: mesh.id(),
                start: face.indices.start,
                end: face.indices.end,
                pipeline,
                texture: params.texture,
                terrain: None,
            },
            slot: None,
            included: false,
        });
    }
    cached
}

fn same_surface_geometry(left: &CachedItem, right: &CachedItem) -> bool {
    left.mesh.id() == right.mesh.id()
        && left.position == right.position
        && left.rotation == right.rotation
        && left.scale == right.scale
        && left.pickable == right.pickable
        && left.shading == right.shading
        && left.terrain_composition == right.terrain_composition
}

pub(super) fn draw_priority(item: &CachedItem, camera: Camera) -> (f64, f64) {
    let center = (item.info.bounds.min.as_dvec3() + item.info.bounds.max.as_dvec3()) * 0.5 * item.scale.as_dvec3();
    let offset = item.position - camera.position() + (item.rotation * center.as_vec3()).as_dvec3();
    let radius = ((item.info.bounds.max.as_dvec3() - item.info.bounds.min.as_dvec3()).abs() * item.scale.as_dvec3().abs() * 0.5).length();
    let distance = offset.length();
    (radius / distance.max(1.0), distance)
}

pub(super) fn surface_pipeline(blend: bool, double_sided: bool, scale: Vec3) -> Pipeline {
    match (blend, double_sided, math::flips_winding(scale)) {
        (true, true, _) => Pipeline::BlendDoubleSided,
        (true, false, true) => Pipeline::BlendFlipped,
        (true, false, false) => Pipeline::Blend,
        (false, true, _) => Pipeline::DoubleSided,
        (false, false, true) => Pipeline::FlippedWinding,
        (false, false, false) => Pipeline::Surface,
    }
}

pub(super) fn refresh_winding_pipeline(faces: &mut [CachedFace], old_scale: Vec3, scale: Vec3) -> bool {
    if math::flips_winding(old_scale) == math::flips_winding(scale) {
        return false;
    }
    for face in faces {
        face.draw.pipeline = surface_pipeline(face.base_params.blend, face.base_params.double_sided, scale);
    }
    true
}
