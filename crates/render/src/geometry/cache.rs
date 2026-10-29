use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use math::{Frustum, Quat, Vec3};
use world::{Entity, Region};

use super::instance::{instance_base, is_visible, parameter_part};
use super::items::{
    CachedFace, CachedFaceContext, CachedItem, ItemId, ItemIndex, ItemSlab, MeshInfo, cached_faces, draw_priority, mesh_info,
    same_surface_geometry,
};
use super::{GeometryRenderer, MeshState, ShadingMode};
use crate::scene::{
    Camera, MeshId, Object, Placement, PreparedMesh, SceneView, SharedMesh, Surface, SurfaceKind, ViewVersion, texture_animation,
};

#[derive(Default)]
pub(super) struct FrameCache {
    pub(super) items: ItemSlab,
    item_indices: HashMap<ItemId, ItemIndex>,
    pub(super) objects: HashMap<Entity, ItemIndex>,
    pub(super) mesh_info: HashMap<MeshId, Arc<MeshInfo>>,
    pub(super) candidates: Vec<ItemIndex>,
    pub(super) next_candidates: Vec<ItemIndex>,
    pub(super) visible: Vec<ItemIndex>,
    pub(super) next_visible: Vec<ItemIndex>,
    pub(super) mesh_ids: Vec<MeshId>,
    pub(super) mesh_set: HashSet<MeshId>,
    pub(super) mesh_priorities: HashMap<MeshId, (f64, f64)>,
    pub(super) mesh_ready: HashSet<MeshId>,
    pub(super) mesh_representatives: HashMap<MeshId, SharedMesh>,
    pub(super) mesh_compositions: HashMap<MeshId, Option<Arc<[f32]>>>,
    pub(super) retained_meshes: HashSet<MeshId>,
    pub(super) mesh_state: MeshState,
    pub(super) animated_items: HashSet<ItemIndex>,
    pub(super) patch_items: Vec<ItemIndex>,
    pub(super) patch_faces: Vec<(ItemIndex, usize)>,
    pub(super) changed_objects: HashSet<Entity>,
    pub(super) changed_surfaces: HashSet<(Region, crate::scene::SurfaceKind)>,
    pub(super) retired_regions: HashSet<Region>,
    pub(super) version: Option<ViewVersion>,
    pub(super) camera: Option<Camera>,
    pub(super) origin: math::DVec3,
    pub(super) size: [u32; 2],
}

impl FrameCache {
    pub(super) fn index(&self, id: ItemId) -> Option<ItemIndex> {
        self.item_indices.get(&id).copied()
    }

    pub(super) fn get_by_id_mut(&mut self, id: ItemId) -> Option<&mut CachedItem> {
        let index = self.index(id)?;
        self.items.get_mut(index)
    }

    pub(super) fn insert_item(&mut self, item: CachedItem) -> Option<ItemIndex> {
        let id = item.id;
        if let Some(index) = self.index(id) {
            if let Some(slot) = self.items.get_mut(index) {
                *slot = item;
                return Some(index);
            }
            self.item_indices.remove(&id);
        }
        let index = self.items.insert(item)?;
        self.item_indices.insert(id, index);
        Some(index)
    }

    pub(super) fn remove_item(&mut self, index: ItemIndex) -> Option<CachedItem> {
        let item = self.items.remove(index)?;
        self.item_indices.remove(&item.id);
        Some(item)
    }

