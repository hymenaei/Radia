use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use assets::{Priority, TextureHandle, TextureId};
use math::{DVec3, Quat, Vec3};
use wgpu::{CommandEncoder, Device};
use world::Region;

use super::{FramePreparation, FramePreparationKey, GeometryRenderer, PrepareTimes, ShadingMode, Stats};
use crate::geometry::{
    batching::MAX_FACE_INSTANCES,
    cache::{CachedFace, ItemId, draw_priority, visible_difference},
    instance::{instance_base, packed_instance, scene_origin},
};
use crate::material::{FaceMaterial, FaceParams};
use crate::pipeline::COLOR;
use crate::scene::{Camera, Object, SceneView, texture_animation};

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
            if self
                .prepared
                .as_ref()
                .is_some_and(|prepared| prepared.reusable(&key, self.textures.state(), self.memory.mesh_state(), started))
            {
                self.stats.uploaded_bytes = 0;
                self.memory_stats();
                self.record_prepare(started, times, 0);
                return;
            }
        }

        let visibility_started = Instant::now();
        self.cache.visible_added.clear();
        self.cache.visible_removed.clear();
        self.epoch = self.epoch.checked_add(1).expect("render frame space exhausted");
        self.stats = Stats::default();
        let previous_camera = self.cache.camera;
        let camera_changed = previous_camera != Some(camera) || self.cache.size != size;
        let camera_position_changed = previous_camera.is_none_or(|previous| previous.position() != camera.position());
        let origin = scene_origin(camera.position());
        let transform_changed = previous_camera.is_none() || self.cache.origin != origin;
        let update = self.update_cache(view);
        self.stats.full_rebuilds = usize::from(update.full_rebuilds);
        self.stats.region_invalidations = update.region_invalidations;
        self.stats.objects_reprepared = update.objects_reprepared;
        self.stats.dirtied_items = update.dirtied_items;
        let projection = self.pipelines.update_camera_uniform(&self.queue, camera, size, origin);
        self.water.update(&self.queue, view, camera, projection, water_height);
        let candidates_changed = if update.full_visibility || camera_changed {
            self.rebuild_candidates(camera, projection)
        } else if !update.pose_items.is_empty() {
            self.update_candidates(camera, projection, &update.pose_items)
        } else {
            false
        };
        times.visibility = visibility_started.elapsed();

        let requests_started = Instant::now();
        self.requested_textures.clear();
        if update.full_rebuilds {
            for index in 0..self.cache.candidates.len() {
                self.request_item_textures(self.cache.candidates[index]);
            }
        } else {
            for index in 0..self.cache.candidate_added.len() {
                let id = self.cache.candidate_added[index];
                if self.cache.candidates.binary_search(&id).is_ok() {
                    self.request_item_textures(id);
                }
            }
            for index in 0..update.texture_items.len() {
                let id = update.texture_items[index];
                if self.cache.candidates.binary_search(&id).is_ok() {
                    self.request_item_textures(id);
                }
            }
        }
        self.request_visible_textures();
        for entity in update.background {
            if let Some(object) = view.object(entity) {
                self.request_background_textures(object);
            }
        }
        times.requests = requests_started.elapsed();

        let candidate_mesh_missing = if candidates_changed || update.mesh_dirty { self.rebuild_visible_meshes(camera) } else { false };
        let mesh_progress = candidate_mesh_missing
            || update.mesh_dirty
            || self.cache.mesh_state != self.memory.mesh_state()
            || !self.pending_meshes.is_empty();
        let mut uploaded = 0;
        if mesh_progress {
            self.mesh_budget_blocked.clear();
            self.pending_meshes.clear();
            self.cache.mesh_ready.clear();
            for index in 0..self.cache.mesh_ids.len() {
                let mesh_id = self.cache.mesh_ids[index];
                let Some(mesh) = self.cache.mesh_representatives.get(&mesh_id).cloned() else { continue };
                if self.meshes.contains_key(&mesh_id) {
                    self.cache.mesh_ready.insert(mesh_id);
                    continue;
                }
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
        } else {
            self.cache.next_visible.clear();
            for id in &self.cache.candidates {
                let Some(item) = self.cache.items.get(id) else { continue };
                if self.meshes.contains_key(&item.mesh.id()) && !item.faces.is_empty() {
                    self.cache.next_visible.push(*id);
                }
            }
        }
        if self.cache.visible != self.cache.next_visible {
            visible_difference(
                &self.cache.visible,
                &self.cache.next_visible,
                &mut self.cache.visible_added,
                &mut self.cache.visible_removed,
            );
        }

        let upload_started = Instant::now();
        if !same {
            self.textures.upload(device, encoder, self.epoch);
        }
        times.textures += upload_started.elapsed();
        let texture_state = self.textures.state();
        let texture_changes = self.textures.take_changed_ids();
        self.stats.texture_state_changes = texture_changes.len();
        let visible_changed = self.cache.visible != self.cache.next_visible;
        let use_next = mesh_progress || visible_changed;
        let full_texture_refresh = !self.cache.batches_valid || update.batch_dirty;
        let batches_started = Instant::now();
        let rebuild_batches = full_texture_refresh || transform_changed || (visible_changed && !self.can_append_visible_batches());
        if full_texture_refresh {
            self.stats.texture_state_reresolves = self.refresh_texture_bindings(device, use_next);
        } else {
            if visible_changed {
                self.update_visible_texture_bindings(device);
            }
            if !update.terrain_regions.is_empty() {
                self.rebuild_visible_texture_demand(use_next);
            }
            self.stats.texture_state_reresolves =
                self.refresh_changed_texture_bindings(device, &texture_changes, &update.terrain_regions, use_next);
        }
        let draw_commands_changed = if rebuild_batches {
            self.rebuild_batches(camera, origin, animation_now, use_next);
            self.rebuild_pick_instances(use_next);
            true
        } else {
            if visible_changed {
                self.hide_removed_instances(camera, origin);
                let appended = self.append_visible_batches(camera, origin, animation_now, camera_position_changed);
                self.update_pick_instances();
                let reordered =
                    self.update_instances(camera, origin, transform_changed, camera_position_changed, &update.pose_items, animation_now);
                appended || reordered
            } else {
                self.update_instances(camera, origin, transform_changed, camera_position_changed, &update.pose_items, animation_now)
            }
        };
        if draw_commands_changed {
            self.batches.build_draw_batches(&self.draws, &self.meshes, self.indirect_first_instance);
            let indirect_bytes = self.batches.indirect_bytes();
            if !indirect_bytes.is_empty() {
                self.queue.write_buffer(&self.indirect, 0, indirect_bytes);
            }
        }
        if visible_changed {
            std::mem::swap(&mut self.cache.visible, &mut self.cache.next_visible);
        }
        self.cache.next_visible.clear();
        self.cache.batches_valid = true;
        self.cache.texture_state = texture_state;
        self.cache.camera = Some(camera);
        self.cache.origin = origin;
        self.cache.size = size;
        self.cache.version = Some(view.version());
        self.cache.next_candidates.clear();
        let animation_deadline =
            self.cache.visible.iter().filter_map(|id| self.cache.items.get(id).and_then(|item| item.animation_deadline)).min();
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
            animation_deadline,
        });
        times.instances = instances_started.elapsed();
        self.record_prepare(started, times, uploaded);
    }

    fn request_visible_textures(&mut self) {
        let epoch = self.epoch;
        let (handles, requested, textures) = (&self.cache.visible_texture_handles, &mut self.requested_textures, &mut self.textures);
        for texture in handles.values() {
            if requested.insert(texture.id()) {
                textures.request(texture, epoch, Priority::Visible);
            }
        }
    }

    fn request_item_textures(&mut self, id: ItemId) {
        let epoch = self.epoch;
        let Some(item) = self.cache.items.get(&id) else { return };
        for face in &item.faces {
            if face.params.texture.is_some()
                && let Some(texture) = &face.texture
                && self.requested_textures.insert(texture.id())
            {
                self.textures.request(texture, epoch, Priority::Visible);
            }
        }
        if let Some(textures) = &item.terrain_textures {
            for texture in textures.iter().flatten() {
                if self.requested_textures.insert(texture.id()) {
                    self.textures.request(texture, epoch, Priority::Visible);
                }
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

    fn refresh_texture_bindings(&mut self, device: &Device, use_next: bool) -> usize {
        self.refresh_visible_texture_bindings(device, use_next, true)
    }

    fn refresh_visible_texture_bindings(&mut self, device: &Device, use_next: bool, clear_terrain: bool) -> usize {
        self.rebuild_visible_texture_demand(use_next);
        let visible = if use_next { &self.cache.next_visible } else { &self.cache.visible };
        if clear_terrain {
            self.textures.clear_terrain_bindings();
        }
        self.pending_textures.clear();
        self.cache.texture_keys.clear();
        self.cache.texture_set.clear();
        self.cache.texture_face_counts.clear();
        for id in visible {
            if let Some(item) = self.cache.items.get(id) {
                for face in &item.faces {
                    if let Some(key) = face.params.texture {
                        *self.cache.texture_face_counts.entry(key).or_default() += 1;
                        if self.cache.texture_set.insert(key) {
                            self.cache.texture_keys.push(key);
                        }
                    }
                }
            }
        }
        for key in self.cache.texture_keys.iter().copied() {
            if self.textures.resident(device, key).is_none() {
                self.pending_textures.insert(key.id);
            }
        }
        let mut updated_regions = HashSet::new();
        for id in visible {
            let Some(item) = self.cache.items.get_mut(id) else { continue };
            if let Some(textures) = &item.terrain_textures {
                if self.textures.prepare_terrain_binding(device, item.region, textures, self.epoch) {
                    updated_regions.insert(item.region);
                }
                for face in &mut item.faces {
                    face.draw.terrain = Some(item.region);
                }
                for texture in textures.iter().flatten() {
                    if !self.textures.is_resident(texture.id()) {
                        self.pending_textures.insert(texture.id());
                    }
                }
            }
        }
        visible
            .iter()
            .filter_map(|id| self.cache.items.get(id))
            .filter(|item| item.surface_kind.is_some() && updated_regions.contains(&item.region))
            .map(|item| item.faces.len())
            .sum::<usize>()
    }

    fn rebuild_visible_texture_demand(&mut self, use_next: bool) {
        self.cache.visible_texture_counts.clear();
        self.cache.visible_texture_handles.clear();
        let count = if use_next { self.cache.next_visible.len() } else { self.cache.visible.len() };
        for index in 0..count {
            let id = if use_next { self.cache.next_visible[index] } else { self.cache.visible[index] };
            self.adjust_visible_texture_demand(id, true);
        }
    }

    fn adjust_visible_texture_demand(&mut self, id: ItemId, adding: bool) {
        let Some(item) = self.cache.items.get(&id) else { return };
        let (counts, handles) = (&mut self.cache.visible_texture_counts, &mut self.cache.visible_texture_handles);
        for face in &item.faces {
            if face.params.texture.is_some()
                && let Some(texture) = &face.texture
            {
                adjust_texture_reference(counts, handles, texture, adding);
            }
        }
        if let Some(terrain) = &item.terrain_textures {
            for texture in terrain.iter().flatten() {
                adjust_texture_reference(counts, handles, texture, adding);
            }
        }
    }

    fn update_visible_texture_bindings(&mut self, device: &Device) {
        for removed_index in 0..self.cache.visible_removed.len() {
            let id = self.cache.visible_removed[removed_index];
            self.adjust_visible_texture_demand(id, false);
            let face_count = self.cache.items.get(&id).map_or(0, |item| item.faces.len());
            for index in 0..face_count {
                let key = self.cache.items.get(&id).and_then(|item| item.faces.get(index)).and_then(|face| face.params.texture);
                if let Some(key) = key {
                    let empty = if let Some(count) = self.cache.texture_face_counts.get_mut(&key) {
                        *count = count.saturating_sub(1);
                        *count == 0
                    } else {
                        false
                    };
                    if empty {
                        self.cache.texture_face_counts.remove(&key);
                        self.cache.texture_set.remove(&key);
                        self.pending_textures.remove(&key.id);
                    }
                }
            }
        }
        self.cache.texture_keys.retain(|key| self.cache.texture_face_counts.contains_key(key));
        self.cache.changed_terrain_regions.clear();
        for entering_index in 0..self.cache.visible_added.len() {
            let id = self.cache.visible_added[entering_index];
            self.adjust_visible_texture_demand(id, true);
            let face_count = self.cache.items.get(&id).map_or(0, |item| item.faces.len());
            for index in 0..face_count {
                let key = self.cache.items.get(&id).and_then(|item| item.faces.get(index)).and_then(|face| face.params.texture);
                if let Some(key) = key {
                    let active = match self.cache.texture_face_counts.entry(key) {
                        std::collections::hash_map::Entry::Occupied(mut entry) => {
                            *entry.get_mut() += 1;
                            false
                        }
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(1);
                            self.cache.texture_set.insert(key);
                            self.cache.texture_keys.push(key);
                            true
                        }
                    };
                    if active && self.textures.resident(device, key).is_none() {
                        self.pending_textures.insert(key.id);
                    }
                }
            }
            let Some(item) = self.cache.items.get(&id) else { continue };
            if !item.surface_kind.is_some() || !self.cache.changed_terrain_regions.insert(item.region) {
                continue;
            }
            if let Some(textures) = &item.terrain_textures {
                if self.textures.prepare_terrain_binding(device, item.region, textures, self.epoch) {
                    self.cache.changed_terrain_regions.insert(item.region);
                }
                for texture in textures.iter().flatten() {
                    if self.textures.is_resident(texture.id()) {
                        self.pending_textures.remove(&texture.id());
                    } else {
                        self.pending_textures.insert(texture.id());
                    }
                }
            }
        }
    }

    fn refresh_changed_texture_bindings(
        &mut self,
        device: &Device,
        changed: &HashSet<TextureId>,
        regions: &[Region],
        use_next: bool,
    ) -> usize {
        let visible = if use_next { &self.cache.next_visible } else { &self.cache.visible };
        if changed.is_empty() && regions.is_empty() {
            return 0;
        }
        let mut changed_faces = 0;
        for key in self.cache.texture_keys.iter().copied().filter(|key| changed.contains(&key.id)) {
            changed_faces += self.cache.texture_face_counts.get(&key).copied().unwrap_or(0);
            if self.textures.resident(device, key).is_some() {
                self.pending_textures.remove(&key.id);
            } else {
                self.pending_textures.insert(key.id);
            }
        }
        let mut affected_regions = HashSet::new();
        affected_regions.extend(regions.iter().copied());
        for id in visible {
            let Some(item) = self.cache.items.get(id) else { continue };
            if item
                .terrain_textures
                .as_ref()
                .is_some_and(|textures| textures.iter().flatten().any(|texture| changed.contains(&texture.id())))
            {
                affected_regions.insert(item.region);
            }
        }
        let mut updated_regions = HashSet::new();
        for id in visible {
            let Some(item) = self.cache.items.get(id) else { continue };
            if !affected_regions.contains(&item.region) {
                continue;
            }
            if let Some(textures) = &item.terrain_textures {
                if self.textures.prepare_terrain_binding(device, item.region, textures, self.epoch) {
                    updated_regions.insert(item.region);
                }
                for texture in textures.iter().flatten() {
                    if self.textures.is_resident(texture.id()) {
                        self.pending_textures.remove(&texture.id());
                    } else {
                        self.pending_textures.insert(texture.id());
                    }
                }
            }
        }
        changed_faces
            + visible
                .iter()
                .filter_map(|id| self.cache.items.get(id))
                .filter(|item| item.surface_kind.is_some() && updated_regions.contains(&item.region))
                .map(|item| item.faces.len())
                .sum::<usize>()
    }

    fn rebuild_batches(&mut self, camera: Camera, origin: DVec3, now: Instant, use_next: bool) {
        self.batches.clear();
        self.cache.slots.clear();
        self.cache.blended_items.clear();
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
                let params = current_params(source, shading, face, now);
                face.params = params;
                let bytes = packed_instance(&base, &params, scale);
                let distance = params.blend.then(|| blend_distance(position, rotation, scale, face.center, camera));
                face.included = self.batches.push(face.draw, face.order, distance, || bytes);
            }
            item.parameters_dirty = false;
        }
        self.omitted_faces = omitted.saturating_add(self.batches.omitted());
        self.stats.omitted_faces = self.omitted_faces;
        self.batches.finish(&mut self.draws, &mut self.instance_bytes, &mut self.cache.slots);
        for (id, item) in &mut self.cache.items {
            for face in &mut item.faces {
                face.slot = self.cache.slots.get(&face.order).copied();
                face.included = face.slot.is_some();
            }
            if item.faces.iter().any(|face| face.included && face.params.blend) {
                self.cache.blended_items.insert(*id);
            }
            item.animation_deadline = next_animation_deadline(item, now);
        }
        if !self.instance_bytes.is_empty() {
            self.queue.write_buffer(&self.instances, 0, &self.instance_bytes);
        }
    }

    fn can_append_visible_batches(&self) -> bool {
        let mut additional = 0;
        for id in &self.cache.visible_added {
            let Some(item) = self.cache.items.get(id) else { continue };
            for face in &item.faces {
                if !face.included {
                    additional += 1;
                }
            }
        }
        self.batches.can_append(additional)
    }

    fn hide_removed_instances(&mut self, camera: Camera, origin: DVec3) {
        self.cache.visibility_slots.clear();
        for index in 0..self.cache.visible_removed.len() {
            let id = self.cache.visible_removed[index];
            self.cache.blended_items.remove(&id);
            let Some(item) = self.cache.items.get_mut(&id) else { continue };
            let base = instance_base(item.position, item.rotation, Vec3::ZERO, origin);
            let position = item.position;
            let rotation = item.rotation;
            let scale = item.scale;
            for face in &item.faces {
                if !face.included {
                    continue;
                }
                let Some(slot) = self.cache.slots.get(&face.order).copied() else { continue };
                let bytes = packed_instance(&base, &face.params, Vec3::ZERO);
                let offset = slot as usize * std::mem::size_of_val(&bytes);
                if let Some(destination) = self.instance_bytes.get_mut(offset..offset + bytes.len()) {
                    destination.copy_from_slice(&bytes);
                    self.cache.visibility_slots.push(slot);
                }
                if face.params.blend {
                    let distance = blend_distance(position, rotation, scale, face.center, camera);
                    self.batches.update_blended(face.order, distance, bytes);
                }
            }
        }
        self.write_visibility_slots();
    }

    fn write_visibility_slots(&mut self) {
        self.cache.visibility_slots.sort_unstable();
        self.cache.visibility_slots.dedup();
        let mut first = None;
        let mut last = 0u32;
        for slot in self.cache.visibility_slots.iter().copied() {
            if let Some(start) = first
                && slot != last + 1
            {
                self.write_instance_slots(start, last);
                first = None;
            }
            if first.is_none() {
                first = Some(slot);
            }
            last = slot;
        }
        if let Some(start) = first {
            self.write_instance_slots(start, last);
        }
        self.cache.visibility_slots.clear();
    }

    fn write_instance_slots(&self, first: u32, last: u32) {
        let start = first as usize * crate::pipeline::INSTANCE_BYTES;
        let end = (last as usize + 1) * crate::pipeline::INSTANCE_BYTES;
        if let Some(bytes) = self.instance_bytes.get(start..end) {
            self.queue.write_buffer(&self.instances, start as u64, bytes);
        }
    }

    fn append_visible_batches(&mut self, camera: Camera, origin: DVec3, now: Instant, camera_position_changed: bool) -> bool {
        let opaque_start = self.batches.opaque_bytes();
        let mut draw_commands_changed = false;
        let mut blended_changed = false;
        for entering_index in 0..self.cache.visible_added.len() {
            let id = self.cache.visible_added[entering_index];
            let Some(item) = self.cache.items.get_mut(&id) else { continue };
            let entering = item.visible_index.is_none();
            let base = instance_base(item.position, item.rotation, item.scale, origin);
            let source = item.source.as_ref();
            let shading = item.shading;
            let position = item.position;
            let rotation = item.rotation;
            let scale = item.scale;
            for face in &mut item.faces {
                if face.included {
                    if entering {
                        let params = current_params(source, shading, face, now);
                        face.params = params;
                        let bytes = packed_instance(&base, &params, scale);
                        if params.blend {
                            let distance = blend_distance(position, rotation, scale, face.center, camera);
                            self.batches.update_blended(face.order, distance, bytes);
                            blended_changed = true;
                        } else if let Some(slot) = self.cache.slots.get(&face.order).copied() {
                            let offset = slot as usize * std::mem::size_of_val(&bytes);
                            if let Some(destination) = self.instance_bytes.get_mut(offset..offset + bytes.len()) {
                                destination.copy_from_slice(&bytes);
                                self.cache.visibility_slots.push(slot);
                            }
                        }
                    }
                    continue;
                }
                let params = current_params(source, shading, face, now);
                let bytes = packed_instance(&base, &params, scale);
                if params.blend {
                    let distance = blend_distance(position, rotation, scale, face.center, camera);
                    if !self.batches.append_blended(face.draw, face.order, distance, bytes) {
                        continue;
                    }
                    face.slot = None;
                    blended_changed = true;
                } else {
                    let Some(slot) = self.batches.append_opaque(
                        face.draw,
                        face.order,
                        bytes,
                        &mut self.draws,
                        &mut self.instance_bytes,
                        &mut self.cache.slots,
                    ) else {
                        continue;
                    };
                    face.slot = Some(slot);
                }
                face.params = params;
                face.included = true;
                draw_commands_changed = true;
            }
            item.parameters_dirty = false;
            item.animation_deadline = next_animation_deadline(item, now);
            if item.faces.iter().any(|face| face.included && face.params.blend) {
                self.cache.blended_items.insert(id);
            }
        }
        self.write_visibility_slots();
        if (draw_commands_changed || blended_changed) && self.batches.has_blended() && !camera_position_changed {
            self.batches.sort_blended(&mut self.draws, &mut self.instance_bytes, &mut self.cache.slots);
            blended_changed = true;
        }
        let opaque_end = self.batches.opaque_bytes();
        if opaque_end > opaque_start {
            self.queue.write_buffer(&self.instances, opaque_start as u64, &self.instance_bytes[opaque_start..opaque_end]);
        }
        if blended_changed && !camera_position_changed {
            let blend_start = self.batches.blended_start();
            if let Some(bytes) = self.instance_bytes.get(blend_start..)
                && !bytes.is_empty()
            {
                self.queue.write_buffer(&self.instances, blend_start as u64, bytes);
            }
        }
        if draw_commands_changed {
            self.stats.omitted_faces = self.batches.omitted();
        }
        draw_commands_changed || blended_changed
    }

    fn rebuild_pick_instances(&mut self, use_next: bool) {
        self.visible.clear();
        self.visible_ids.clear();
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
            self.visible_ids.push(*id);
            self.visible.push(item.instance(true));
        }
        self.visible_count = self.visible.len();
    }

    fn update_pick_instances(&mut self) {
        for removed_index in 0..self.cache.visible_removed.len() {
            let id = self.cache.visible_removed[removed_index];
            let index = self.cache.items.get_mut(&id).and_then(|item| item.visible_index.take());
            let Some(index) = index else { continue };
            self.visible.swap_remove(index);
            self.visible_ids.swap_remove(index);
            if let Some(swapped_id) = self.visible_ids.get(index).copied()
                && let Some(swapped) = self.cache.items.get_mut(&swapped_id)
            {
                swapped.visible_index = Some(index);
            }
        }
        for added_index in 0..self.cache.visible_added.len() {
            let id = self.cache.visible_added[added_index];
            let Some(item) = self.cache.items.get_mut(&id) else { continue };
            if !item.faces.iter().any(|face| face.included) {
                continue;
            }
            item.visible_index = Some(self.visible.len());
            self.visible_ids.push(id);
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
    ) -> bool {
        self.cache.update_items.clear();
        self.cache.update_set.clear();
        self.cache.pose_set.clear();
        self.cache.pose_set.extend(pose_items.iter().copied());
        if transform_changed {
            self.cache.update_items.extend(self.cache.visible.iter().copied());
        } else {
            for id in pose_items.iter().copied() {
                if self.cache.update_set.insert(id) {
                    self.cache.update_items.push(id);
                }
            }
            for id in self.cache.animated_items.iter().copied().filter(|id| {
                self.cache
                    .items
                    .get(id)
                    .is_some_and(|item| item.visible_index.is_some() && item.animation_deadline.is_some_and(|deadline| deadline <= now))
            }) {
                if self.cache.update_set.insert(id) {
                    self.cache.update_items.push(id);
                }
            }
            if camera_position_changed {
                for id in self.cache.blended_items.iter().copied() {
                    if self.cache.update_set.insert(id) {
                        self.cache.update_items.push(id);
                    }
                }
            }
        }
        if self.cache.update_items.is_empty() {
            return false;
        }
        let mut blend_order_changed = false;
        let mut blend_bytes_changed = false;
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
            let animation_due = item.animation_deadline.is_some_and(|deadline| deadline <= now);
            let animation_started = animation_due.then(Instant::now);
            let parameters_dirty = item.parameters_dirty;
            let animation_only = animation_due && !pose_changed && !transform_changed && !parameters_dirty;
            let base = (transform_changed || pose_changed || parameters_dirty || animation_due)
                .then(|| instance_base(position, rotation, scale, origin));
            let animated_faces = &item.animated_faces;
            let faces = &mut item.faces;
            let face_count = if animation_only { animated_faces.len() } else { faces.len() };
            let mut face_cursor = 0;
            while face_cursor < face_count {
                let face_index = if animation_only { animated_faces[face_cursor] } else { face_cursor };
                face_cursor += 1;
                let Some(face) = faces.get_mut(face_index) else { continue };
                if !face.included {
                    continue;
                }
                let refresh_params = parameters_dirty
                    || (animation_due
                        && animation.is_some_and(|animation| animation.targets_face(face.order.3) && face.base_params.texture.is_some()));
                let previous_params = face.params;
                let params = if refresh_params { current_params(source, shading, face, now) } else { face.params };
                let params_changed = params != previous_params;
                if refresh_params {
                    face.params = params;
                }
                if animation_due
                    && face.base_params.texture.is_some()
                    && animation.is_some_and(|animation| animation.targets_face(face.order.3))
                {
                    self.stats.animated_faces_visited += 1;
                    self.stats.animated_faces_changed += usize::from(params_changed);
                }
                if params.blend {
                    let distance_changed = camera_position_changed || transform_changed || pose_changed;
                    if params_changed {
                        if let Some(base) = base.as_ref() {
                            let bytes = packed_instance(base, &params, scale);
                            if distance_changed {
                                let distance = blend_distance(position, rotation, scale, face.center, camera);
                                self.batches.update_blended(face.order, distance, bytes);
                                blend_order_changed = true;
                            } else {
                                self.batches.update_blended_bytes(face.order, bytes);
                                blend_bytes_changed = true;
                            }
                        }
                    } else if distance_changed {
                        let distance = blend_distance(position, rotation, scale, face.center, camera);
                        if let Some(base) = base.as_ref().filter(|_| transform_changed || pose_changed) {
                            self.batches.update_blended_transform(face.order, distance, base);
                        } else {
                            self.batches.update_blended_distance(face.order, distance);
                        }
                        blend_order_changed = true;
                    }
                } else if let Some(slot) = face.slot {
                    let offset = slot as usize * crate::pipeline::INSTANCE_BYTES;
                    if (params_changed || transform_changed || pose_changed)
                        && let Some(base) = base.as_ref()
                    {
                        let bytes = packed_instance(base, &params, scale);
                        if let Some(destination) = self.instance_bytes.get_mut(offset..offset + bytes.len()) {
                            destination.copy_from_slice(&bytes);
                            if !transform_changed {
                                self.cache.visibility_slots.push(slot);
                            }
                        }
                    }
                }
            }
            item.parameters_dirty = false;
            if animation_due || parameters_dirty {
                item.animation_deadline = next_animation_deadline(item, now);
            }
            if let Some(animation_started) = animation_started {
                self.stats.animation_update_ms += animation_started.elapsed().as_secs_f64() * 1000.0;
            }
        }
        if !transform_changed {
            self.write_visibility_slots();
        }
        if blend_order_changed {
            self.batches.sort_blended(&mut self.draws, &mut self.instance_bytes, &mut self.cache.slots);
            if !transform_changed {
                let start = self.batches.blended_start();
                if let Some(bytes) = self.instance_bytes.get(start..)
                    && !bytes.is_empty()
                {
                    self.queue.write_buffer(&self.instances, start as u64, bytes);
                }
            }
        } else if blend_bytes_changed {
            self.batches.write_blended_bytes(&mut self.instance_bytes);
            let start = self.batches.blended_start();
            if let Some(bytes) = self.instance_bytes.get(start..)
                && !bytes.is_empty()
            {
                self.queue.write_buffer(&self.instances, start as u64, bytes);
            }
        }
        if transform_changed && !self.instance_bytes.is_empty() {
            self.queue.write_buffer(&self.instances, 0, &self.instance_bytes);
        }
        blend_order_changed
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
                full_rebuilds = self.stats.full_rebuilds,
                region_invalidations = self.stats.region_invalidations,
                texture_state_reresolves = self.stats.texture_state_reresolves,
                texture_state_changes = self.stats.texture_state_changes,
                objects_reprepared = self.stats.objects_reprepared,
                dirtied_items = self.stats.dirtied_items,
                animated_faces_visited = self.stats.animated_faces_visited,
                animated_faces_changed = self.stats.animated_faces_changed,
                animation_update_ms = self.stats.animation_update_ms,
                "slow scene preparation"
            );
        }
    }
}

