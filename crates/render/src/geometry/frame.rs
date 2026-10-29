use super::{FramePreparation, FramePreparationKey, GeometryRenderer, PrepareTimes, ShadingMode, Stats};
use crate::{
    geometry::{
        cache::{CacheUpdate, CachedFace, ItemId},
        draw_list::MAX_FACE_INSTANCES,
        instance::{instance_base, parameter_part, scene_origin},
    },
    material::FaceParams,
    pipeline::COLOR,
    scene::{Camera, Object, SceneView, texture_animation},
};
use std::time::{Duration, Instant};
use wgpu::{CommandEncoder, Device};

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
        self.stats.layouts = 0;
        self.stats.items_updated = 0;
        self.stats.faces_patched = 0;
        self.stats.blended_sorts = 0;
        self.stats.animated_faces_visited = 0;
        self.stats.animated_faces_changed = 0;
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

        self.stats = Stats::default();
        self.epoch = self.epoch.checked_add(1).expect("render frame space exhausted");
        let previous_camera = self.cache.camera;
        let camera_changed = previous_camera != Some(camera) || self.cache.size != size;
        let camera_position_changed = previous_camera.is_none_or(|previous| previous.position() != camera.position());
        let origin = scene_origin(camera.position());
        let origin_rebased = previous_camera.is_none() || self.cache.origin != origin;

        let visibility_started = Instant::now();
        let mut update = self.update_cache(view);
        self.stats.items_updated = update.items_updated;
        update.pose_items.sort_unstable();
        update.pose_items.dedup();
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

        let meshes_started = Instant::now();
        let previous_mesh_state = self.cache.mesh_state;
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
            self.cache.mesh_state = self.memory.mesh_state();
        }
        if self.cache.mesh_state != previous_mesh_state {
            self.draw_list.invalidate_batches();
        }
        let rebuild_visible = candidates_changed || mesh_progress;
        if rebuild_visible {
            self.cache.next_visible.clear();
            for id in &self.cache.candidates {
                let Some(item) = self.cache.items.get(id) else { continue };
                let mesh_ready =
                    if mesh_progress { self.cache.mesh_ready.contains(&item.mesh.id()) } else { self.meshes.contains_key(&item.mesh.id()) };
                if mesh_ready && !item.faces.is_empty() {
                    self.cache.next_visible.push(*id);
                }
            }
        }
        times.meshes = meshes_started.elapsed();
        let visible_changed = rebuild_visible && self.cache.visible != self.cache.next_visible;
        let layout = !self.draw_list.valid() || update.layout_changed || origin_rebased || visible_changed;

        let draw_list_started = Instant::now();
        if origin_rebased {
            for item in self.cache.items.values_mut() {
                item.transform = instance_base(item.position, item.rotation, item.scale, origin);
            }
        }
        if layout {
            let visible = if visible_changed { &self.cache.next_visible } else { &self.cache.visible };
            let stats = self.draw_list.layout(&self.cache.visible, visible, &mut self.cache.items, camera);
            self.stats.layouts = 1;
            self.omitted_faces = stats.omitted_faces;
            if stats.blended {
                self.stats.blended_sorts += 1;
            }
        }
        let layout_time = draw_list_started.elapsed();

        let requests_started = Instant::now();
        self.request_frame_textures(view, &update.background);
        times.requests = requests_started.elapsed();

        let draw_updates_started = Instant::now();
        self.prepare_face_patches(&update, layout, visible_changed, animation_now);
        let pose_items: &[ItemId] = if layout { &[] } else { &update.pose_items };
        let patch_stats = self.draw_list.patch(&mut self.cache.items, &self.cache.patch_faces, pose_items, camera);
        self.stats.faces_patched = patch_stats.faces_patched;
        let blended_sorted = if layout {
            false
        } else if camera_position_changed && self.draw_list.has_blended() {
            self.draw_list.sort_blended(&mut self.cache.items, camera)
        } else if patch_stats.blended_sorted {
            self.draw_list.reorder_blended(&mut self.cache.items);
            true
        } else {
            false
        };
        self.stats.blended_sorts += usize::from(blended_sorted);
        if layout {
            self.draw_list.upload_layout(&self.queue, &self.instances);
        } else if !self.cache.patch_faces.is_empty() || blended_sorted {
            self.draw_list.upload_patch(&self.queue, &self.instances, !blended_sorted);
        }
        let draw_updates_time = draw_updates_started.elapsed();
        let draw_commands_changed = layout || blended_sorted;
        let draw_batches_started = Instant::now();
        if draw_commands_changed {
            self.draw_list.build_draw_batches(&self.meshes, self.indirect_first_instance);
            let bytes = self.draw_list.indirect_bytes();
            if !bytes.is_empty() {
                self.queue.write_buffer(&self.indirect, 0, bytes);
            }
        }
        let draw_batches_time = draw_batches_started.elapsed();
        times.draw_list = layout_time + draw_updates_time + draw_batches_time;
        let texture_started = Instant::now();
        if !same {
            self.textures.upload(device, encoder, self.epoch);
        }
        let texture_changes = self.textures.take_changed_ids();
        self.prepare_texture_bindings(device, layout, &texture_changes, &update.terrain_regions);
        times.textures += texture_started.elapsed();
        if visible_changed {
            std::mem::swap(&mut self.cache.visible, &mut self.cache.next_visible);
        }
        self.cache.next_visible.clear();
        self.cache.camera = Some(camera);
        self.cache.origin = origin;
        self.cache.size = size;
        self.cache.version = Some(view.version());
        self.cache.next_candidates.clear();
        let animation_deadline =
            self.cache.visible.iter().filter_map(|id| self.cache.items.get(id).and_then(|item| item.animation_deadline)).min();
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
        self.stats.instances = self.draw_list.visible_items();
        let draw_submissions =
            if self.indirect_first_instance { self.draw_list.draw_batches().len() } else { self.draw_list.draws().len() };
        self.stats.draw_calls = draw_submissions + usize::from(self.water.active);
        self.stats.draw_commands = self.draw_list.draws().len() + usize::from(self.water.active);
        self.stats.pending_meshes = self.pending_meshes.len();
        self.stats.pending_textures = self.pending_textures.len();
        if self.stats.omitted_faces > 0 && self.draw_report.is_none_or(|at| at.elapsed() >= Duration::from_secs(5)) {
            self.draw_report = Some(Instant::now());
            tracing::warn!(
                omitted_faces = self.stats.omitted_faces,
                mesh_budget_blocked = self.stats.mesh_budget_blocked,
                instance_capacity = MAX_FACE_INSTANCES,
                instance_buffer_bytes = super::INSTANCE_BUFFER_BYTES,
                draw_submissions = self.stats.draw_calls,
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
        times.draw_list += instances_started.elapsed();
        self.record_prepare(started, times, uploaded);
    }

    fn prepare_face_patches(&mut self, update: &CacheUpdate, layout: bool, visible_changed: bool, now: Instant) {
        let visible = if visible_changed { &self.cache.next_visible } else { &self.cache.visible };
        let items = &mut self.cache.items;
        let patch_items = &mut self.cache.patch_items;
        patch_items.clear();
        if !layout {
            patch_items.extend(update.pose_items.iter().copied());
            patch_items.extend(update.parameter_items.iter().copied());
        }
        for id in self.cache.animated_items.iter().copied() {
            if visible.binary_search(&id).is_ok()
                && items.get(&id).is_some_and(|item| item.animation_deadline.is_some_and(|deadline| deadline <= now))
            {
                patch_items.push(id);
            }
        }
        patch_items.sort_unstable();
        patch_items.dedup();
        self.cache.patch_faces.clear();
        for id in patch_items.iter().copied() {
            let pose_changed = !layout && update.pose_items.binary_search(&id).is_ok();
            let Some(item) = items.get_mut(&id) else { continue };
            let parameters_dirty = item.parameters_dirty;
            let animation_due = item.animation_deadline.is_some_and(|deadline| deadline <= now);
            let animation = item
                .source
                .as_ref()
                .and_then(|object| object.detail().texture_animation)
                .filter(|animation| texture_animation::is_enabled(*animation));
            let full_faces = pose_changed || parameters_dirty;
            let animated_faces = &item.animated_faces;
            let source = item.source.as_ref();
            let shading = item.shading;
            let scale = item.scale;
            let faces = &mut item.faces;
            let count = if full_faces { faces.len() } else { animated_faces.len() };
            for cursor in 0..count {
                let face_index = if full_faces { cursor } else { animated_faces.get(cursor).copied().unwrap_or(usize::MAX) };
                let Some(face) = faces.get_mut(face_index) else { continue };
                if face.slot.is_none() {
                    continue;
                }
                let animated = animation_due
                    && face.base_params.texture.is_some()
                    && animation.is_some_and(|animation| animation.targets_face(face.order.3));
                let refresh_params = parameters_dirty || animated;
                let params_changed = if refresh_params {
                    let params = current_params(source, shading, face, now);
                    let changed = params != face.params;
                    if changed {
                        face.params = params;
                        face.parameter_bytes = parameter_part(&params, scale);
                    }
                    changed
                } else {
                    false
                };
                if animated {
                    self.stats.animated_faces_visited += 1;
                    self.stats.animated_faces_changed += usize::from(params_changed);
                }
                if pose_changed || params_changed {
                    self.cache.patch_faces.push((id, face_index));
                }
            }
            item.parameters_dirty = false;
            if animation_due || parameters_dirty {
                item.animation_deadline = next_animation_deadline(item, now);
            }
        }
    }

    fn record_prepare(&mut self, started: Instant, times: PrepareTimes, uploaded: usize) {
        self.stats.prepare_ms = started.elapsed().as_secs_f64() * 1000.0;
        if self.stats.prepare_ms >= 16.0 && self.slow_report.is_none_or(|at| at.elapsed() >= Duration::from_secs(1)) {
            self.slow_report = Some(Instant::now());
            tracing::warn!(
                total_ms = self.stats.prepare_ms,
                visibility_ms = times.visibility.as_secs_f64() * 1000.0,
                texture_requests_ms = times.requests.as_secs_f64() * 1000.0,
                texture_upload_ms = times.textures.as_secs_f64() * 1000.0,
                mesh_upload_ms = times.meshes.as_secs_f64() * 1000.0,
                draw_list_ms = times.draw_list.as_secs_f64() * 1000.0,
                mesh_uploads = uploaded,
                mesh_upload_bytes = self.stats.uploaded_bytes,
                draw_submissions = self.stats.draw_calls,
                draw_commands = self.stats.draw_commands,
                instances = self.stats.instances,
                pending_meshes = self.stats.pending_meshes,
                pending_textures = self.stats.pending_textures,
                layouts = self.stats.layouts,
                items_updated = self.stats.items_updated,
                faces_patched = self.stats.faces_patched,
                blended_sorts = self.stats.blended_sorts,
                animated_faces_visited = self.stats.animated_faces_visited,
                animated_faces_changed = self.stats.animated_faces_changed,
                omitted_faces = self.stats.omitted_faces,
                "slow scene preparation"
            );
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
    if !item.animated_faces.iter().any(|index| item.faces.get(*index).is_some_and(|face| face.slot.is_some())) {
        return None;
    }
    let source = item.source.as_ref()?;
    let animation = source.detail().texture_animation?;
    let (elapsed, offset) = source.animation_timing(now)?;
    texture_animation::next_update_after(animation, elapsed, offset).and_then(|delay| now.checked_add(delay))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        geometry::{
            MeshState,
            cache::{cached_faces, mesh_info, object_item_id, refresh_winding_pipeline, surface_pipeline},
            draw_list::DrawInterner,
        },
        pipeline::Pipeline,
        scene::{Budget, Placement, Reconciler},
        texture::TextureState,
    };
    use math::{DVec3, Vec3};

    fn prepare_key(view: &SceneView) -> FramePreparationKey {
        FramePreparationKey {
            version: view.version(),
            camera: Camera::new(DVec3::ZERO, Vec3::X, 1.0, [720, 720]).expect("fixture camera"),
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
        moved.camera = Camera::new(DVec3::X, Vec3::X, 1.0, [720, 720]).expect("moved camera");
        assert!(!prepared.reusable(&moved, texture_state(), MeshState::default(), now));
        let mut resized = prepare_key(&view);
        resized.size[0] = 1600;
        assert!(!prepared.reusable(&resized, texture_state(), MeshState::default(), now));
        let mut quality = prepare_key(&view);
        quality.camera = quality.camera.with_quality(128.0, 2.0).expect("changed quality");
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
    fn stepped_animation_waits_for_deadline() {
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
    fn surface_faces_keep_surface_pipelines() {
        let (_scene, view, _) = crate::geometry::fixtures::scene(255);
        let object = view.objects().next().expect("fixture object");
        let (_, mesh) = object.mesh().expect("fixture mesh");
        let info = mesh_info(mesh);
        let mut interner = DrawInterner::default();
        for (shading, pipeline) in [(ShadingMode::Terrain, Pipeline::Terrain), (ShadingMode::Water, Pipeline::Water)] {
            let faces = cached_faces((1, 1, 0), mesh, &info, None, None, Vec3::ONE, shading, None, None, &mut interner);
            assert!(!faces.is_empty());
            assert!(faces.iter().all(|face| interner.key(face.group).is_some_and(|key| key.pipeline == pipeline)));
        }
    }

    #[test]
    fn scale_winding_rekeys_faces() {
        let (_scene, view, _) = crate::geometry::fixtures::scene(255);
        let object = view.objects().next().expect("fixture object");
        let (_, mesh) = object.mesh().expect("fixture mesh");
        let Placement::Ready(transform) = object.placement() else { panic!("fixture placement") };
        let info = mesh_info(mesh);
        let mut interner = DrawInterner::default();
        let mut faces = cached_faces(
            object_item_id(object),
            mesh,
            &info,
            object.materials(),
            object.detail().surface_appearance.as_ref(),
            transform.scale,
            ShadingMode::Studio,
            None,
            None,
            &mut interner,
        );
        let original = interner.key(faces[0].group).expect("fixture draw group").pipeline;
        let flipped = Vec3::new(-transform.scale.x, transform.scale.y, transform.scale.z);
        assert!(refresh_winding_pipeline(&mut faces, transform.scale, flipped, &mut interner));
        let key = interner.key(faces[0].group).expect("flipped draw group");
        assert_ne!(key.pipeline, original);
        assert_eq!(key.pipeline, surface_pipeline(faces[0].base_params.blend, faces[0].base_params.double_sided, flipped));
        assert!(!refresh_winding_pipeline(&mut faces, flipped, flipped, &mut interner));
    }
}