    pub(super) fn clear_items(&mut self) {
        self.items.clear();
        self.item_indices.clear();
    }
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
        if let Some(changes) = changes {
            for change in changes {
                full |= change.full;
                reconcile_all |= change.reconcile_all;
                regions.extend(change.regions.iter().copied());
                self.cache.changed_objects.extend(change.objects.iter().copied());
                self.cache.changed_surfaces.extend(change.surfaces.iter().copied());
                self.cache.retired_regions.extend(change.retired.iter().copied());
            }
        }
        if full {
            self.cache.clear_items();
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
            self.cache.animated_items.clear();
            self.draw_list.clear_groups();
            self.pending_meshes.clear();
            self.pending_textures.clear();
            self.cache.mesh_state = self.memory.mesh_state();
            let mut update = CacheUpdate {
                layout_changed: true,
                items_updated: view.object_count() + view.surfaces().count(),
                full_visibility: true,
                mesh_dirty: true,
                ..CacheUpdate::default()
            };
            for surface in view.surfaces() {
                if let Some(item) = self.surface_item(view, surface) {
                    self.cache.insert_item(item);
                }
            }
            for object in view.objects() {
                update.background.push(object.detail().summary.entity);
                if let Some(item) = self.object_item(object)
                    && let Some(index) = self.cache.insert_item(item)
                {
                    if self.cache.items.get(index).is_some_and(|item| !item.animated_faces.is_empty()) {
                        self.cache.animated_items.insert(index);
                    }
                    self.cache.objects.insert(object.detail().summary.entity, index);
                }
            }
            self.cache.version = Some(view.version());
            return update;
        }

