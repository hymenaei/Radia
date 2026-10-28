use std::time::{Duration, Instant};

use assets::{Priority, TextureHandle};
use math::{DVec3, Frustum, Quat, Vec3};
use wgpu::{CommandEncoder, Device};
use world::{Entity, FaceAppearance, Region};

use super::{FramePreparation, FramePreparationKey, GeometryRenderer, PrepareTimes, ShadingMode, Stats};
use crate::geometry::{
    batching::{DrawKey, MAX_FACE_INSTANCES},
    cache::{CachedFace, CachedItem, ItemId, MeshInfo, mesh_info},
    instance::{instance_base, is_visible, packed_instance, scene_origin},
};
use crate::material::{FaceMaterial, FaceParams};
use crate::pipeline::{COLOR, Pipeline};
use crate::scene::{Camera, Object, Placement, PreparedMesh, SceneView, Surface, SurfaceKind, texture_animation};

impl GeometryRenderer {
    pub(crate) fn prepare(
        &mut self,
        device: &Device,
        encoder: &mut CommandEncoder,
        view: &SceneView,
        camera: Camera,
        size: [u32; 2],
        water_height: Option<f64>,
    ) {
        let started = Instant::now();
        let animation_now = started;
        let mut times = PrepareTimes::default();
        let key = FramePreparationKey { version: view.version(), camera, size, water_height };
        let same = self.prepared.as_ref().is_some_and(|prepared| prepared.key == key);
        if same {
            let upload_started = Instant::now();
            self.textures.upload(device, encoder, self.epoch);
            times.textures = upload_started.elapsed();
            if self.prepared.as_ref().is_some_and(|prepared| prepared.reusable(&key, self.textures.state(), self.memory.mesh_state())) {
                self.stats.uploaded_bytes = 0;
                self.memory_stats();
                self.record_prepare(started, times, 0);
                return;
            }
        }

        let visibility_started = Instant::now();
        self.epoch = self.epoch.checked_add(1).expect("render frame space exhausted");
        self.stats = Stats::default();
        let previous_camera = self.cache.camera;
        let camera_changed = previous_camera != Some(camera) || self.cache.size != size;
        let camera_position_changed = previous_camera.is_none_or(|previous| previous.position() != camera.position());
        let origin = scene_origin(camera.position());
        let transform_changed = previous_camera.is_none() || self.cache.origin != origin;
        let mut update = self.update_cache(view);
        let projection = self.pipelines.update_camera_uniform(&self.queue, camera, size, origin);
        self.water.update(&self.queue, view, camera, projection, water_height);
        let candidates_changed = if update.full_visibility || camera_changed {
            self.rebuild_candidates(camera, projection)
        } else if !update.pose_items.is_empty() {
            self.update_candidates(camera, projection, &update.pose_items)
        } else {
            false
        };
        if candidates_changed {
            update.batch_dirty = true;
            self.rebuild_visible_textures();
        } else if update.batch_dirty {
            self.rebuild_visible_textures();
        }
        times.visibility = visibility_started.elapsed();

        let requests_started = Instant::now();
        self.request_visible_textures();
        for entity in update.background {
            if let Some(object) = view.object(entity) {
                self.request_background_textures(object);
            }
        }
        times.requests = requests_started.elapsed();

        if candidates_changed || update.mesh_dirty {
            self.rebuild_visible_meshes(camera);
        }
        let mesh_progress =
            candidates_changed || update.mesh_dirty || self.cache.mesh_state != self.memory.mesh_state() || !self.pending_meshes.is_empty();
        let mut uploaded = 0;
        if mesh_progress {
            self.mesh_budget_blocked.clear();
            self.pending_meshes.clear();
            self.cache.mesh_ready.clear();
            for index in 0..self.cache.mesh_ids.len() {
                let mesh_id = self.cache.mesh_ids[index];
                let Some(mesh) = self.cache.mesh_representatives.get(&mesh_id).cloned() else { continue };
                let composition = self.cache.mesh_compositions.get(&mesh_id).cloned().flatten();
                if self.upload_mesh(device, &mesh, composition.as_deref(), &mut uploaded) {
                    self.cache.mesh_ready.insert(mesh_id);
                } else {
                    self.pending_meshes.insert(mesh_id);
                }
            }
            self.cache.next_visible.clear();
            for id in &self.cache.candidates {
                let Some(item) = self.cache.items.get(id) else { continue };
                if self.cache.mesh_ready.contains(&item.mesh.id()) && !item.faces.is_empty() {
                    self.cache.next_visible.push(*id);
                }
            }
            self.cache.mesh_state = self.memory.mesh_state();
        }

        let upload_started = Instant::now();
        if !same {
            self.textures.upload(device, encoder, self.epoch);
        }
        times.textures += upload_started.elapsed();
        let texture_state = self.textures.state();
        let visible_changed = mesh_progress && self.cache.visible != self.cache.next_visible;
        let batches_started = Instant::now();
        let rebuild_batches =
            !self.cache.batches_valid || update.batch_dirty || visible_changed || self.cache.texture_state != texture_state;
        let (animated_faces, draw_commands_changed) = if rebuild_batches {
            self.refresh_texture_bindings(device, mesh_progress);
            let animated = self.rebuild_batches(camera, origin, animation_now, mesh_progress);
            self.rebuild_pick_instances(mesh_progress);
            (animated, true)
        } else {
            self.update_instances(camera, origin, transform_changed, camera_position_changed, &update.pose_items, animation_now)
        };
        if draw_commands_changed {
            self.batches.build_draw_batches(&self.draws, &self.meshes, self.indirect_first_instance);
            let indirect_bytes = self.batches.indirect_bytes();
            if !indirect_bytes.is_empty() {
                self.queue.write_buffer(&self.indirect, 0, indirect_bytes);
            }
        }
        if mesh_progress {
            if visible_changed {
                std::mem::swap(&mut self.cache.visible, &mut self.cache.next_visible);
            }
            self.cache.next_visible.clear();
        }
        self.cache.batches_valid = true;
        self.cache.texture_state = texture_state;
        self.cache.camera = Some(camera);
        self.cache.origin = origin;
        self.cache.size = size;
        self.cache.version = Some(view.version());
        self.cache.next_candidates.clear();
        times.faces = batches_started.elapsed();

        let instances_started = Instant::now();
        let mesh_omitted_faces = if self.mesh_budget_blocked.is_empty() {
            0
        } else {
            self.cache.candidates.iter().filter_map(|id| self.cache.items.get(id)).fold(0usize, |count, item| {
                if self.mesh_budget_blocked.contains(&item.mesh.id()) { count.saturating_add(item.faces.len()) } else { count }
            })
        };
        self.stats.mesh_budget_blocked = self.mesh_budget_blocked.len();
        self.stats.omitted_faces = self.omitted_faces.saturating_add(mesh_omitted_faces);
        self.stats.omission_reason = match (self.omitted_faces > 0, mesh_omitted_faces > 0) {
            (true, true) => Some(super::OmissionReason::CombinedBudgets),
            (true, false) => Some(super::OmissionReason::InstanceBufferBudget),
            (false, true) => Some(super::OmissionReason::MeshMemoryBudget),
            (false, false) => None,
        };
        self.stats.instances = self.visible_count;
        let draw_submissions = if self.indirect_first_instance { self.batches.draw_batches().len() } else { self.draws.len() };
        self.stats.draw_calls = draw_submissions + usize::from(self.water.active);
        self.stats.draw_commands = self.draws.len() + usize::from(self.water.active);
        self.stats.pending_meshes = self.pending_meshes.len();
        self.stats.pending_textures = self.pending_textures.len();
        if self.stats.omitted_faces > 0 && self.draw_report.is_none_or(|at| at.elapsed() >= Duration::from_secs(5)) {
            self.draw_report = Some(Instant::now());
            tracing::warn!(
                omitted_faces = self.stats.omitted_faces,
                mesh_budget_blocked = self.stats.mesh_budget_blocked,
                instance_capacity = MAX_FACE_INSTANCES,
                instance_buffer_bytes = super::INSTANCE_BUFFER_BYTES,
                "scene geometry exceeded its GPU instance buffer budget"
            );
        }
        self.memory_stats();
        self.prepared = Some(FramePreparation {
            key,
            textures: self.textures.state(),
            meshes: self.memory.mesh_state(),
            more_meshes: !self.pending_meshes.is_empty() && uploaded > 0,
            animated_faces,
        });
        times.instances = instances_started.elapsed();
        self.record_prepare(started, times, uploaded);
    }

