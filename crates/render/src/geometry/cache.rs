use std::{
    cmp::Ordering,
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use math::{Frustum, Quat, Vec3};
use world::{Entity, Region};

use super::instance::{instance_base, is_visible, parameter_part};
use super::items::{
    CachedFace, CachedFaceContext, CachedItem, CachedReplacement, ItemId, ItemIndex, ItemSlab, MeshInfo, cached_faces, draw_priority,
    mesh_info, object_draw_scale, same_surface_geometry,
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
    pub(super) replacements: BTreeSet<ItemIndex>,
    pub(super) mesh_info: HashMap<MeshId, Arc<MeshInfo>>,
    pub(super) candidates: Vec<ItemIndex>,
    pub(super) next_candidates: Vec<ItemIndex>,
    pub(super) replacement_visible: HashSet<ItemIndex>,
    pub(super) next_replacement_visible: HashSet<ItemIndex>,
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
        self.replacements.remove(&index);
        self.replacement_visible.remove(&index);
        self.next_replacement_visible.remove(&index);
        Some(item)
    }

    pub(super) fn clear_items(&mut self) {
        self.items.clear();
        self.item_indices.clear();
        self.replacements.clear();
        self.replacement_visible.clear();
        self.next_replacement_visible.clear();
    }
}

impl GeometryRenderer {
    pub(super) fn update_cache(&mut self, view: &SceneView) -> CacheUpdate {
        let version = self.cache.version.clone();
        let changes = version.as_ref().and_then(|version| view.changes_since(version));
        let mut full = changes.is_none();
        let mut reconcile_all = false;
        let mut regions = Vec::new();
        let mut motion_changed = HashSet::new();
        self.cache.changed_objects.clear();
        self.cache.changed_surfaces.clear();
        self.cache.retired_regions.clear();
        if let Some(changes) = changes {
            for change in changes {
                full |= change.full;
                reconcile_all |= change.reconcile_all;
                regions.extend(change.regions.iter().copied());
                motion_changed.extend(change.motion_changed.iter().copied());
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
            self.cache.replacement_visible.clear();
            self.cache.next_replacement_visible.clear();
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

        if reconcile_all {
            motion_changed.extend(view.objects().map(|object| object.detail().summary.entity));
        }
        let mut update = CacheUpdate { motion_changed, ..CacheUpdate::default() };
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
        if !object.mesh_matches_geometry() {
            if let Some(index) = old_index {
                self.discard_replacement(index);
                self.update_held_object_pose(index, transform, update);
            }
            return;
        }
        let previous_mesh = old_index.and_then(|index| self.cache.items.get(index).map(|cached| cached.mesh.id()));
        let unchanged = old_index.is_some_and(|index| {
            self.cache.items.get(index).is_some_and(|cached| {
                cached.id == object_item_id(object)
                    && cached.mesh.id() == mesh.id()
                    && cached.source.as_ref().is_some_and(|previous| previous.same_render_appearance(object))
            })
        });
        if unchanged && let Some(index) = old_index {
            self.discard_replacement(index);
            let Some(cached) = self.cache.items.get_mut(index) else { return };
            let flags = object.mesh_flags().unwrap_or_default();
            let scale = object_draw_scale(object, transform.scale);
            let pose_changed =
                cached.position != transform.pose.position || cached.rotation != transform.pose.rotation || cached.scale != scale;
            let parameters_dirty =
                cached.source.as_ref().and_then(|previous| previous.detail().texture_animation) != object.detail().texture_animation;
            update.items_updated += usize::from(pose_changed || parameters_dirty);
            if parameters_dirty {
                cached.animated_faces = animated_face_indices(object, &cached.faces);
                cached.animation_deadline = (!cached.animated_faces.is_empty()).then(Instant::now);
            }
            let scale_changed = cached.scale != scale;
            let winding_changed =
                self.draw_list.refresh_winding(&mut cached.faces, cached.scale, cached.invert_normals, scale, flags.invert);
            if pose_changed {
                cached.transform = instance_base(transform.pose.position, transform.pose.rotation, scale, flags.invert, self.cache.origin);
                if scale_changed {
                    for face in &mut cached.faces {
                        face.parameter_bytes = parameter_part(&face.params, scale);
                    }
                }
            }
            cached.position = transform.pose.position;
            cached.rotation = transform.pose.rotation;
            cached.scale = scale;
            cached.invert_normals = flags.invert;
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
        let Some(item) = self.object_item(object) else {
            if let Some(index) = old_index {
                self.remove_item(index);
                self.cache.animated_items.remove(&index);
            }
            self.cache.objects.remove(&entity);
            update.full_visibility = true;
            update.layout_changed = true;
            return;
        };
        if let Some(index) = old_index
            && previous_mesh != Some(item.mesh.id())
            && !self.meshes.contains_key(&item.mesh.id())
        {
            self.discard_replacement(index);
            if let Some(cached) = self.cache.items.get_mut(index) {
                cached.replacement = Some(Box::new(CachedReplacement { item, retry_state: None }));
                self.cache.replacements.insert(index);
            }
            self.update_held_object_pose(index, transform, update);
            update.items_updated += 1;
            update.mesh_dirty = true;
            update.full_visibility = true;
            return;
        }
        update.items_updated += 1;
        if let Some(index) = old_index {
            self.remove_item(index);
            self.cache.animated_items.remove(&index);
        }
        let id = item.id;
        update.mesh_dirty |= previous_mesh != Some(item.mesh.id()) || old_index.is_some();
        if let Some(index) = self.cache.insert_item(item) {
            if self.cache.items.get(index).is_some_and(|item| !item.animated_faces.is_empty()) {
                self.cache.animated_items.insert(index);
            }
            self.cache.objects.insert(entity, index);
            if old_id == Some(id) {
                update.pose_items.push(index);
            }
        }
        update.layout_changed = true;
        if old_id != Some(id) {
            update.full_visibility = true;
        }
    }

    fn discard_replacement(&mut self, index: ItemIndex) {
        let replacement = self.cache.items.get_mut(index).and_then(|item| item.replacement.take());
        if let Some(replacement) = replacement {
            self.cache.replacements.remove(&index);
            self.draw_list.release_faces(&replacement.item.faces);
        }
    }

    fn update_held_object_pose(&mut self, index: ItemIndex, transform: world::WorldTransform, update: &mut CacheUpdate) {
        let Some(cached) = self.cache.items.get_mut(index) else { return };
        let mirror = cached.source.as_ref().and_then(Object::mesh_flags).is_some_and(|flags| flags.mirror);
        let scale = super::items::draw_scale(transform.scale, mirror);
        let pose_changed =
            cached.position != transform.pose.position || cached.rotation != transform.pose.rotation || cached.scale != scale;
        let scale_changed = cached.scale != scale;
        let winding_changed =
            self.draw_list.refresh_winding(&mut cached.faces, cached.scale, cached.invert_normals, scale, cached.invert_normals);
        if pose_changed {
            cached.transform =
                instance_base(transform.pose.position, transform.pose.rotation, scale, cached.invert_normals, self.cache.origin);
            if scale_changed {
                for face in &mut cached.faces {
                    face.parameter_bytes = parameter_part(&face.params, scale);
                }
            }
        }
        cached.position = transform.pose.position;
        cached.rotation = transform.pose.rotation;
        cached.scale = scale;
        cached.parameters_dirty |= scale_changed;
        update.items_updated += usize::from(pose_changed);
        update.layout_changed |= winding_changed;
        if pose_changed {
            update.pose_items.push(index);
        }
    }

    fn replacement(&self, index: ItemIndex) -> Option<&CachedReplacement> {
        self.cache.items.get(index)?.replacement.as_deref()
    }

    pub(super) fn replacement_groups(&self) -> HashMap<MeshId, Vec<ItemIndex>> {
        let mut groups = HashMap::<MeshId, Vec<ItemIndex>>::new();
        for index in &self.cache.replacements {
            if let Some(replacement) = self.replacement(*index) {
                groups.entry(replacement.item.mesh.id()).or_default().push(*index);
            }
        }
        groups
    }

    pub(super) fn set_replacement_retry_state(&mut self, indices: &[ItemIndex], state: MeshState) {
        for index in indices {
            if let Some(replacement) = self.cache.items.get_mut(*index).and_then(|item| item.replacement.as_mut()) {
                replacement.retry_state = Some(state);
            }
        }
    }

    pub(super) fn replacement_retry_state(&self, indices: &[ItemIndex]) -> Option<MeshState> {
        indices.iter().find_map(|index| self.replacement(*index)?.retry_state)
    }

    pub(super) fn has_ready_replacement(&self) -> bool {
        self.cache
            .replacements
            .iter()
            .any(|index| self.replacement(*index).is_some_and(|replacement| self.meshes.contains_key(&replacement.item.mesh.id())))
    }

    pub(super) fn promote_ready_replacements(&mut self, update: &mut CacheUpdate) -> bool {
        let indices = self
            .cache
            .replacements
            .iter()
            .copied()
            .filter(|index| self.replacement(*index).is_some_and(|replacement| self.meshes.contains_key(&replacement.item.mesh.id())))
            .collect::<Vec<_>>();
        self.promote_replacements(indices, update)
    }

    pub(super) fn drop_blocked_fallbacks(&mut self, indices: &[ItemIndex], update: &mut CacheUpdate) -> Vec<MeshId> {
        let previous_meshes =
            indices.iter().filter_map(|index| self.cache.items.get(*index).map(|item| item.mesh.id())).collect::<Vec<_>>();
        if self.promote_replacements(indices.to_vec(), update) { previous_meshes } else { Vec::new() }
    }

    fn promote_replacements(&mut self, indices: Vec<ItemIndex>, update: &mut CacheUpdate) -> bool {
        let mut changed = false;
        for index in indices {
            let replacement = self.cache.items.get_mut(index).and_then(|item| item.replacement.take()).map(|replacement| *replacement);
            let Some(replacement) = replacement else { continue };
            self.cache.replacements.remove(&index);
            self.cache.replacement_visible.remove(&index);
            self.cache.next_replacement_visible.remove(&index);
            let Some(item) = self.cache.items.get_mut(index) else { continue };
            let previous = std::mem::replace(item, replacement.item);
            self.draw_list.release_faces(&previous.faces);
            self.cache.animated_items.remove(&index);
            if self.cache.items.get(index).is_some_and(|item| !item.animated_faces.is_empty()) {
                self.cache.animated_items.insert(index);
            }
            update.items_updated += 1;
            update.full_visibility = true;
            update.layout_changed = true;
            update.mesh_dirty = true;
            changed = true;
        }
        if changed {
            self.prune_mesh_info();
        }
        changed
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
        if !object.mesh_matches_geometry() {
            return None;
        }
        let (_, mesh) = object.mesh()?;
        let Placement::Ready(transform) = object.placement() else { return None };
        let flags = object.mesh_flags().unwrap_or_default();
        let scale = object_draw_scale(object, transform.scale);
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
                scale,
                invert_normals: flags.invert,
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
            scale,
            invert_normals: flags.invert,
            replacement: None,
            transform: instance_base(transform.pose.position, transform.pose.rotation, scale, flags.invert, self.cache.origin),
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
            SurfaceKind::RegionWater => (Vec3::ONE, Quat::IDENTITY, false, ShadingMode::RegionWater),
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
                invert_normals: false,
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
            invert_normals: false,
            replacement: None,
            transform: instance_base(surface.position, rotation, scale, false, self.cache.origin),
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

    pub(super) fn prune_mesh_info(&mut self) {
        self.cache.retained_meshes.clear();
        for (_, item) in self.cache.items.iter() {
            self.cache.retained_meshes.insert(item.mesh.id());
            if let Some(replacement) = item.replacement.as_ref() {
                self.cache.retained_meshes.insert(replacement.item.mesh.id());
            }
        }
        self.cache.mesh_info.retain(|id, _| self.cache.retained_meshes.contains(id));
    }

    fn remove_item(&mut self, index: ItemIndex) -> Option<CachedItem> {
        let item = self.cache.remove_item(index)?;
        self.cache.replacements.remove(&index);
        self.draw_list.release_faces(&item.faces);
        if let Some(replacement) = item.replacement.as_ref() {
            self.draw_list.release_faces(&replacement.item.faces);
        }
        Some(item)
    }

    pub(super) fn rebuild_candidates(&mut self, camera: Camera, projection: math::Mat4) -> (bool, bool) {
        self.cache.next_candidates.clear();
        self.cache.next_replacement_visible.clear();
        let frustum = Frustum::from_clip_matrix(projection);
        for (index, item) in self.cache.items.iter() {
            if self.is_item_visible(item, camera, &frustum) {
                self.cache.next_candidates.push(index);
            }
            if item.replacement.as_ref().is_some_and(|replacement| self.is_item_visible(&replacement.item, camera, &frustum)) {
                self.cache.next_replacement_visible.insert(index);
            }
        }
        self.cache.next_candidates.sort_unstable_by_key(|index| self.cache.items.get(*index).map(|item| item.id));
        let candidates_changed = self.cache.candidates != self.cache.next_candidates;
        if candidates_changed {
            std::mem::swap(&mut self.cache.candidates, &mut self.cache.next_candidates);
        }
        let replacements_changed = self.cache.replacement_visible != self.cache.next_replacement_visible;
        if replacements_changed {
            std::mem::swap(&mut self.cache.replacement_visible, &mut self.cache.next_replacement_visible);
        }
        self.cache.next_candidates.clear();
        self.cache.next_replacement_visible.clear();
        (candidates_changed, replacements_changed)
    }

    pub(super) fn update_candidates(&mut self, camera: Camera, projection: math::Mat4, ids: &[ItemIndex]) -> (bool, bool) {
        let frustum = Frustum::from_clip_matrix(projection);
        let mut candidates_changed = false;
        let mut replacements_changed = false;
        for item_index in ids {
            let Some(item) = self.cache.items.get(*item_index) else { continue };
            let visible = self.is_item_visible(item, camera, &frustum);
            let replacement_visible =
                item.replacement.as_ref().is_some_and(|replacement| self.is_item_visible(&replacement.item, camera, &frustum));
            let id = item.id;
            let was_replacement_visible = self.cache.replacement_visible.contains(item_index);
            if replacement_visible != was_replacement_visible {
                if replacement_visible {
                    self.cache.replacement_visible.insert(*item_index);
                } else {
                    self.cache.replacement_visible.remove(item_index);
                }
                replacements_changed = true;
            }
            let position = self
                .cache
                .candidates
                .binary_search_by(|candidate| self.cache.items.get(*candidate).map_or(Ordering::Less, |item| item.id.cmp(&id)));
            match position {
                Ok(index) if !visible => {
                    self.cache.candidates.remove(index);
                    candidates_changed = true;
                }
                Err(position) if visible => {
                    self.cache.candidates.insert(position, *item_index);
                    candidates_changed = true;
                }
                _ => {}
            }
        }
        (candidates_changed, replacements_changed)
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
        let candidates = self.cache.candidates.clone();
        for id in candidates {
            let Some(item) = self.cache.items.get(id) else { continue };
            let mesh = item.mesh.clone();
            let composition = item.terrain_composition.clone();
            let priority = draw_priority(item, camera);
            let replacement = item.replacement.as_ref().map(|replacement| {
                let replacement = &replacement.item;
                (
                    replacement.mesh.clone(),
                    replacement.terrain_composition.clone(),
                    draw_priority(replacement, camera),
                    self.meshes.contains_key(&replacement.mesh.id()),
                )
            });
            missing |= add_mesh_demand(&mut self.cache, &mesh, composition, priority, self.meshes.contains_key(&mesh.id()));
            if let Some((mesh, composition, priority, resident)) = replacement {
                missing |= add_mesh_demand(&mut self.cache, &mesh, composition, priority, resident);
            }
        }
        if !self.cache.replacement_visible.is_empty() {
            let replacements = self
                .cache
                .replacement_visible
                .iter()
                .filter_map(|index| {
                    let item = self.cache.items.get(*index)?;
                    let replacement = &item.replacement.as_ref()?.item;
                    Some((replacement.mesh.clone(), replacement.terrain_composition.clone(), draw_priority(replacement, camera)))
                })
                .collect::<Vec<_>>();
            for (mesh, composition, priority) in replacements {
                missing |= add_mesh_demand(&mut self.cache, &mesh, composition, priority, self.meshes.contains_key(&mesh.id()));
            }
        }
        self.cache.mesh_ids.sort_by(|a, b| {
            let resident = self.meshes.contains_key(b).cmp(&self.meshes.contains_key(a));
            if resident != std::cmp::Ordering::Equal {
                return resident;
            }
            let a_priority = self.cache.mesh_priorities[a];
            let b_priority = self.cache.mesh_priorities[b];
            b_priority.0.total_cmp(&a_priority.0).then_with(|| a_priority.1.total_cmp(&b_priority.1))
        });
        missing
    }

    pub(super) fn forget_mesh_demand(&mut self, mesh_id: MeshId) {
        self.cache.mesh_ids.retain(|candidate| *candidate != mesh_id);
        self.cache.mesh_set.remove(&mesh_id);
        self.cache.mesh_priorities.remove(&mesh_id);
        self.cache.mesh_ready.remove(&mesh_id);
        self.cache.mesh_representatives.remove(&mesh_id);
        self.cache.mesh_compositions.remove(&mesh_id);
        self.pending_meshes.remove(&mesh_id);
    }
}

fn add_mesh_demand(
    cache: &mut FrameCache,
    mesh: &SharedMesh,
    composition: Option<Arc<[f32]>>,
    priority: (f64, f64),
    resident: bool,
) -> bool {
    let mesh_id = mesh.id();
    if cache.mesh_set.insert(mesh_id) {
        cache.mesh_ids.push(mesh_id);
        cache.mesh_representatives.insert(mesh_id, mesh.clone());
        cache.mesh_compositions.insert(mesh_id, composition);
        cache.mesh_priorities.insert(mesh_id, priority);
        !resident
    } else {
        if let Some(current) = cache.mesh_priorities.get_mut(&mesh_id)
            && (priority.0 > current.0 || (priority.0 == current.0 && priority.1 < current.1))
        {
            *current = priority;
        }
        false
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
    pub(super) motion_changed: HashSet<Entity>,
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
        SurfaceKind::RegionWater => (0, u128::from(region.get()), 0),
        SurfaceKind::Terrain(patch) => {
            let (x, y) = patch.coordinates();
            (1, u128::from(region.get()), u16::from(y) * 16 + u16::from(x))
        }
    }
}