fn adjust_texture_reference(
    counts: &mut HashMap<TextureId, usize>,
    handles: &mut HashMap<TextureId, TextureHandle>,
    texture: &TextureHandle,
    adding: bool,
) {
    let id = texture.id();
    if adding {
        *counts.entry(id).or_default() += 1;
        handles.entry(id).or_insert_with(|| texture.clone());
    } else if let Some(count) = counts.get_mut(&id) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            counts.remove(&id);
            handles.remove(&id);
        }
    }
}

fn current_params(source: Option<&Object>, shading: ShadingMode, face: &CachedFace, now: Instant) -> FaceParams {
    let Some(object) = source else { return face.base_params };
    let Some(animation) = object.detail().texture_animation else { return face.base_params };
    if !texture_animation::is_enabled(animation) || !animation.targets_face(face.order.3) {
        return face.base_params;
    }
    let Some((elapsed, offset)) = object.animation_timing(now) else { return face.base_params };
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
    params
}

fn next_animation_deadline(item: &super::cache::CachedItem, now: Instant) -> Option<Instant> {
    if !item.animated_faces.iter().any(|index| item.faces.get(*index).is_some_and(|face| face.included)) {
        return None;
    }
    let source = item.source.as_ref()?;
    let animation = source.detail().texture_animation?;
    let (elapsed, offset) = source.animation_timing(now)?;
    texture_animation::next_update_after(animation, elapsed, offset).and_then(|delay| now.checked_add(delay))
}