        let mut update = CacheUpdate::default();
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
                    if let Some(item) = self.cache.get_by_id_mut(id)
                        && item.terrain_textures.as_ref() != Some(&textures)
                    {
                        item.terrain_textures = Some(textures.clone());
                        self.draw_list.update_terrain(region, &textures);
                        update.items_updated += 1;
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
                    .map(|(index, item)| (index, item.entity))
                    .collect::<Vec<_>>();
                if !removed.is_empty() {
                    update.full_visibility = true;
                    update.layout_changed = true;
                }
                for (index, entity) in removed {
                    self.remove_item(index);
                    self.cache.animated_items.remove(&index);
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
        if update.full_visibility || update.mesh_dirty {
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
            .filter_map(|(_, item)| {
                (item.surface_kind.is_some() && !current_surfaces.contains(&item.id)).then_some((item.region, item.surface_kind?))
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
        let old_index = self.cache.objects.get(&entity).copied();
        let old_id = old_index.and_then(|index| self.cache.items.get(index).map(|item| item.id));
        let Some(object) = object else {
            if let Some(index) = old_index {
                update.items_updated += 1;
                self.remove_item(index);
                self.cache.animated_items.remove(&index);
                self.cache.objects.remove(&entity);
                update.full_visibility = true;
                update.layout_changed = true;
            }
            return;
        };
        let Some((_, mesh)) = object.mesh() else {
            if let Some(index) = old_index {
                update.items_updated += 1;
                self.remove_item(index);
                self.cache.animated_items.remove(&index);
                self.cache.objects.remove(&entity);
                update.full_visibility = true;
                update.layout_changed = true;
            }
            return;
        };
        let Placement::Ready(transform) = object.placement() else {
            if let Some(index) = old_index {
                update.items_updated += 1;
                self.remove_item(index);
                self.cache.animated_items.remove(&index);
                self.cache.objects.remove(&entity);
                update.full_visibility = true;
                update.layout_changed = true;
            }
            return;
        };
        let previous_mesh = old_index.and_then(|index| self.cache.items.get(index).map(|cached| cached.mesh.id()));
        if let Some(index) = old_index
            && old_id == Some(object_item_id(object))
            && let Some(cached) = self.cache.items.get_mut(index)
            && cached.mesh.id() == mesh.id()
            && cached.source.as_ref().is_some_and(|previous| previous.same_render_appearance(object))
        {
            let pose_changed =
                cached.position != transform.pose.position || cached.rotation != transform.pose.rotation || cached.scale != transform.scale;
            let parameters_dirty =
                cached.source.as_ref().and_then(|previous| previous.detail().texture_animation) != object.detail().texture_animation;
            update.items_updated += usize::from(pose_changed || parameters_dirty);
            if parameters_dirty {
                cached.animated_faces = animated_face_indices(object, &cached.faces);
                cached.animation_deadline = (!cached.animated_faces.is_empty()).then(Instant::now);
            }
            let scale_changed = cached.scale != transform.scale;
            let winding_changed = self.draw_list.refresh_winding(&mut cached.faces, cached.scale, transform.scale);
            if pose_changed {
                cached.transform = instance_base(transform.pose.position, transform.pose.rotation, transform.scale, self.cache.origin);
                if scale_changed {
                    for face in &mut cached.faces {
                        face.parameter_bytes = parameter_part(&face.params, transform.scale);
                    }
                }
            }
            cached.position = transform.pose.position;
            cached.rotation = transform.pose.rotation;
            cached.scale = transform.scale;
            update.layout_changed |= winding_changed;
            cached.parameters_dirty |= parameters_dirty;
            cached.source = Some(object.clone());
            if !cached.animated_faces.is_empty() {
                self.cache.animated_items.insert(index);
            } else {
                self.cache.animated_items.remove(&index);
            }
            if pose_changed {
                update.pose_items.push(index);
            }
            if parameters_dirty {
                update.parameter_items.push(index);
            }
            return;
        }
        let item = self.object_item(object);
        update.items_updated += usize::from(item.is_some() || old_id.is_some());
        if let Some(index) = old_index {
            self.remove_item(index);
            self.cache.animated_items.remove(&index);
        }
        if let Some(item) = item {
            let id = item.id;
            update.mesh_dirty |= previous_mesh != Some(item.mesh.id());
            if let Some(index) = self.cache.insert_item(item) {
                if self.cache.items.get(index).is_some_and(|item| !item.animated_faces.is_empty()) {
                    self.cache.animated_items.insert(index);
                }
                self.cache.objects.insert(entity, index);
            }
            update.layout_changed = true;
            if old_id != Some(id) {
                update.full_visibility = true;
            }
        } else {
            self.cache.objects.remove(&entity);
            update.full_visibility = true;
            update.layout_changed = true;
        }
    }

    fn update_surface(&mut self, view: &SceneView, surface: Option<&Surface>, region: Region, kind: SurfaceKind, update: &mut CacheUpdate) {
        let id = surface_item_id(region, kind);
        let old_index = self.cache.index(id);
        if let Some(surface) = surface
            && let Some(item) = self.surface_item(view, surface)
        {
            if let Some(previous) = old_index.and_then(|index| self.cache.items.get_mut(index))
                && same_surface_geometry(previous, &item)
            {
                if previous.terrain_textures != item.terrain_textures {
                    previous.terrain_textures = item.terrain_textures;
                    if let Some(textures) = previous.terrain_textures.as_ref() {
                        self.draw_list.update_terrain(region, textures);
                    }
                    update.items_updated += 1;
                    if !update.terrain_regions.contains(&region) {
                        update.terrain_regions.push(region);
                    }
                }
                self.draw_list.release_faces(&item.faces);
                return;
            }
            update.items_updated += 1;
            update.mesh_dirty |=
                old_index.is_none_or(|index| self.cache.items.get(index).is_none_or(|previous| previous.mesh.id() != item.mesh.id()));
            if let Some(index) = old_index {
                self.remove_item(index);
            }
            self.cache.insert_item(item);
            update.layout_changed = true;
            if old_index.is_none() {
                update.full_visibility = true;
            }
        } else if let Some(index) = old_index {
            self.remove_item(index);
            update.items_updated += 1;
            update.full_visibility = true;
            update.layout_changed = true;
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
        let faces = cached_faces(
            CachedFaceContext {
                id,
                mesh,
                info: &info,
                materials,
                appearance,
                scale: transform.scale,
                shading: ShadingMode::Studio,
                terrain: None,
                terrain_textures: None,
            },
            &mut self.draw_list,
        );
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
            transform: instance_base(transform.pose.position, transform.pose.rotation, transform.scale, self.cache.origin),
            pickable: true,
            shading: ShadingMode::Studio,
            terrain_textures: None,
            terrain_composition: None,
            surface_kind: None,
            source: Some(object.clone()),
            faces,
            animation_deadline: (!animated_faces.is_empty()).then(Instant::now),
            animated_faces,
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
        let terrain_region = (shading == ShadingMode::Terrain).then_some(surface.region);
        let faces = cached_faces(
            CachedFaceContext {
                id,
                mesh,
                info: &info,
                materials: None,
                appearance: None,
                scale,
                shading,
                terrain: terrain_region,
                terrain_textures: terrain_textures.as_ref(),
            },
            &mut self.draw_list,
        );
        Some(CachedItem {
            id,
            entity: None,
            region: surface.region,
            mesh: mesh.clone(),
            info,
            position: surface.position,
            rotation,
            scale,
            transform: instance_base(surface.position, rotation, scale, self.cache.origin),
            pickable,
            shading,
            terrain_textures,
            terrain_composition: surface.terrain_composition.clone(),
            surface_kind: Some(surface.kind),
            source: None,
            faces,
            animated_faces: Vec::new(),
            animation_deadline: None,
            parameters_dirty: false,
        })
    }

    fn cached_mesh_info(&mut self, mesh: &crate::scene::SharedMesh) -> std::sync::Arc<MeshInfo> {
        self.cache.mesh_info.entry(mesh.id()).or_insert_with(|| mesh_info(mesh)).clone()
    }

    fn prune_mesh_info(&mut self) {
        self.cache.retained_meshes.clear();
        for (_, item) in self.cache.items.iter() {
            self.cache.retained_meshes.insert(item.mesh.id());
        }
        self.cache.mesh_info.retain(|id, _| self.cache.retained_meshes.contains(id));
    }

    fn remove_item(&mut self, index: ItemIndex) -> Option<CachedItem> {
        let item = self.cache.remove_item(index)?;
        self.draw_list.release_faces(&item.faces);
        Some(item)
    }

    pub(super) fn rebuild_candidates(&mut self, camera: Camera, projection: math::Mat4) -> bool {
        self.cache.next_candidates.clear();
        let frustum = Frustum::from_clip_matrix(projection);
        for (index, item) in self.cache.items.iter() {
            if self.is_item_visible(item, camera, &frustum) {
                self.cache.next_candidates.push(index);
            }
        }
        self.cache.next_candidates.sort_unstable_by_key(|index| self.cache.items.get(*index).map(|item| item.id));
        let changed = self.cache.candidates != self.cache.next_candidates;
        if changed {
            std::mem::swap(&mut self.cache.candidates, &mut self.cache.next_candidates);
        }
        self.cache.next_candidates.clear();
        changed
    }

    pub(super) fn update_candidates(&mut self, camera: Camera, projection: math::Mat4, ids: &[ItemIndex]) -> bool {
        let frustum = Frustum::from_clip_matrix(projection);
        let mut changed = false;
        for item_index in ids {
            let Some(item) = self.cache.items.get(*item_index) else { continue };
            let visible = self.is_item_visible(item, camera, &frustum);
            let id = item.id;
            let position = self
                .cache
                .candidates
                .binary_search_by(|candidate| self.cache.items.get(*candidate).map_or(Ordering::Less, |item| item.id.cmp(&id)));
            match position {
                Ok(index) if !visible => {
                    self.cache.candidates.remove(index);
                    changed = true;
                }
                Err(position) if visible => {
                    self.cache.candidates.insert(position, *item_index);
                    changed = true;
                }
                _ => {}
            }
        }
        changed
    }

    fn is_item_visible(&self, item: &CachedItem, camera: Camera, frustum: &Frustum) -> bool {
        is_visible(item.position, item.rotation, item.scale, item.info.bounds, item.shading, camera, frustum)
    }

    pub(super) fn rebuild_visible_meshes(&mut self, camera: Camera) -> bool {
        self.cache.mesh_ids.clear();
        self.cache.mesh_set.clear();
        self.cache.mesh_representatives.clear();
        self.cache.mesh_compositions.clear();
        self.cache.mesh_priorities.clear();
        let mut missing = false;
        for id in &self.cache.candidates {
            let Some(item) = self.cache.items.get(*id) else { continue };
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
    pub(super) layout_changed: bool,
    pub(super) items_updated: usize,
    pub(super) terrain_regions: Vec<Region>,
    pub(super) full_visibility: bool,
    pub(super) mesh_dirty: bool,
    pub(super) background: Vec<Entity>,
    pub(super) pose_items: Vec<ItemIndex>,
    pub(super) parameter_items: Vec<ItemIndex>,
}

pub(super) fn object_item_id(object: &Object) -> ItemId {
    (2, object.detail().summary.id.as_u128(), 0)
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
