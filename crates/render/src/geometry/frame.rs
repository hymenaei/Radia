use math::flips_winding;

use super::{
    COLOR, Camera, CommandEncoder, Device, DrawCommand, Duration, FaceMaterial, FaceParams, FramePreparation, FramePreparationKey, Frustum,
    GeometryRenderer, HashMap, HashSet, INSTANCE_BYTES, Instance, Instant, MAX_DRAWS, MAX_FACE_INSTANCES, MAX_INSTANCES, MeshId, Ordering,
    PickFace, Pipeline, Placement, PrepareTimes, PreparedMesh, Priority, Quat, SceneView, ShadingMode, Stats, SurfaceKind, TextureKey,
    Vec3, instance_base, is_visible, packed_instance,
};

struct Group {
    first: (u8, u128, u16, u8),
    bytes: Vec<u8>,
    count: u32,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct DrawKey {
    mesh: MeshId,
    start: u32,
    end: u32,
    pipeline: Pipeline,
    texture: Option<TextureKey>,
}

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
                .is_some_and(|prepared| prepared.reusable(&key, self.textures.state(), self.memory.bytes.load(Ordering::Acquire)))
            {
                self.stats.uploaded_bytes = 0;
                self.memory_stats();
                self.record_prepare(started, times, 0);
                return;
            }
        }
        let visibility_started = Instant::now();
        self.epoch = self.epoch.checked_add(1).expect("render frame space exhausted");
        self.stats = Stats::default();
        self.draws.clear();
        self.visible.clear();
        let projection = self.studio.update_camera_uniform(&self.queue, camera, size);
        self.water.update(&self.queue, view, camera, projection, water_height);
        let objects = view.objects().filter_map(|object| {
            let (Some((_, mesh)), Placement::Ready(transform)) = (object.mesh(), object.placement()) else { return None };
            Some((
                (2, object.detail().summary.id.as_u128(), 0),
                Instance {
                    mesh: mesh.clone(),
                    position: transform.pose.position,
                    rotation: transform.pose.rotation,
                    scale: transform.scale,
                    pickable: true,
                    faces: Vec::new(),
                    shading: ShadingMode::Studio,
                },
                object.materials(),
            ))
        });
        let surfaces = view.surfaces().filter_map(|surface| {
            let PreparedMesh::Ready(mesh) = &surface.mesh else { return None };
            let (kind, index, shading) = match surface.kind {
                SurfaceKind::Ground(patch) => {
                    let (x, y) = patch.coordinates();
                    (1, u16::from(y) * 16 + u16::from(x), ShadingMode::Ground)
                }
                SurfaceKind::Water => (0, 0, ShadingMode::Water),
            };
            Some((
                (kind, u128::from(surface.region.get()), index),
                Instance {
                    mesh: mesh.clone(),
                    position: surface.position,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                    pickable: surface.kind != SurfaceKind::Water,
                    faces: Vec::new(),
                    shading,
                },
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
        candidates.sort_unstable_by_key(|(id, _, _)| *id);
        times.visibility = visibility_started.elapsed();
        let requests_started = Instant::now();
        self.requested_textures.clear();
        let mut request = |face: &FaceMaterial, priority| {
            let Some(texture) = &face.base_color else { return };
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
        for (_, instance, faces) in &candidates {
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
        let faces_started = Instant::now();
        let mut groups: HashMap<DrawKey, Group> = HashMap::new();
        let mut blended = Vec::new();
        let mut uploaded = 0;
        let mut count = 0;
        let mut pending_meshes = HashSet::new();
        let mut pending_textures = HashSet::new();
        for (id, mut instance, faces) in candidates {
            if !self.upload_mesh(device, &instance.mesh, &mut uploaded) {
                pending_meshes.insert(instance.mesh.id());
                continue;
            }
            let base = instance_base(&instance, camera.position());
            for (index, face) in instance.mesh.mesh().faces().iter().enumerate() {
                if face.indices.is_empty() {
                    continue;
                }
                let mut params = FaceParams::new(faces.and_then(|faces| faces.iter().find(|material| material.slot == face.slot)));
                match instance.shading {
                    ShadingMode::Ground => {
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
                let pipeline = if matches!(instance.shading, ShadingMode::Water) {
                    Pipeline::Water
                } else {
                    match (params.blend, params.double_sided, flips_winding(instance.scale)) {
                        (true, true, _) => Pipeline::BlendDoubleSided,
                        (true, false, true) => Pipeline::BlendFlipped,
                        (true, false, false) => Pipeline::Blend,
                        (false, true, _) => Pipeline::DoubleSided,
                        (false, false, true) => Pipeline::FlippedWinding,
                        (false, false, false) => Pipeline::Studio,
                    }
                };
                let key = DrawKey { mesh: instance.mesh.id(), start: face.indices.start, end: face.indices.end, pipeline, texture };
                if count == MAX_FACE_INSTANCES
                    || (groups.len() + blended.len() >= MAX_DRAWS && (params.blend || !groups.contains_key(&key)))
                {
                    self.stats.omitted_faces += 1;
                    continue;
                }
                count += 1;
                let bytes = packed_instance(&base, &params, instance.scale);
                let order = (id.0, id.1, id.2, face.slot);
                if params.blend {
                    let center = self.meshes[&key.mesh].centers[index];
                    let relative = instance.position - camera.position() + (instance.rotation * (center * instance.scale)).as_dvec3();
                    blended.push((relative.length_squared(), order, key, bytes));
                } else {
                    let group = groups.entry(key).or_insert_with(|| Group { first: order, bytes: Vec::new(), count: 0 });
                    group.first = group.first.min(order);
                    group.bytes.extend(bytes);
                    group.count += 1;
                }
                instance.faces.push(PickFace { indices: face.indices.clone(), double_sided: params.double_sided });
            }
            if !instance.faces.is_empty() {
                self.visible.push(instance);
            }
        }
        times.faces = faces_started.elapsed();
        let instances_started = Instant::now();
        let mut bytes = Vec::with_capacity(count * INSTANCE_BYTES);
        let mut groups: Vec<_> = groups.into_iter().collect();
        groups.sort_unstable_by_key(|(_, group)| group.first);
        for (key, group) in groups {
            let first = (bytes.len() / INSTANCE_BYTES) as u32;
            bytes.extend(group.bytes);
            self.draws.push(DrawCommand {
                key: key.mesh,
                pipeline: key.pipeline,
                first,
                count: group.count,
                indices: key.start..key.end,
                texture: key.texture,
            });
        }
        blended.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        for (_, _, key, instance) in blended {
            let first = (bytes.len() / INSTANCE_BYTES) as u32;
            bytes.extend(instance);
            self.draws.push(DrawCommand {
                key: key.mesh,
                pipeline: key.pipeline,
                first,
                count: 1,
                indices: key.start..key.end,
                texture: key.texture,
            });
        }
        if !bytes.is_empty() {
            self.queue.write_buffer(&self.instances, 0, &bytes);
        }
        self.stats.instances = self.visible.len();
        self.stats.draw_calls = self.draws.len() + usize::from(self.water.active);
        self.memory_stats();
        self.prepared = Some(FramePreparation {
            key,
            textures: self.textures.state(),
            mesh_bytes: self.stats.mesh_bytes,
            more_meshes: !pending_meshes.is_empty() && uploaded > 0,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture::TextureState;
    use math::DVec3;
    use scene::{Budget, Reconciler};

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
        let prepared = FramePreparation { key: prepare_key(&view), textures: texture_state(), mesh_bytes: 0, more_meshes: false };
        assert!(prepared.reusable(&prepare_key(&view), texture_state(), 0));
        let mut moved = prepare_key(&view);
        moved.camera = Camera::new(DVec3::X, Vec3::X, 1.0, [720, 720]).unwrap();
        assert!(!prepared.reusable(&moved, texture_state(), 0), "camera movement changes origin-relative transforms and blend sorting");
        let mut resized = prepare_key(&view);
        resized.size[0] = 1600;
        assert!(!prepared.reusable(&resized, texture_state(), 0), "aspect ratio changes visibility");
        let mut quality = prepare_key(&view);
        quality.camera = quality.camera.with_quality(128.0, 2.0).unwrap();
        assert!(!prepared.reusable(&quality, texture_state(), 0));
        let mut water = prepare_key(&view);
        water.water_height = Some(21.0);
        assert!(!prepared.reusable(&water, texture_state(), 0));
        assert!(!prepared.reusable(&prepare_key(&other), texture_state(), 0), "another login cannot reuse the old scene");
        scene.close();
        assert!(!prepared.reusable(&prepare_key(&view), texture_state(), 0), "session closure invalidates cached draws");
    }

    #[test]
    fn preparation_tracks_gpu_progress() {
        let (_scene, view) = Reconciler::new(Budget::default());
        let state = texture_state();
        let mut prepared = FramePreparation { key: prepare_key(&view), textures: state, mesh_bytes: 0, more_meshes: false };
        for (reason, changed) in [
            ("image readiness or cancellation", TextureState { revision: 1, ..state }),
            ("handle release or asset change", TextureState { lifecycle: 1, ..state }),
            ("staging completion", TextureState { staging: 256, ..state }),
            ("conversion admission", TextureState { jobs: 1, ..state }),
            ("soft cap change", TextureState { cap: 64 * 1024 * 1024, ..state }),
        ] {
            assert!(!prepared.reusable(&prepare_key(&view), changed, 0), "{reason} must allow preparation to progress");
        }
        assert!(!prepared.reusable(&prepare_key(&view), state, 4), "mesh retirement can unblock uploads");
        prepared.more_meshes = true;
        assert!(!prepared.reusable(&prepare_key(&view), state, 0), "per-frame upload limits must not strand remaining meshes");
    }
}
