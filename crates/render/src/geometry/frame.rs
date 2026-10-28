use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use assets::Priority;
use math::{Frustum, Quat, Vec3, flips_winding};
use wgpu::{CommandEncoder, Device};
use world::FaceAppearance;

use super::{FramePreparation, FramePreparationKey, GeometryRenderer, Instance, MAX_INSTANCES, PickFace, PrepareTimes, ShadingMode, Stats};
use crate::geometry::{
    batching::{Batches, DrawKey},
    instance::{instance_base, is_visible, packed_instance},
    mesh::resident_mesh_fallback,
};
use crate::material::{FaceMaterial, FaceParams};
use crate::pipeline::{COLOR, Pipeline};
use crate::scene::{Camera, MeshId, Placement, PreparedMesh, SceneView, SharedMesh, SurfaceKind, texture_animation};

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
        self.textures.clear_terrain_bindings();
        let visibility_started = Instant::now();
        self.epoch = self.epoch.checked_add(1).expect("render frame space exhausted");
        self.stats = Stats::default();
        self.visible.clear();
        let projection = self.pipelines.update_camera_uniform(&self.queue, camera, size);
        self.water.update(&self.queue, view, camera, projection, water_height);
        let objects = view.objects().filter_map(|object| {
            let (Some(mesh), Placement::Ready(transform)) = (object.render_mesh(), object.placement()) else { return None };
            let flags = object.mesh_flags().unwrap_or_default();
            let scale = transform.scale * Vec3::new(if flags.mirror { -1.0 } else { 1.0 }, 1.0, 1.0);
            let animation = object
                .detail()
                .texture_animation
                .zip(object.animation_timing(animation_now))
                .map(|(animation, (elapsed, offset))| (animation, elapsed, offset));
            Some((
                (2, object.detail().summary.id.as_u128(), 0),
                Instance {
                    mesh: mesh.clone(),
                    position: transform.pose.position,
                    rotation: transform.pose.rotation,
                    scale,
                    invert_normals: flags.invert,
                    pickable: true,
                    faces: Vec::new(),
                    shading: ShadingMode::Studio,
                    terrain_textures: None,
                    terrain_composition: None,
                },
                object.materials(),
                animation,
                object.detail().surface_appearance.as_ref(),
            ))
        });
        let surfaces = view.surfaces().filter_map(|surface| {
            let PreparedMesh::Ready(mesh) = &surface.mesh else { return None };
            let (kind, index, shading) = match surface.kind {
                SurfaceKind::Terrain(patch) => {
                    let (x, y) = patch.coordinates();
                    (1, u16::from(y) * 16 + u16::from(x), ShadingMode::Terrain)
                }
                SurfaceKind::Water => (0, 0, ShadingMode::Water),
            };
            let terrain_textures = if shading == ShadingMode::Terrain {
                Some(view.region(surface.region).map_or([None, None, None, None], |region| region.terrain_textures().clone()))
            } else {
                None
            };
            Some((
                (kind, u128::from(surface.region.get()), index),
                Instance {
                    mesh: mesh.clone(),
                    position: surface.position,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                    invert_normals: false,
                    pickable: surface.kind != SurfaceKind::Water,
                    faces: Vec::new(),
                    shading,
                    terrain_textures,
                    terrain_composition: surface.terrain_composition.clone(),
                },
                None,
                None,
                None,
            ))
        });
        let mut candidates = Vec::new();
        let frustum = Frustum::from_clip_matrix(projection);
        for candidate in surfaces.chain(objects) {
            if is_visible(&candidate.1, camera, &frustum) {
                candidates.push(candidate);
            }
            if candidates.len() == MAX_INSTANCES {
                break;
            }
        }
        candidates.sort_unstable_by_key(|(id, _, _, _, _)| *id);
        times.visibility = visibility_started.elapsed();
        let requests_started = Instant::now();
        self.requested_textures.clear();
        for (_, instance, _, _, _) in &candidates {
            if let Some(textures) = &instance.terrain_textures {
                for texture in textures.iter().flatten() {
                    self.request_texture(texture, Priority::Visible);
                }
            }
        }
        let mut request = |face: &FaceMaterial, priority| {
            let Some(texture) = face.base_color_texture.as_ref().and_then(|binding| binding.image()) else { return };
            if self.requested_textures.contains(&texture.id()) {
                return;
            }
            if priority == Priority::Background && !texture.info().is_some_and(|info| info.size.into_iter().all(|edge| edge <= 32)) {
                self.requested_textures.insert(texture.id());
                return;
            }
            if FaceParams::new(Some(face)).texture.is_none() {
                return;
            }
            self.requested_textures.insert(texture.id());
            self.textures.request(texture, self.epoch, priority);
        };
        for (_, _, faces, _, _) in &candidates {
            if let Some(faces) = faces {
                for face in *faces {
                    request(face, Priority::Visible);
                }
            }
        }
        for object in view.objects() {
            if let Some(faces) = object.materials() {
                for face in faces {
                    request(face, Priority::Background);
                }
            }
        }
        drop(request);
        for (_, instance, _, _, _) in &candidates {
            if let Some(mesh) = self.meshes.get_mut(&instance.mesh.id()) {
                mesh.used = self.epoch;
            }
        }
        times.requests = requests_started.elapsed();
        let upload_started = Instant::now();
        if !same {
            self.textures.upload(device, encoder, self.epoch);
        }
        times.textures += upload_started.elapsed();
        let faces_started = Instant::now();
        let mut batches = Batches::default();
        let mut uploaded = 0;
        let mut mesh_fallbacks: HashMap<MeshId, Option<(MeshId, SharedMesh)>> = HashMap::new();
        let mut pending_meshes = HashSet::new();
        let mut pending_textures = HashSet::new();
        let mut animated_faces = false;
        for (id, mut instance, faces, animation, appearance) in candidates {
            let selected_id = instance.mesh.id();
            let fallback = if self.meshes.contains_key(&selected_id) {
                None
            } else if let Some(fallback) = mesh_fallbacks.get(&selected_id) {
                fallback.clone()
            } else {
                let fallback = instance.mesh.asset_lod().and_then(|(asset, lod)| {
                    let fallback_id = resident_mesh_fallback(&self.asset_meshes, asset, lod, selected_id)?;
                    self.meshes.get(&fallback_id)?.shared.clone().map(|mesh| (fallback_id, mesh))
                });
                mesh_fallbacks.insert(selected_id, fallback.clone());
                fallback
            };
            if !self.upload_mesh(device, &instance.mesh, instance.terrain_composition.as_deref(), &mut uploaded) {
                pending_meshes.insert(selected_id);
                let Some((fallback_id, fallback)) = fallback else { continue };
                let Some(mesh) = self.meshes.get_mut(&fallback_id) else { continue };
                mesh.used = self.epoch;
                instance.mesh = fallback;
            }
            let base = instance_base(&instance, camera.position());
            for (index, face) in instance.mesh.mesh().faces().iter().enumerate() {
                if face.indices.is_empty() {
                    continue;
                }
                let animation =
                    animation.filter(|(animation, _, _)| texture_animation::is_enabled(*animation) && animation.targets_face(face.slot));
                let texture_entry_transform = animation.map(|(animation, elapsed, offset)| {
                    texture_animation::transform_at(
                        animation,
                        elapsed,
                        offset,
                        appearance
                            .and_then(|appearance| appearance.face(face.slot))
                            .map_or_else(FaceAppearance::default, |face| *face)
                            .transform,
                    )
                });
                let animate_frame =
                    animation.is_some_and(|(animation, elapsed, offset)| texture_animation::is_changing_at(animation, elapsed, offset));
                let mut params = FaceParams::with_texture_entry_transform(
                    faces.and_then(|faces| faces.iter().find(|material| material.slot == face.slot)),
                    texture_entry_transform,
                );
                match instance.shading {
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
                let texture = params.texture.and_then(|key| self.textures.resident(device, key));
                if texture.is_none()
                    && let Some(key) = params.texture
                {
                    pending_textures.insert(key.id);
                }
                let terrain = instance.terrain_textures.as_ref().map(|textures| {
                    let key = self.textures.terrain_key(textures, self.epoch);
                    self.textures.prepare_terrain_binding(device, key);
                    for (texture, key) in textures.iter().zip(key.0) {
                        if key.is_none()
                            && let Some(texture) = texture
                        {
                            pending_textures.insert(texture.id());
                        }
                    }
                    key
                });
                let pipeline = match instance.shading {
                    ShadingMode::Terrain => Pipeline::Terrain,
                    ShadingMode::Water => Pipeline::Water,
                    ShadingMode::Studio => surface_pipeline(params.blend, params.double_sided, instance.scale, instance.invert_normals),
                };
                let key =
                    DrawKey { mesh: instance.mesh.id(), start: face.indices.start, end: face.indices.end, pipeline, texture, terrain };
                let order = (id.0, id.1, id.2, face.slot);
                let distance_squared = params.blend.then(|| {
                    let center = self.meshes[&key.mesh].centers[index];
                    let relative = instance.position - camera.position() + (instance.rotation * (center * instance.scale)).as_dvec3();
                    relative.length_squared()
                });
                if !batches.push(key, order, distance_squared, || packed_instance(&base, &params, instance.scale)) {
                    continue;
                }
                animated_faces |= animate_frame && params.texture.is_some();
                instance.faces.push(PickFace { indices: face.indices.clone(), double_sided: params.double_sided });
            }
            if !instance.faces.is_empty() {
                self.visible.push(instance);
            }
        }
        times.faces = faces_started.elapsed();
        let instances_started = Instant::now();
        self.stats.omitted_faces = batches.omitted();
        let bytes = batches.finish(&mut self.draws);
        if !bytes.is_empty() {
            self.queue.write_buffer(&self.instances, 0, &bytes);
        }
        self.stats.instances = self.visible.len();
        self.stats.draw_calls = self.draws.len() + usize::from(self.water.active);
        self.memory_stats();
        self.prepared = Some(FramePreparation {
            key,
            textures: self.textures.state(),
            meshes: self.memory.mesh_state(),
            more_meshes: !pending_meshes.is_empty() && uploaded > 0,
            animated_faces,
        });
        self.stats.pending_textures = pending_textures.len();
        self.stats.pending_meshes = pending_meshes.len();
        times.instances = instances_started.elapsed();
        self.record_prepare(started, times, uploaded);
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

    fn request_texture(&mut self, texture: &assets::TextureHandle, priority: Priority) {
        if self.requested_textures.contains(&texture.id()) {
            return;
        }
        if priority == Priority::Background && !texture.info().is_some_and(|info| info.size.into_iter().all(|edge| edge <= 32)) {
            self.requested_textures.insert(texture.id());
            return;
        }
        self.requested_textures.insert(texture.id());
        self.textures.request(texture, self.epoch, priority);
    }
}

