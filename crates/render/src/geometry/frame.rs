use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use assets::Priority;
use math::{Frustum, Quat, Vec3, flips_winding};
use wgpu::{CommandEncoder, Device};
use world::FaceAppearance;

use super::{FramePreparation, FramePreparationKey, GeometryRenderer, Instance, MAX_INSTANCES, PickFace, PrepareTimes, ShadingMode, Stats};
use crate::FrameInput;
use crate::environment::Evaluated;
use crate::geometry::{
    batching::{Batches, DrawKey},
    instance::{instance_base, is_visible, packed_instance},
};
use crate::material::{FaceMaterial, FaceParams};
use crate::pipeline::{COLOR, Pipeline, TexturePair, blend_keys};
use crate::scene::{Camera, Placement, PreparedMesh, SceneView, SurfaceKind, texture_animation};
use crate::texture::TextureKey;

#[derive(Clone, Copy, Default)]
struct EnvironmentPairs {
    sun: TexturePair,
    moon: TexturePair,
    cloud: TexturePair,
    bloom: TexturePair,
    rainbow: TexturePair,
    halo: TexturePair,
    water_normal: TexturePair,
}

impl GeometryRenderer {
    pub(crate) fn prepare(
        &mut self,
        device: &Device,
        encoder: &mut CommandEncoder,
        view: &SceneView,
        camera: Camera,
        size: [u32; 2],
        frame_input: FrameInput<'_>,
    ) {
        let started = Instant::now();
        let animation_now = started;
        let mut times = PrepareTimes::default();
        let key = FramePreparationKey { version: view.geometry_version(), camera, size };
        self.epoch = self.epoch.checked_add(1).expect("render frame space exhausted");
        for id in &self.visible_textures {
            self.textures.touch(*id, self.epoch);
        }
        if let Some(environment) = frame_input.environment {
            request_environment_textures(view, environment, &mut self.textures, self.epoch);
        }
        let projection = self.pipelines.update_camera_uniform(&self.queue, camera, size);
        let same = self.prepared.as_ref().is_some_and(|prepared| prepared.key == key);
        let texture_progress =
            self.prepared.as_ref().is_some_and(|prepared| prepared.textures != self.textures.state()) || self.textures.uploading();
        if same && texture_progress {
            let upload_started = Instant::now();
            self.textures.upload(device, encoder, self.epoch);
            times.textures = upload_started.elapsed();
        }
        if self.prepared.as_ref().is_some_and(|prepared| prepared.reusable(&key, self.textures.geometry_state(), self.memory.mesh_state()))
        {
            let pairs = environment_pairs(device, view, &mut self.textures, frame_input.environment);
            self.update_environment_uniforms(device, camera, projection, size, frame_input, pairs);
            if let Some(prepared) = self.prepared.as_mut() {
                prepared.textures = self.textures.state();
                prepared.geometry_textures = self.textures.geometry_state();
                prepared.meshes = self.memory.mesh_state();
            }
            self.stats.uploaded_bytes = 0;
            self.memory_stats();
            self.record_prepare(started, times, 0);
            return;
        }
        self.requested_textures.clear();
        self.visible_textures.clear();
        let visibility_started = Instant::now();
        self.stats = Stats::default();
        self.visible.clear();
        let objects = view.objects().filter_map(|object| {
            let (Some((_, mesh)), Placement::Ready(transform)) = (object.mesh(), object.placement()) else { return None };
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
                    scale: transform.scale,
                    pickable: true,
                    faces: Vec::new(),
                    shading: ShadingMode::Surface,
                    region_water_id: None,
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
                SurfaceKind::RegionWater => (0, 0, ShadingMode::RegionWater),
            };
            Some((
                (kind, u128::from(surface.region.get()), index),
                Instance {
                    mesh: mesh.clone(),
                    position: surface.position,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                    pickable: surface.kind != SurfaceKind::RegionWater,
                    faces: Vec::new(),
                    shading,
                    region_water_id: (surface.kind == SurfaceKind::RegionWater).then(|| u128::from(surface.region.get())),
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
        let mut request = |face: &FaceMaterial, priority| {
            let Some(texture) = face.base_color_texture.as_ref().and_then(|binding| binding.image()) else { return };
            if priority == Priority::Visible {
                self.visible_textures.insert(texture.id());
            }
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
        for (_, instance, faces, _, _) in &candidates {
            if let Some(mesh) = self.meshes.get_mut(&instance.mesh.id()) {
                mesh.used = self.epoch;
            }
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
        times.requests = requests_started.elapsed();
        let upload_started = Instant::now();
        if !same {
            self.textures.upload(device, encoder, self.epoch);
        }
        times.textures += upload_started.elapsed();
        let pairs = environment_pairs(device, view, &mut self.textures, frame_input.environment);
        let faces_started = Instant::now();
        let mut batches = Batches::default();
        self.region_water_count = 0;
        let mut uploaded = 0;
        let mut pending_meshes = HashSet::new();
        let mut pending_textures = HashSet::new();
        let mut animated_faces = false;
        for (id, mut instance, faces, animation, appearance) in candidates {
            if !self.upload_mesh(device, &instance.mesh, &mut uploaded) {
                pending_meshes.insert(instance.mesh.id());
                continue;
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
                    }
                    ShadingMode::RegionWater => {}
                    ShadingMode::Surface => {}
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
                let pipeline = if matches!(instance.shading, ShadingMode::RegionWater) {
                    Pipeline::RegionWater
                } else {
                    surface_pipeline(params.blend, params.double_sided, instance.scale)
                };
                let key = DrawKey { mesh: instance.mesh.id(), start: face.indices.start, end: face.indices.end, pipeline, texture };
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
                if let Some(region) = instance.region_water_id {
                    self.include_region_water(region, instance.position, camera.position());
                }
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
        self.update_environment_uniforms(device, camera, projection, size, frame_input, pairs);
        self.stats.instances = self.visible.len();
        self.stats.draw_calls = self.draws.len() + usize::from(self.void_water.active);
        self.memory_stats();
        self.prepared = Some(FramePreparation {
            key,
            textures: self.textures.state(),
            geometry_textures: self.textures.geometry_state(),
            meshes: self.memory.mesh_state(),
            more_meshes: !pending_meshes.is_empty() && uploaded > 0,
            animated_faces,
        });
        self.stats.pending_textures = pending_textures.len();
        self.stats.pending_meshes = pending_meshes.len();
        times.instances = instances_started.elapsed();
        self.record_prepare(started, times, uploaded);
    }

    fn update_environment_uniforms(
        &mut self,
        device: &Device,
        camera: Camera,
        projection: math::Mat4,
        size: [u32; 2],
        frame_input: FrameInput<'_>,
        pairs: EnvironmentPairs,
    ) {
        let normal = pairs.water_normal;
        self.pipelines.update_environment(
            device,
            &self.textures,
            &self.queue,
            camera.position(),
            frame_input.environment,
            normal,
            frame_input.water_height,
            frame_input.water_scroll,
        );
        self.void_water.update(
            device,
            &self.textures,
            &self.queue,
            camera,
            projection.inverse(),
            frame_input.water_height,
            frame_input.environment,
            normal,
            frame_input.water_scroll,
            &self.region_water_bounds,
            self.region_water_count,
        );
        self.sky.update(
            device,
            &self.textures,
            &self.queue,
            projection,
            projection.inverse(),
            size,
            frame_input.environment,
            frame_input.cloud_scroll,
            frame_input.animation_time,
            pairs.sun,
            pairs.moon,
            pairs.cloud,
            pairs.bloom,
            pairs.rainbow,
            pairs.halo,
        );
    }

    fn include_region_water(&mut self, region: u128, position: math::DVec3, camera: math::DVec3) {
        if self.region_water_ids.get(..self.region_water_count).is_some_and(|ids| ids.contains(&region)) {
            return;
        }
        let relative = (position - camera).as_vec3();
        if !relative.is_finite() {
            return;
        }
        let Some(cutout) = self.region_water_bounds.get_mut(self.region_water_count) else { return };
        *cutout = math::Vec4::new(relative.x, relative.y, 256.0, 256.0);
        let Some(id) = self.region_water_ids.get_mut(self.region_water_count) else { return };
        *id = region;
        self.region_water_count += 1;
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

fn surface_pipeline(blend: bool, double_sided: bool, scale: Vec3) -> Pipeline {
    match (blend, double_sided, flips_winding(scale)) {
        (true, true, _) => Pipeline::BlendDoubleSided,
        (true, false, true) => Pipeline::BlendFlipped,
        (true, false, false) => Pipeline::Blend,
        (false, true, _) => Pipeline::DoubleSided,
        (false, false, true) => Pipeline::FlippedWinding,
        (false, false, false) => Pipeline::Surface,
    }
}

fn request_environment_textures(view: &SceneView, environment: &Evaluated, textures: &mut super::Textures, epoch: u64) {
    for blend in [
        environment.sky.sun.texture,
        environment.sky.moon.texture,
        environment.sky.clouds.texture,
        environment.sky.bloom,
        environment.sky.rainbow,
        environment.sky.halo,
        environment.water.normal_map,
    ] {
        for id in [blend.current, blend.next].into_iter().flatten() {
            if let Some(handle) = view.environment_texture(id) {
                textures.request_environment(handle, epoch);
            }
        }
    }
}

fn environment_pairs(
    device: &Device,
    view: &SceneView,
    textures: &mut super::Textures,
    environment: Option<&Evaluated>,
) -> EnvironmentPairs {
    let Some(environment) = environment else { return EnvironmentPairs::default() };
    EnvironmentPairs {
        sun: blend_keys(device, environment.sky.sun.texture, view, textures),
        moon: blend_keys(device, environment.sky.moon.texture, view, textures),
        cloud: blend_keys(device, environment.sky.clouds.texture, view, textures),
        bloom: blend_keys(device, environment.sky.bloom, view, textures),
        rainbow: blend_keys(device, environment.sky.rainbow, view, textures),
        halo: blend_keys(device, environment.sky.halo, view, textures),
        water_normal: blend_keys(device, environment.water.normal_map, view, textures),
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
            version: view.geometry_version(),
            camera: Camera::new(DVec3::ZERO, Vec3::X, 1.0, [720, 720]).unwrap(),
            size: [1280, 720],
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
            geometry_textures: crate::texture::GeometryTextureState::default(),
            meshes: MeshState::default(),
            more_meshes: false,
            animated_faces: false,
        };
        assert!(prepared.reusable(&prepare_key(&view), crate::texture::GeometryTextureState::default(), MeshState::default()));
        let mut moved = prepare_key(&view);
        moved.camera = Camera::new(DVec3::X, Vec3::X, 1.0, [720, 720]).unwrap();
        assert!(
            !prepared.reusable(&moved, crate::texture::GeometryTextureState::default(), MeshState::default()),
            "camera movement changes origin-relative transforms and blend sorting"
        );
        let mut resized = prepare_key(&view);
        resized.size[0] = 1600;
        assert!(
            !prepared.reusable(&resized, crate::texture::GeometryTextureState::default(), MeshState::default()),
            "aspect ratio changes visibility"
        );
        let mut quality = prepare_key(&view);
        quality.camera = quality.camera.with_quality(128.0, 2.0).unwrap();
        assert!(!prepared.reusable(&quality, crate::texture::GeometryTextureState::default(), MeshState::default()));
        assert!(
            !prepared.reusable(&prepare_key(&other), crate::texture::GeometryTextureState::default(), MeshState::default()),
            "another login cannot reuse the old scene"
        );
        scene.close();
        assert!(!prepared.reusable(&prepare_key(&view), crate::texture::GeometryTextureState::default(), MeshState::default()));
    }

    #[test]
    fn preparation_tracks_gpu_progress() {
        let (_scene, view) = Reconciler::new(Budget::default());
        let state = texture_state();
        let mut prepared = FramePreparation {
            key: prepare_key(&view),
            textures: state,
            geometry_textures: crate::texture::GeometryTextureState::default(),
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
            assert!(prepared.needs_frame(changed, crate::texture::GeometryTextureState::default(), MeshState::default()), "{reason} must allow preparation to progress");
            assert!(prepared.reusable(&prepare_key(&view), crate::texture::GeometryTextureState::default(), MeshState::default()));
        }
        assert!(
            prepared.reusable(&prepare_key(&view), crate::texture::GeometryTextureState::default(), MeshState::default()),
            "environment texture progress does not rebuild geometry"
        );
        assert!(
            !prepared.reusable(&prepare_key(&view), crate::texture::GeometryTextureState::default(), MeshState { bytes: 4, revision: 0 }),
            "mesh retirement can unblock uploads"
        );
        assert!(
            !prepared.reusable(&prepare_key(&view), crate::texture::GeometryTextureState::default(), MeshState { bytes: 0, revision: 1 }),
            "released ranges can unblock uploads without changing page bytes"
        );
        assert!(
            !prepared.reusable(&prepare_key(&view), crate::texture::GeometryTextureState(1), MeshState::default()),
            "object texture readiness changes draw bindings"
        );
        prepared.more_meshes = true;
        assert!(
            !prepared.reusable(&prepare_key(&view), crate::texture::GeometryTextureState::default(), MeshState::default()),
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
            geometry_textures: crate::texture::GeometryTextureState::default(),
            meshes: MeshState::default(),
            more_meshes: false,
            animated_faces: false,
        };

        assert!(!prepared.needs_frame(state, crate::texture::GeometryTextureState::default(), MeshState::default()), "a static scene needs no frame after preparation");
        prepared.animated_faces = true;
        assert!(prepared.needs_frame(state, crate::texture::GeometryTextureState::default(), MeshState::default()), "a visible animated face keeps the frame scheduler active");
        prepared.animated_faces = false;
        assert!(!prepared.needs_frame(state, crate::texture::GeometryTextureState::default(), MeshState::default()), "removing the final visible animated face stops redraws");
    }
}
