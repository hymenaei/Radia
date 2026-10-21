use super::*;
use scene::{Budget, SceneReconciler};

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
    let (mut scene, view) = SceneReconciler::new(Budget::default());
    let (_other_scene, other) = SceneReconciler::new(Budget::default());
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
    let (_scene, view) = SceneReconciler::new(Budget::default());
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

#[test]
fn instance_attributes_match_shader() {
    let base = [0xa5; TRANSFORM_BYTES];
    let mut params = FaceParams::new(None);
    params.color = [0.125, 0.25, 0.5, 0.75];
    params.uv = [[2.0, 3.0, 4.0, 0.0], [5.0, 6.0, 7.0, 0.0]];
    params.params = [1.0, 2.0, 0.7, 1.0];
    let bytes = packed_instance(&base, &params, Vec3::new(8.0, 9.0, 10.0));
    assert_eq!(&bytes[..112], &base, "model and normal attributes retain their offsets");
    let attributes: Vec<_> = bytes[112..].as_chunks::<4>().0.iter().map(|word| f32::from_le_bytes(*word)).collect();
    assert_eq!(
        attributes,
        [0.125, 0.25, 0.5, 0.75, 2.0, 3.0, 4.0, 0.0, 5.0, 6.0, 7.0, 0.0, 1.0, 2.0, 0.7, 1.0, 8.0, 9.0, 10.0, 0.0],
        "studio shader locations 10 through 14 consume five vec4 attributes"
    );
    assert_eq!(bytes.len(), 192, "GPU instance stride");
}

#[test]
fn resource_admission() {
    let memory = Arc::new(Accounting::default());
    let a = memory.reserve(MESH_BYTES, Resource::Mesh).unwrap();
    assert!(memory.reserve(1, Resource::Mesh).is_none());
    let b = memory.reserve(DEPTH_BYTES, Resource::Depth).unwrap();
    assert!(memory.reserve(1, Resource::Depth).is_none());
    drop(a);
    drop(b);
    assert_eq!(memory.bytes.load(Ordering::Acquire), 0);
    assert_eq!(memory.depth.load(Ordering::Acquire), 0);
}

#[test]
fn normal_transform() {
    let rotation = Quat::from_rotation_z(0.3);
    let scale = Vec3::new(-2.0, 3.0, 4.0);
    let normal = normal_matrix(rotation, scale) * Vec3::new(1.0, 1.0, 0.0);
    let tangent = rotation * (scale * Vec3::new(1.0, -1.0, 0.0));
    assert!(normal.dot(tangent).abs() < 1e-5);
    assert!((normal_matrix(Quat::IDENTITY, Vec3::splat(1e30)) * Vec3::X).is_finite());
    assert_eq!(normal_matrix(Quat::IDENTITY, scale) * Vec3::X, -Vec3::X);
}

#[test]
fn reflected_winding() {
    for mask in 0..8 {
        let scale = Vec3::from_array([0, 1, 2].map(|axis| if mask & (1 << axis) == 0 { 2.0 } else { -2.0 }));
        let cross = (scale * Vec3::X).cross(scale * Vec3::Y);
        let outward = normal_matrix(Quat::IDENTITY, scale) * Vec3::Z;
        assert_eq!(flips_winding(scale), cross.dot(outward) < 0.0, "reflection mask {mask}");
        assert_eq!(flips_winding(scale * 1e30), flips_winding(scale), "large finite scales must preserve winding");
    }
}