fn surface_pipeline(blend: bool, double_sided: bool, scale: Vec3, invert_normals: bool) -> Pipeline {
    match (blend, double_sided, flips_winding(scale) ^ invert_normals) {
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
        assert!(
            !prepared.reusable(&moved, texture_state(), MeshState::default()),
            "camera movement changes origin-relative transforms and blend sorting"
        );
        let mut resized = prepare_key(&view);
        resized.size[0] = 1600;
        assert!(!prepared.reusable(&resized, texture_state(), MeshState::default()), "aspect ratio changes visibility");
        let mut quality = prepare_key(&view);
        quality.camera = quality.camera.with_quality(128.0, 2.0).unwrap();
        assert!(!prepared.reusable(&quality, texture_state(), MeshState::default()));
        let mut water = prepare_key(&view);
        water.water_height = Some(21.0);
        assert!(!prepared.reusable(&water, texture_state(), MeshState::default()));
        assert!(
            !prepared.reusable(&prepare_key(&other), texture_state(), MeshState::default()),
            "another login cannot reuse the old scene"
        );
        scene.close();
        assert!(!prepared.reusable(&prepare_key(&view), texture_state(), MeshState::default()), "session closure invalidates cached draws");
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
        assert!(!prepared.reusable(&prepare_key(&view), state, MeshState { bytes: 4, revision: 0 }), "mesh retirement can unblock uploads");
        assert!(
            !prepared.reusable(&prepare_key(&view), state, MeshState { bytes: 0, revision: 1 }),
            "released ranges can unblock uploads without changing page bytes"
        );
        prepared.more_meshes = true;
        assert!(
            !prepared.reusable(&prepare_key(&view), state, MeshState::default()),
            "per-frame upload limits must not strand remaining meshes"
        );
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

        assert!(!prepared.needs_frame(state, MeshState::default()), "a static scene needs no frame after preparation");
        prepared.animated_faces = true;
        assert!(prepared.needs_frame(state, MeshState::default()), "a visible animated face keeps the frame scheduler active");
        prepared.animated_faces = false;
        assert!(!prepared.needs_frame(state, MeshState::default()), "removing the final visible animated face stops redraws");
    }
}