    fn update_cache(&mut self, view: &SceneView) -> CacheUpdate {
        let version = self.cache.version.clone();
        let changes = version.as_ref().and_then(|version| view.changes_since(version));
        let mut full = changes.is_none();
        self.cache.changed_objects.clear();
        self.cache.changed_surfaces.clear();
        self.cache.retired_regions.clear();
        if let Some(changes) = changes {
            for change in changes {
                full |= change.full || !change.regions.is_empty();
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
            self.cache.visible_textures.clear();
            self.cache.resident_textures.clear();
            self.cache.animated_items.clear();
            self.cache.batches_valid = false;
            self.pending_meshes.clear();
            self.pending_textures.clear();
            self.cache.mesh_state = self.memory.mesh_state();
            let mut update = CacheUpdate { full_visibility: true, batch_dirty: true, mesh_dirty: true, ..CacheUpdate::default() };
            for surface in view.surfaces() {
                if let Some(item) = self.surface_item(view, surface) {
                    self.cache.items.insert(item.id, item);
                }
            }
            for object in view.objects() {
                update.background.push(object.detail().summary.entity);
                if let Some(item) = self.object_item(object) {
                    if item.source.as_ref().is_some_and(has_enabled_animation) {
                        self.cache.animated_items.insert(item.id);
                    }
                    self.cache.objects.insert(object.detail().summary.entity, item.id);
                    self.cache.items.insert(item.id, item);
                }
            }
            self.cache.version = Some(view.version());
            return update;
        }

        let mut update = CacheUpdate::default();
        while let Some(region) = self.cache.retired_regions.iter().next().copied() {
            self.cache.retired_regions.remove(&region);
            let removed =
                self.cache.items.iter().filter(|(_, item)| item.region == region).map(|(id, item)| (*id, item.entity)).collect::<Vec<_>>();
            if !removed.is_empty() {
                update.full_visibility = true;
                update.batch_dirty = true;
            }
            for (id, entity) in removed {
                self.cache.items.remove(&id);
                self.cache.animated_items.remove(&id);
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
        if update.batch_dirty {
            self.cache.resident_textures.clear();
        }
        if update.full_visibility || update.batch_dirty {
            self.prune_mesh_info();
        }
        self.cache.version = Some(view.version());
        update
    }

    fn update_object(&mut self, object: Option<&Object>, entity: Entity, update: &mut CacheUpdate) {
        let old_id = self.cache.objects.get(&entity).copied();
        let Some(object) = object else {
            if let Some(id) = old_id {
                self.cache.items.remove(&id);
                self.cache.animated_items.remove(&id);
                self.cache.objects.remove(&entity);
                update.full_visibility = true;
                update.batch_dirty = true;
            }
            return;
        };
        let Some((_, mesh)) = object.mesh() else {
            if let Some(id) = old_id {
                self.cache.items.remove(&id);
                self.cache.animated_items.remove(&id);
                self.cache.objects.remove(&entity);
                update.full_visibility = true;
                update.batch_dirty = true;
            }
            return;
        };
        let Placement::Ready(transform) = object.placement() else {
            if let Some(id) = old_id {
                self.cache.items.remove(&id);
                self.cache.animated_items.remove(&id);
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
            cached.position = transform.pose.position;
            cached.rotation = transform.pose.rotation;
            cached.scale = transform.scale;
            cached.parameters_dirty |=
                cached.source.as_ref().and_then(|previous| previous.detail().texture_animation) != object.detail().texture_animation;
            cached.source = Some(object.clone());
            if has_enabled_animation(object) {
                self.cache.animated_items.insert(id);
            } else {
                self.cache.animated_items.remove(&id);
            }
            update.pose_items.push(id);
            return;
        }
        let item = self.object_item(object);
        if let Some(id) = old_id {
            self.cache.items.remove(&id);
            self.cache.animated_items.remove(&id);
        }
        if let Some(item) = item {
            let id = item.id;
            update.mesh_dirty |= previous_mesh != Some(item.mesh.id());
            if item.source.as_ref().is_some_and(has_enabled_animation) {
                self.cache.animated_items.insert(id);
            }
            self.cache.objects.insert(entity, id);
            self.cache.items.insert(id, item);
            update.pose_items.push(id);
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
            update.mesh_dirty |= !old || self.cache.items.get(&id).is_some_and(|previous| previous.mesh.id() != item.mesh.id());
            self.cache.items.insert(id, item);
            update.batch_dirty = true;
            update.pose_items.push(id);
            if !old {
                update.full_visibility = true;
            }
        } else if self.cache.items.remove(&id).is_some() {
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
            source: Some(object.clone()),
            faces,
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
        let faces = cached_faces(id, mesh, &info, None, None, scale, shading);
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
            source: None,
            faces,
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

    fn rebuild_candidates(&mut self, camera: Camera, projection: math::Mat4) -> bool {
        self.cache.next_candidates.clear();
        let frustum = Frustum::from_clip_matrix(projection);
        for item in self.cache.items.values() {
            if self.is_item_visible(item, camera, &frustum) {
                self.cache.next_candidates.push(item.id);
            }
        }
        let changed = self.cache.candidates != self.cache.next_candidates;
        if changed {
            std::mem::swap(&mut self.cache.candidates, &mut self.cache.next_candidates);
        }
        self.cache.next_candidates.clear();
        changed
    }

    fn update_candidates(&mut self, camera: Camera, projection: math::Mat4, ids: &[ItemId]) -> bool {
        let frustum = Frustum::from_clip_matrix(projection);
        let mut changed = false;
        for id in ids {
            let Some(item) = self.cache.items.get(id) else { continue };
            let visible = self.is_item_visible(item, camera, &frustum);
            match self.cache.candidates.binary_search(id) {
                Ok(index) if !visible => {
                    self.cache.candidates.remove(index);
                    changed = true;
                }
                Err(index) if visible => {
                    self.cache.candidates.insert(index, *id);
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

    fn rebuild_visible_textures(&mut self) {
        self.cache.visible_textures.clear();
        self.cache.visible_texture_ids.clear();
        for id in &self.cache.candidates {
            if let Some(item) = self.cache.items.get(id) {
                for face in &item.faces {
                    if face.params.texture.is_some()
                        && let Some(texture) = &face.texture
                        && self.cache.visible_texture_ids.insert(texture.id())
                    {
                        self.cache.visible_textures.push(texture.clone());
                    }
                }
                if let Some(textures) = &item.terrain_textures {
                    for texture in textures.iter().flatten() {
                        if self.cache.visible_texture_ids.insert(texture.id()) {
                            self.cache.visible_textures.push(texture.clone());
                        }
                    }
                }
            }
        }
    }

    fn rebuild_visible_meshes(&mut self, camera: Camera) {
        self.cache.mesh_ids.clear();
        self.cache.mesh_set.clear();
        self.cache.mesh_representatives.clear();
        self.cache.mesh_compositions.clear();
        self.cache.mesh_priorities.clear();
        for id in &self.cache.candidates {
            let Some(item) = self.cache.items.get(id) else { continue };
            let mesh_id = item.mesh.id();
            let priority = draw_priority(item, camera);
            if self.cache.mesh_set.insert(mesh_id) {
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
    }

    fn request_visible_textures(&mut self) {
        self.requested_textures.clear();
        for texture in &self.cache.visible_textures {
            if self.requested_textures.insert(texture.id()) {
                self.textures.request(texture, self.epoch, Priority::Visible);
            }
        }
    }

    fn request_background_textures(&mut self, object: &Object) {
        let Some(faces) = object.materials() else { return };
        for face in faces {
            self.request_material_texture(face, Priority::Background);
        }
    }

    fn request_material_texture(&mut self, face: &FaceMaterial, priority: Priority) {
        let Some(texture) = face.base_color_texture.as_ref().and_then(|binding| binding.image()) else { return };
        if FaceParams::new(Some(face)).texture.is_none() {
            return;
        }
        if priority == Priority::Background && !texture.info().is_some_and(|info| info.size.into_iter().all(|edge| edge <= 32)) {
            self.requested_textures.insert(texture.id());
            return;
        }
        self.request_material_texture_id(texture, priority);
    }

    fn request_material_texture_id(&mut self, texture: &TextureHandle, priority: Priority) {
        if self.requested_textures.insert(texture.id()) {
            self.textures.request(texture, self.epoch, priority);
        }
    }

    fn refresh_texture_bindings(&mut self, device: &Device, use_next: bool) {
        let visible = if use_next { &self.cache.next_visible } else { &self.cache.visible };
        self.textures.clear_terrain_bindings();
        self.pending_textures.clear();
        self.cache.texture_keys.clear();
        self.cache.texture_set.clear();
        self.cache.resident_textures.clear();
        for id in visible {
            if let Some(item) = self.cache.items.get(id) {
                for face in &item.faces {
                    if let Some(key) = face.params.texture
                        && self.cache.texture_set.insert(key)
                    {
                        self.cache.texture_keys.push(key);
                    }
                }
            }
        }
        for key in self.cache.texture_keys.iter().copied() {
            let resident = self.textures.resident(device, key);
            self.cache.resident_textures.insert(key, resident);
            if resident.is_none() {
                self.pending_textures.insert(key.id);
            }
        }
        for id in visible {
            let terrain_textures = self.cache.items.get(id).and_then(|item| item.terrain_textures.clone());
            if let Some(textures) = terrain_textures {
                let key = self.textures.terrain_key(&textures, self.epoch);
                for (texture, resolved) in textures.iter().zip(key.0) {
                    if resolved.is_none()
                        && let Some(texture) = texture
                    {
                        self.pending_textures.insert(texture.id());
                    }
                }
                self.textures.prepare_terrain_binding(device, key);
                if let Some(item) = self.cache.items.get_mut(id) {
                    for face in &mut item.faces {
                        face.draw.terrain = Some(key);
                    }
                }
            }
            if let Some(item) = self.cache.items.get_mut(id) {
                for face in &mut item.faces {
                    face.draw.texture = face.params.texture.and_then(|key| self.cache.resident_textures.get(&key).copied().flatten());
                }
            }
        }
    }

    fn rebuild_batches(&mut self, camera: Camera, origin: DVec3, now: Instant, use_next: bool) -> bool {
        self.batches.clear();
        self.cache.slots.clear();
        let mut animated = false;
        let visible = if use_next { &self.cache.next_visible } else { &self.cache.visible };
        let required_faces =
            visible.iter().filter_map(|id| self.cache.items.get(id)).fold(0usize, |count, item| count.saturating_add(item.faces.len()));
        self.cache.priority_items.clear();
        let ordered = if required_faces > MAX_FACE_INSTANCES {
            self.cache.priority_items.extend_from_slice(visible);
            let items = &self.cache.items;
            self.cache.priority_items.sort_unstable_by(|a, b| {
                let a_priority = items.get(a).map_or((0.0, f64::INFINITY), |item| draw_priority(item, camera));
                let b_priority = items.get(b).map_or((0.0, f64::INFINITY), |item| draw_priority(item, camera));
                b_priority.0.total_cmp(&a_priority.0).then_with(|| a_priority.1.total_cmp(&b_priority.1)).then_with(|| a.cmp(b))
            });
            self.cache.priority_items.as_slice()
        } else {
            visible.as_slice()
        };
        let mut remaining = MAX_FACE_INSTANCES;
        let mut omitted = 0usize;
        for id in ordered {
            let Some(item) = self.cache.items.get_mut(id) else { continue };
            if item.faces.len() > remaining {
                omitted = omitted.saturating_add(item.faces.len());
                continue;
            }
            remaining -= item.faces.len();
            let base = instance_base(item.position, item.rotation, item.scale, origin);
            let source = item.source.as_ref();
            let shading = item.shading;
            let position = item.position;
            let rotation = item.rotation;
            let scale = item.scale;
            for face in &mut item.faces {
                face.included = false;
                let (params, animating) = current_params(source, shading, face, now);
                face.params = params;
                let bytes = packed_instance(&base, &params, scale);
                let distance = params.blend.then(|| blend_distance(position, rotation, scale, face.center, camera));
                face.included = self.batches.push(face.draw, face.order, distance, || bytes);
                animated |= animating && face.included;
            }
            item.parameters_dirty = false;
        }
        self.omitted_faces = omitted.saturating_add(self.batches.omitted());
        self.stats.omitted_faces = self.omitted_faces;
        self.batches.finish(&mut self.draws, &mut self.instance_bytes, &mut self.cache.slots);
        for item in self.cache.items.values_mut() {
            for face in &mut item.faces {
                face.slot = self.cache.slots.get(&face.order).copied();
                face.included = face.slot.is_some();
            }
        }
        if !self.instance_bytes.is_empty() {
            self.queue.write_buffer(&self.instances, 0, &self.instance_bytes);
        }
        animated
    }

    fn rebuild_pick_instances(&mut self, use_next: bool) {
        self.visible.clear();
        for item in self.cache.items.values_mut() {
            item.visible_index = None;
        }
        let visible = if use_next { &self.cache.next_visible } else { &self.cache.visible };
        for id in visible {
            let Some(item) = self.cache.items.get_mut(id) else { continue };
            if !item.faces.iter().any(|face| face.included) {
                continue;
            }
            item.visible_index = Some(self.visible.len());
            self.visible.push(item.instance(true));
        }
        self.visible_count = self.visible.len();
    }

    fn update_instances(
        &mut self,
        camera: Camera,
        origin: DVec3,
        transform_changed: bool,
        camera_position_changed: bool,
        pose_items: &[ItemId],
        now: Instant,
    ) -> (bool, bool) {
        let animated_visible =
            self.cache.animated_items.iter().any(|id| self.cache.items.get(id).is_some_and(|item| item.visible_index.is_some()));
        if camera_position_changed && !transform_changed && pose_items.is_empty() && !animated_visible && !self.batches.has_blended() {
            return (false, false);
        }
        self.cache.update_items.clear();
        self.cache.update_set.clear();
        self.cache.pose_set.clear();
        self.cache.pose_set.extend(pose_items.iter().copied());
        if transform_changed || camera_position_changed {
            self.cache.update_items.extend(self.cache.visible.iter().copied());
        } else {
            for id in pose_items.iter().chain(
                self.cache.animated_items.iter().filter(|id| self.cache.items.get(id).is_some_and(|item| item.visible_index.is_some())),
            ) {
                if self.cache.update_set.insert(*id) {
                    self.cache.update_items.push(*id);
                }
            }
        }
        let mut animated = false;
        let mut blend_changed = false;
        for id in &self.cache.update_items {
            let pose_changed = self.cache.pose_set.contains(id);
            let Some(item) = self.cache.items.get_mut(id) else { continue };
            if let Some(index) = item.visible_index
                && let Some(instance) = self.visible.get_mut(index)
            {
                instance.position = item.position;
                instance.rotation = item.rotation;
                instance.scale = item.scale;
            }
            if item.visible_index.is_none() {
                continue;
            }
            let source = item.source.as_ref();
            let shading = item.shading;
            let position = item.position;
            let rotation = item.rotation;
            let scale = item.scale;
            let animation =
                source.and_then(|object| object.detail().texture_animation).filter(|animation| texture_animation::is_enabled(*animation));
            let parameters_dirty = item.parameters_dirty;
            let base = (transform_changed || pose_changed || parameters_dirty || animation.is_some())
                .then(|| instance_base(position, rotation, scale, origin));
            for face in &mut item.faces {
                if !face.included {
                    continue;
                }
                let refresh_params = parameters_dirty
                    || animation.is_some_and(|animation| animation.targets_face(face.order.3) && face.base_params.texture.is_some());
                let (params, animating) = if refresh_params {
                    let (params, animating) = current_params(source, shading, face, now);
                    face.params = params;
                    (params, animating)
                } else {
                    (face.params, false)
                };
                if params.blend {
                    let distance_changed = camera_position_changed || transform_changed || pose_changed;
                    let distance =
                        (distance_changed || refresh_params).then(|| blend_distance(position, rotation, scale, face.center, camera));
                    if refresh_params {
                        if let Some(base) = base.as_ref() {
                            let bytes = packed_instance(base, &params, scale);
                            self.batches.update_blended(face.order, distance.unwrap_or(0.0), bytes);
                        }
                    } else if let Some(distance) = distance {
                        if let Some(base) = base.as_ref().filter(|_| transform_changed || pose_changed) {
                            self.batches.update_blended_transform(face.order, distance, base);
                        } else {
                            self.batches.update_blended_distance(face.order, distance);
                        }
                    }
                    blend_changed |= distance.is_some() || refresh_params;
                } else if let Some(slot) = face.slot {
                    let offset = slot as usize * crate::pipeline::INSTANCE_BYTES;
                    if (refresh_params || transform_changed || pose_changed)
                        && let Some(base) = base.as_ref()
                    {
                        let bytes = packed_instance(base, &params, scale);
                        if let Some(destination) = self.instance_bytes.get_mut(offset..offset + bytes.len()) {
                            destination.copy_from_slice(&bytes);
                            if !transform_changed {
                                self.queue.write_buffer(&self.instances, offset as u64, &bytes);
                            }
                        }
                    }
                }
                animated |= animating;
            }
            item.parameters_dirty = false;
        }
        if blend_changed {
            self.batches.sort_blended(&mut self.draws, &mut self.instance_bytes, &mut self.cache.slots);
            if !transform_changed {
                let start = self.batches.blended_start();
                if let Some(bytes) = self.instance_bytes.get(start..)
                    && !bytes.is_empty()
                {
                    self.queue.write_buffer(&self.instances, start as u64, bytes);
                }
            }
        }
        if transform_changed && !self.instance_bytes.is_empty() {
            self.queue.write_buffer(&self.instances, 0, &self.instance_bytes);
        }
        (animated, blend_changed)
    }

    fn record_prepare(&mut self, started: Instant, times: PrepareTimes, uploaded: usize) {
        self.stats.prepare_ms = started.elapsed().as_secs_f64() * 1000.0;
        if self.stats.prepare_ms >= 16.0 && self.slow_report.is_none_or(|at| at.elapsed() >= Duration::from_secs(1)) {
            self.slow_report = Some(Instant::now());
            tracing::info!(
                total_ms = self.stats.prepare_ms,
                setup_visibility_ms = times.visibility.as_secs_f64() * 1000.0,
                texture_requests_ms = times.requests.as_secs_f64() * 1000.0,
                texture_upload_ms = times.textures.as_secs_f64() * 1000.0,
                meshes_faces_ms = times.faces.as_secs_f64() * 1000.0,
                instance_upload_ms = times.instances.as_secs_f64() * 1000.0,
                mesh_uploads = uploaded,
                mesh_upload_bytes = self.stats.uploaded_bytes,
                draws = self.stats.draw_calls,
                instances = self.stats.instances,
                pending_meshes = self.stats.pending_meshes,
                pending_textures = self.stats.pending_textures,
                "slow scene preparation"
            );
        }
    }
}

#[derive(Default)]
struct CacheUpdate {
    full_visibility: bool,
    batch_dirty: bool,
    mesh_dirty: bool,
    background: Vec<Entity>,
    pose_items: Vec<ItemId>,
}

fn object_item_id(object: &Object) -> ItemId {
    (2, object.detail().summary.id.as_u128(), 0)
}

fn has_enabled_animation(object: &Object) -> bool {
    object.detail().texture_animation.is_some_and(texture_animation::is_enabled)
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

fn cached_faces(
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
        let pipeline =
            if shading == ShadingMode::Water { Pipeline::Water } else { surface_pipeline(params.blend, params.double_sided, scale) };
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
            draw: DrawKey { mesh: mesh.id(), start: face.indices.start, end: face.indices.end, pipeline, texture: None, terrain: None },
            slot: None,
            included: false,
        });
    }
    cached
}

fn current_params(source: Option<&Object>, shading: ShadingMode, face: &CachedFace, now: Instant) -> (FaceParams, bool) {
    let Some(object) = source else { return (face.base_params, false) };
    let Some(animation) = object.detail().texture_animation else { return (face.base_params, false) };
    if !texture_animation::is_enabled(animation) || !animation.targets_face(face.order.3) {
        return (face.base_params, false);
    }
    let Some((elapsed, offset)) = object.animation_timing(now) else { return (face.base_params, false) };
    let transform = texture_animation::transform_at(animation, elapsed, offset, face.texture_entry_transform);
    let material = object.materials().and_then(|materials| face.material_index.and_then(|index| materials.get(index)));
    let mut params = FaceParams::with_texture_entry_transform(material, Some(transform));
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
    let changing = texture_animation::is_changing_at(animation, elapsed, offset) && params.texture.is_some();
    (params, changing)
}

fn blend_distance(position: DVec3, rotation: Quat, scale: Vec3, center: Vec3, camera: Camera) -> f64 {
    let relative = position - camera.position() + (rotation * (center * scale)).as_dvec3();
    relative.length_squared()
}

fn draw_priority(item: &CachedItem, camera: Camera) -> (f64, f64) {
    let center = (item.info.bounds.min.as_dvec3() + item.info.bounds.max.as_dvec3()) * 0.5 * item.scale.as_dvec3();
    let offset = item.position - camera.position() + (item.rotation * center.as_vec3()).as_dvec3();
    let radius = ((item.info.bounds.max.as_dvec3() - item.info.bounds.min.as_dvec3()).abs() * item.scale.as_dvec3().abs() * 0.5).length();
    let distance = offset.length();
    (radius / distance.max(1.0), distance)
}

fn surface_pipeline(blend: bool, double_sided: bool, scale: Vec3) -> Pipeline {
    match (blend, double_sided, math::flips_winding(scale)) {
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
    use super::*;
    use crate::geometry::MeshState;
    use crate::scene::{Budget, Reconciler};
    use crate::texture::TextureState;
    use math::DVec3;

    fn prepare_key(view: &SceneView) -> FramePreparationKey {
        FramePreparationKey {
            version: view.version(),
            camera: Camera::new(DVec3::ZERO, Vec3::X, 1.0, [720, 720]).unwrap(),
            size: [1280, 720],
            water_height: Some(20.0),
        }
    }

    fn texture_state() -> TextureState {
        TextureState { revision: 0, staging: 0, jobs: 0, lifecycle: 0, cap: 1024 * 1024 * 1024 }
    }

    #[test]
    fn preparation_tracks_scene_inputs() {
        let (mut scene, view) = Reconciler::new(Budget::default());
        let (_other_scene, other) = Reconciler::new(Budget::default());
        let prepared = FramePreparation {
            key: prepare_key(&view),
            textures: texture_state(),
            meshes: MeshState::default(),
            more_meshes: false,
            animated_faces: false,
        };
        assert!(prepared.reusable(&prepare_key(&view), texture_state(), MeshState::default()));
        let mut moved = prepare_key(&view);
        moved.camera = Camera::new(DVec3::X, Vec3::X, 1.0, [720, 720]).unwrap();
        assert!(!prepared.reusable(&moved, texture_state(), MeshState::default()));
        let mut resized = prepare_key(&view);
        resized.size[0] = 1600;
        assert!(!prepared.reusable(&resized, texture_state(), MeshState::default()));
        let mut quality = prepare_key(&view);
        quality.camera = quality.camera.with_quality(128.0, 2.0).unwrap();
        assert!(!prepared.reusable(&quality, texture_state(), MeshState::default()));
        let mut water = prepare_key(&view);
        water.water_height = Some(21.0);
        assert!(!prepared.reusable(&water, texture_state(), MeshState::default()));
        assert!(!prepared.reusable(&prepare_key(&other), texture_state(), MeshState::default()));
        scene.close();
        assert!(!prepared.reusable(&prepare_key(&view), texture_state(), MeshState::default()));
    }

    #[test]
    fn preparation_tracks_gpu_progress() {
        let (_scene, view) = Reconciler::new(Budget::default());
        let state = texture_state();
        let mut prepared = FramePreparation {
            key: prepare_key(&view),
            textures: state,
            meshes: MeshState::default(),
            more_meshes: false,
            animated_faces: false,
        };
        for (reason, changed) in [
            ("image readiness or cancellation", TextureState { revision: 1, ..state }),
            ("handle release or asset change", TextureState { lifecycle: 1, ..state }),
            ("staging completion", TextureState { staging: 256, ..state }),
            ("conversion admission", TextureState { jobs: 1, ..state }),
            ("soft cap change", TextureState { cap: 64 * 1024 * 1024, ..state }),
        ] {
            assert!(!prepared.reusable(&prepare_key(&view), changed, MeshState::default()), "{reason} must allow preparation to progress");
        }
        assert!(!prepared.reusable(&prepare_key(&view), state, MeshState { bytes: 4, revision: 0 }));
        assert!(!prepared.reusable(&prepare_key(&view), state, MeshState { bytes: 0, revision: 1 }));
        prepared.more_meshes = true;
        assert!(!prepared.reusable(&prepare_key(&view), state, MeshState::default()));
    }

    #[test]
    fn redraw_stops_without_visible_animation() {
        let (_scene, view) = Reconciler::new(Budget::default());
        let state = texture_state();
        let mut prepared = FramePreparation {
            key: prepare_key(&view),
            textures: state,
            meshes: MeshState::default(),
            more_meshes: false,
            animated_faces: false,
        };
        assert!(!prepared.needs_frame(state, MeshState::default()));
        prepared.animated_faces = true;
        assert!(prepared.needs_frame(state, MeshState::default()));
        prepared.animated_faces = false;
        assert!(!prepared.needs_frame(state, MeshState::default()));
    }
}