fn blend_distance(position: DVec3, rotation: Quat, scale: Vec3, center: Vec3, camera: Camera) -> f64 {
    let relative = position - camera.position() + (rotation * (center * scale)).as_dvec3();
    relative.length_squared()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::MeshState;
    use crate::geometry::cache::{cached_faces, mesh_info, object_item_id, refresh_winding_pipeline, surface_pipeline};
    use crate::scene::{Budget, Placement, Reconciler};
    use crate::texture::TextureState;
    use math::DVec3;

    #[test]
    fn visibility_difference_tracks_only_membership_changes() {
        let current = [(2, 1, 0), (2, 3, 0), (2, 5, 0)];
        let next = [(2, 2, 0), (2, 3, 0), (2, 4, 0)];
        let (mut added, mut removed) = (Vec::new(), Vec::new());
        visible_difference(&current, &next, &mut added, &mut removed);

        assert_eq!(added, [(2, 2, 0), (2, 4, 0)]);
        assert_eq!(removed, [(2, 1, 0), (2, 5, 0)]);
    }

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
            animation_deadline: None,
        };
        let now = Instant::now();
        assert!(prepared.reusable(&prepare_key(&view), texture_state(), MeshState::default(), now));
        let mut moved = prepare_key(&view);
        moved.camera = Camera::new(DVec3::X, Vec3::X, 1.0, [720, 720]).unwrap();
        assert!(!prepared.reusable(&moved, texture_state(), MeshState::default(), now));
        let mut resized = prepare_key(&view);
        resized.size[0] = 1600;
        assert!(!prepared.reusable(&resized, texture_state(), MeshState::default(), now));
        let mut quality = prepare_key(&view);
        quality.camera = quality.camera.with_quality(128.0, 2.0).unwrap();
        assert!(!prepared.reusable(&quality, texture_state(), MeshState::default(), now));
        let mut water = prepare_key(&view);
        water.water_height = Some(21.0);
        assert!(!prepared.reusable(&water, texture_state(), MeshState::default(), now));
        assert!(!prepared.reusable(&prepare_key(&other), texture_state(), MeshState::default(), now));
        scene.close();
        assert!(!prepared.reusable(&prepare_key(&view), texture_state(), MeshState::default(), now));
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
            animation_deadline: None,
        };
        let now = Instant::now();
        for (reason, changed) in [
            ("image readiness or cancellation", TextureState { revision: 1, ..state }),
            ("handle release or asset change", TextureState { lifecycle: 1, ..state }),
            ("staging completion", TextureState { staging: 256, ..state }),
            ("conversion admission", TextureState { jobs: 1, ..state }),
            ("soft cap change", TextureState { cap: 64 * 1024 * 1024, ..state }),
        ] {
            assert!(
                !prepared.reusable(&prepare_key(&view), changed, MeshState::default(), now),
                "{reason} must allow preparation to progress"
            );
        }
        assert!(!prepared.reusable(&prepare_key(&view), state, MeshState { bytes: 4, revision: 0 }, now));
        assert!(!prepared.reusable(&prepare_key(&view), state, MeshState { bytes: 0, revision: 1 }, now));
        prepared.more_meshes = true;
        assert!(!prepared.reusable(&prepare_key(&view), state, MeshState::default(), now));
    }

    #[test]
    fn stepped_animation_waits_until_its_next_update_deadline() {
        let (_scene, view) = Reconciler::new(Budget::default());
        let state = texture_state();
        let now = Instant::now();
        let deadline = now + Duration::from_millis(500);
        let mut prepared = FramePreparation {
            key: prepare_key(&view),
            textures: state,
            meshes: MeshState::default(),
            more_meshes: false,
            animation_deadline: Some(deadline),
        };
        assert!(!prepared.needs_frame(state, MeshState::default(), now));
        assert!(prepared.reusable(&prepare_key(&view), state, MeshState::default(), now));
        assert!(prepared.needs_frame(state, MeshState::default(), deadline));
        assert!(!prepared.reusable(&prepare_key(&view), state, MeshState::default(), deadline));
        prepared.animation_deadline = None;
        assert!(!prepared.needs_frame(state, MeshState::default(), deadline));
    }

    #[test]
    fn scale_winding_change_updates_face_pipeline() {
        let (_scene, view, _) = crate::geometry::fixtures::scene(255);
        let object = view.objects().next().expect("fixture object");
        let (_, mesh) = object.mesh().expect("fixture mesh");
        let Placement::Ready(transform) = object.placement() else { panic!("fixture placement") };
        let info = mesh_info(mesh);
        let mut faces = cached_faces(
            object_item_id(object),
            mesh,
            &info,
            object.materials(),
            object.detail().surface_appearance.as_ref(),
            transform.scale,
            ShadingMode::Studio,
        );
        let original = faces[0].draw.pipeline;
        let flipped = Vec3::new(-transform.scale.x, transform.scale.y, transform.scale.z);
        assert!(refresh_winding_pipeline(&mut faces, transform.scale, flipped));
        assert_ne!(faces[0].draw.pipeline, original);
        assert_eq!(faces[0].draw.pipeline, surface_pipeline(faces[0].base_params.blend, faces[0].base_params.double_sided, flipped));
        assert!(!refresh_winding_pipeline(&mut faces, flipped, flipped));
    }
}
