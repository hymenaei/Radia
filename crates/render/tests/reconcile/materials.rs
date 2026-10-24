use assets::{TextureHandle, TextureSize};
use render::material::{AlphaBehavior, FaceMaterial, Resolution};
use test_support::texture::{GRAY_4096_J2C, RGBA_J2C};
use world::{FaceAppearance, PbrAlphaMode, PbrMaterialAssignments, PbrMaterialOverride, PbrMaterialOverrides, SurfaceAppearance};

use super::support::{
    AssetId, Budget, Camera, DVec3, FieldUpdate, Instant, NonZeroU32, ObjectPatch, ParentUpdate, Pose, Priority, ReconcileStep, Reconciler,
    Request, Uuid, Vec3, add, camera, prepared_asset, settle_materials, source, world,
};

fn color_image(face: &FaceMaterial) -> &TextureHandle {
    face.base_color_texture.as_ref().expect("expected a texture binding").image().expect("expected a usable image")
}

fn material_asset(id: AssetId) -> assets::Status {
    let json = r#"{"asset":{"version":"2.0"},
        "images":[{"uri":"00000000-0000-0000-0000-000000000064"},{"uri":"00000000-0000-0000-0000-0000000000c8"}],
        "textures":[{"source":0},{"source":1}],
        "materials":[{"pbrMetallicRoughness":{"baseColorFactor":[0.2,0.3,0.4,0.5],"baseColorTexture":{"index":0}},
            "normalTexture":{"index":1},"alphaMode":"MASK","alphaCutoff":0.4,"doubleSided":true}]}
    "#;
    let mut envelope = b"{\0\0\0\x03k\0\0\0\x07versions\0\0\0\x031.1k\0\0\0\x04types\0\0\0\x08GLTF 2.0k\0\0\0\x04datas".to_vec();
    envelope.extend((json.len() as u32).to_be_bytes());
    envelope.extend_from_slice(json.as_bytes());
    envelope.push(b'}');
    prepared_asset(Request::PbrMaterial(id), envelope)
}

#[test]
fn unseen_objects_get_baselines() {
    let mut world = world();
    for (id, x) in [(1, -10.0), (2, 10.0), (3, -20.0)] {
        add(&mut world, id, id as u32, ParentUpdate::Root, x, id as u64 * 2);
        let mut patch = ObjectPatch::new(source(1, id as u64 * 2 + 1), NonZeroU32::new(id as u32).unwrap());
        patch.surface_appearance = Some(
            SurfaceAppearance::new(
                FaceAppearance { texture: Some(AssetId::new(Uuid::from_u128(100 + id))), ..FaceAppearance::default() },
                if id == 3 {
                    vec![(0, FaceAppearance { texture: Some(AssetId::new(Uuid::from_u128(203))), ..FaceAppearance::default() })]
                } else {
                    Vec::new()
                },
            )
            .unwrap(),
        );
        let _ = world.patch_object(patch).unwrap();
    }
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(camera(0.0));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    for id in [103, 203] {
        assert!(
            work.requested
                .iter()
                .any(|request| request.priority == Priority::Background && request.request.resource().id.uuid() == Uuid::from_u128(id)),
            "off-camera defaults and face-zero overrides both need baseline coverage"
        );
    }
    let baseline = work.requested.iter().find(|request| request.request.resource().id.uuid() == Uuid::from_u128(101)).unwrap();
    assert_eq!(baseline.priority, Priority::Background);
    assert_eq!(baseline.request, Request::Texture { id: AssetId::new(Uuid::from_u128(101)), size: TextureSize::new(32) });
    let object = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(1)).unwrap();
    assert!(object.mesh().is_none(), "baseline loading does not require preparing off-camera geometry");
    scene.complete_asset(baseline.ticket, prepared_asset(baseline.request, GRAY_4096_J2C.to_vec()));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    scene.set_camera(Camera::new(DVec3::ZERO, -Vec3::X, std::f64::consts::FRAC_PI_2, [1000, 1000]).unwrap());
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let current = work
        .requested
        .iter()
        .find(|request| request.priority == Priority::Visible && request.request.resource().id.uuid() == Uuid::from_u128(101))
        .unwrap();
    assert_ne!(current.ticket, baseline.ticket);
    let object = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(1)).unwrap();
    assert!(object.mesh().is_some());
    assert!(
        object.materials().unwrap().iter().all(|face| color_image(face).info().unwrap().size == [32; 2]),
        "newly visible faces retain the regional baseline during refinement"
    );
}

#[test]
fn view_bounds_control_texture_priority() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 20.0, 1);
    let mut patch = ObjectPatch::new(source(1, 2), NonZeroU32::new(1).unwrap());
    patch.pose = Some(Pose { position: Some(Vec3::new(20.0, 30.0, 0.0)), rotation: None });
    patch.surface_appearance = Some(
        SurfaceAppearance::new(
            FaceAppearance { texture: Some(AssetId::new(Uuid::from_u128(101))), ..FaceAppearance::default() },
            Vec::new(),
        )
        .unwrap(),
    );
    let _ = world.patch_object(patch).unwrap();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(camera(0.0));
    let first = settle_materials(&mut scene, &mut view, &world, &mut now);
    let baseline = first.requested.last().unwrap();
    assert_eq!(baseline.priority, Priority::Background, "off-screen objects cannot compete with visible textures");
    assert_eq!(baseline.request, Request::Texture { id: AssetId::new(Uuid::from_u128(101)), size: TextureSize::new(32) });
    scene.set_camera(Camera::new(DVec3::ZERO, Vec3::X, std::f64::consts::FRAC_PI_2, [2000, 1000]).unwrap());
    let wide = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(wide.released.contains(&baseline.ticket));
    assert_eq!(wide.requested.last().unwrap().priority, Priority::Visible, "viewport changes refresh the actual visible demand");
    scene.set_camera(camera(0.0));
    let narrow = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(narrow.requested.last().unwrap().priority, Priority::Background, "leaving the view preserves baseline coverage");
}

#[test]
fn shared_textures_follow_camera() {
    let mut world = world();
    for (id, x) in [(1, 10.0), (2, 100.0)] {
        add(&mut world, id, id as u32, ParentUpdate::Root, x, id as u64 * 2);
        let mut patch = ObjectPatch::new(source(1, id as u64 * 2 + 1), NonZeroU32::new(id as u32).unwrap());
        patch.surface_appearance = Some(
            SurfaceAppearance::new(
                FaceAppearance { texture: Some(AssetId::new(Uuid::from_u128(100))), ..FaceAppearance::default() },
                Vec::new(),
            )
            .unwrap(),
        );
        let _ = world.patch_object(patch).unwrap();
    }
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(camera(0.0));
    let first = settle_materials(&mut scene, &mut view, &world, &mut now).requested[0];
    assert_eq!(first.request, Request::Texture { id: AssetId::new(Uuid::from_u128(100)), size: TextureSize::new(128) });
    let ready = prepared_asset(first.request, GRAY_4096_J2C.to_vec());
    scene.complete_asset(first.ticket, ready.clone());
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(view.objects().all(|obj| { obj.materials().unwrap().iter().all(|face| color_image(face).info().unwrap().size == [128; 2]) }));
    let _ = world.remove_object(source(1, 10), NonZeroU32::new(1).unwrap()).unwrap();
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.released, [first.ticket]);
    assert_eq!(work.requested.len(), 1, "removing the nearest user lowers shared demand");
    let low = work.requested[0];
    assert_eq!(low.request, Request::Texture { id: AssetId::new(Uuid::from_u128(100)), size: TextureSize::new(32) });
    scene.complete_asset(low.ticket, assets::Status::Decoding);
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(color_image(&view.objects().next().unwrap().materials().unwrap()[0]).info().unwrap().size, [128; 2]);
    scene.complete_asset(low.ticket, assets::Status::Failed { failure: assets::Failure::Deadline, retry_at: None });
    settle_materials(&mut scene, &mut view, &world, &mut now);
    let face = &view.objects().next().unwrap().materials().unwrap()[0];
    assert_eq!(
        face.base_color_texture
            .as_ref()
            .and_then(|binding| binding.image())
            .expect("failed refinement must keep the usable image")
            .info()
            .unwrap()
            .size,
        [128; 2]
    );
    assert_eq!(face.base_color_texture.as_ref().unwrap().state().resolution, Resolution::Failed(assets::Failure::Deadline));
    let version = view.version();
    scene.complete_asset(low.ticket, prepared_asset(low.request, GRAY_4096_J2C.to_vec()));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_ne!(view.version(), version, "a new discard level invalidates prepared rendering");
    assert_eq!(color_image(&view.objects().next().unwrap().materials().unwrap()[0]).info().unwrap().size, [32; 2]);
    scene.complete_asset(first.ticket, ready);
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(
        color_image(&view.objects().next().unwrap().materials().unwrap()[0]).info().unwrap().size,
        [32; 2],
        "stale quality cannot replace current imagery"
    );
    scene.set_camera(camera(99.0));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.requested.len(), 1);
    let Request::Texture { size, .. } = work.requested[0].request else { panic!("expected texture demand") };
    assert!(size.edge() >= 1024, "approaching a face requests more detail");
    let version = view.version();
    let request = work.requested[0];
    scene.complete_asset(request.ticket, prepared_asset(request.request, GRAY_4096_J2C.to_vec()));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_ne!(view.version(), version, "ready-to-ready image replacement must publish its new detail");
    assert_eq!(color_image(&view.objects().next().unwrap().materials().unwrap()[0]).info().unwrap().size, [size.edge(); 2]);
}

#[test]
fn shared_refinement_preserves_binding() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 10.0, 1);
    let id = AssetId::new(Uuid::from_u128(100));
    let mut patch = ObjectPatch::new(source(1, 2), NonZeroU32::new(1).unwrap());
    patch.surface_appearance =
        Some(SurfaceAppearance::new(FaceAppearance { texture: Some(id), ..Default::default() }, Vec::new()).unwrap());
    let _ = world.patch_object(patch).unwrap();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(camera(0.0));
    let request = settle_materials(&mut scene, &mut view, &world, &mut now).requested[0];
    let mut store = assets::Store::new(assets::Budget::default());
    let route = store.register_route().unwrap();
    let baseline = store.request(route, request.request, Priority::Visible, now).unwrap();
    let assets::LoadJob::Fetch(job) = store.next_job(now).unwrap() else {
        panic!("fresh baseline request unexpectedly used cached source")
    };
    let input = job.complete(Ok(GRAY_4096_J2C.to_vec())).expect("fixture response fits");
    let completion = store.start_decode(input).expect("decode admission").run();
    store.complete(completion, now);
    scene.complete_asset(request.ticket, baseline.status());
    settle_materials(&mut scene, &mut view, &world, &mut now);
    let binding = view.objects().next().unwrap().materials().unwrap()[0].base_color_texture.clone().unwrap();
    let image = binding.image().unwrap();
    assert_eq!(binding.state(), render::material::AssetState { asset: id, resolution: Resolution::Ready });
    assert_eq!(image.info().unwrap().size, [128; 2]);
    let version = view.version();
    let events = image.events();
    let revision = events.revision();

    let refined = store.request(route, Request::Texture { id, size: TextureSize::new(256) }, Priority::Visible, now).unwrap();
    let assets::LoadJob::Decode(input) = store.next_job(now).unwrap() else { panic!("cached refinement unexpectedly refetched the asset") };
    let completion = store.start_decode(input).expect("decode admission").run();
    store.complete(completion, now);
    assert!(matches!(refined.status(), assets::Status::Ready(_)));
    scene.complete_asset(request.ticket, baseline.status());
    settle_materials(&mut scene, &mut view, &world, &mut now);

    let current = view.objects().next().unwrap().materials().unwrap()[0].base_color_texture.as_ref().unwrap();
    assert_eq!(current, &binding, "refinement keeps the binding identity and availability");
    assert_eq!(view.version(), version, "image refinement must not fabricate a material change");
    assert_eq!(current.image().unwrap().info().unwrap().size, [256; 2]);
    assert!(events.revision() > revision, "texture notifications must invalidate GPU preparation independently");
}

#[test]
fn material_arrival_preserves_geometry() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 10.0, 1);
    let id = AssetId::new(Uuid::from_u128(9));
    let mut patch = ObjectPatch::new(source(1, 2), NonZeroU32::new(1).unwrap());
    patch.surface_appearance = Some(
        SurfaceAppearance::new(
            FaceAppearance { texture: Some(AssetId::new(Uuid::from_u128(6))), color: [255, 0, 0, 128], ..FaceAppearance::default() },
            Vec::new(),
        )
        .unwrap(),
    );
    patch.pbr_materials = Some(PbrMaterialAssignments::new(vec![(0, id), (1, id)]).unwrap());
    patch.pbr_material_overrides = Some(
        PbrMaterialOverrides::new(vec![
            (
                0,
                PbrMaterialOverride {
                    base_color: world::PbrTextureOverride { texture: FieldUpdate::Clear, ..Default::default() },
                    base_color_factor: Some([0.6, 0.7, 0.8, 1.0]),
                    alpha_mode: Some(PbrAlphaMode::Opaque),
                    double_sided: Some(false),
                    ..PbrMaterialOverride::default()
                },
            ),
            (1, PbrMaterialOverride { base_color_factor: Some([1.0; 4]), ..PbrMaterialOverride::default() }),
        ])
        .unwrap(),
    );
    let _ = world.patch_object(patch).unwrap();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(camera(0.0));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.requested.len(), 2, "two faces share one material request and one fallback texture request");
    let request = work.requested.iter().find(|request| request.request.resource().kind == assets::Kind::PbrMaterial).unwrap();
    let object = view.objects().next().unwrap();
    let mesh = object.mesh().unwrap().1.id();
    let initial = &object.materials().unwrap()[0];
    assert_eq!(initial.pbr_state.unwrap().resolution, Resolution::Pending);
    assert_eq!(initial.base_color_factor(), [1.0, 0.0, 0.0, 128.0 / 255.0]);
    assert_eq!(initial.alpha, AlphaBehavior::Blend);
    let version = view.version();
    let revision = view.revision();
    let ready = material_asset(id);
    scene.complete_asset(request.ticket, ready.clone());
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!((work.reads, work.builds), (0, 0), "material completion must not rehydrate or rebuild geometry");
    assert_ne!(view.version(), version, "material publications invalidate prepared rendering");
    assert_eq!(view.revision(), revision, "World revision alone cannot identify Scene content");
    assert_eq!(work.requested.len(), 1, "only the visible base-color image is fetched at this shading stage");
    let image_request = work.requested[0];
    assert_eq!(image_request.request.resource(), assets::Resource { id: AssetId::new(Uuid::from_u128(100)), kind: assets::Kind::Texture });
    let object = view.objects().next().unwrap();
    assert_eq!(object.mesh().unwrap().1.id(), mesh);
    let faces = object.materials().unwrap();
    assert_eq!(faces[0].base_color_factor(), [0.6, 0.7, 0.8, 1.0]);
    assert_eq!(faces[0].material.base_color_texture().source, assets::TextureRef::Clear);
    assert_eq!(faces[0].base_color_texture, None, "a cleared assignment has no pending binding");
    assert_eq!(faces[0].alpha, AlphaBehavior::Opaque);
    assert!(!faces[0].double_sided());
    assert_eq!(faces[1].base_color_factor(), [1.0; 4]);
    assert_eq!(faces[1].alpha, AlphaBehavior::Mask(0.4));
    assert!(faces[1].double_sided());
    let assets::Status::Ready(assets::Content::Material(base)) = ready else { panic!("expected material") };
    let assets::MaterialDescription::Pbr(base) = *base else { panic!("expected PBR") };
    assert_eq!(base.base_color_factor, [0.2, 0.3, 0.4, 0.5]);
    let assets::MaterialDescription::Pbr(effective) = faces[1].material else { panic!("expected PBR") };
    assert_eq!(effective.normal, base.normal, "unshaded channels remain intact");
    scene.complete_asset(image_request.ticket, prepared_asset(image_request.request, RGBA_J2C.to_vec()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!((work.reads, work.builds), (0, 0));
    let face = &view.objects().next().unwrap().materials().unwrap()[1];
    assert!(face.base_color_texture.as_ref().is_some_and(|binding| binding.image().is_some()));
    assert_eq!(face.base_color_texture.as_ref().unwrap().state().resolution, Resolution::Ready);

    let mut patch = ObjectPatch::new(source(1, 3), NonZeroU32::new(1).unwrap());
    patch.pbr_materials = Some(PbrMaterialAssignments::new(Vec::new()).unwrap());
    assert!(world.patch_object(patch).unwrap().revision.is_some(), "removing PBR bindings must change World state");
    scene.notify_world_change();
    settle_materials(&mut scene, &mut view, &world, &mut now);
    let object = view.objects().next().unwrap();
    assert_eq!(object.mesh().unwrap().1.id(), mesh, "removing PBR bindings must preserve geometry");
    let face = &object.materials().unwrap()[0];
    assert!(matches!(face.material, assets::MaterialDescription::Legacy(_)));
    assert_eq!(face.pbr_state, None, "removed bindings cannot retain PBR resolution state");
    assert_eq!(face.base_color_factor(), [1.0, 0.0, 0.0, 128.0 / 255.0], "removing PBR restores authored diffuse properties");
    assert_eq!(face.alpha, AlphaBehavior::Blend);
}

#[test]
fn binding_edits_reject_old_completion() {
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 10.0, 1);
    let mut patch = ObjectPatch::new(source(1, 2), NonZeroU32::new(1).unwrap());
    let old = AssetId::new(Uuid::from_u128(9));
    patch.pbr_materials = Some(PbrMaterialAssignments::new(vec![(0, old)]).unwrap());
    let _ = world.patch_object(patch).unwrap();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(camera(0.0));
    let first = settle_materials(&mut scene, &mut view, &world, &mut now).requested[0];
    scene.complete_asset(first.ticket, material_asset(old));
    let image = settle_materials(&mut scene, &mut view, &world, &mut now).requested[0];
    assert!(matches!(view.objects().next().unwrap().materials().unwrap()[0].material, assets::MaterialDescription::Pbr(_)));
    let current = AssetId::new(Uuid::from_u128(10));
    let mut patch = ObjectPatch::new(source(1, 3), NonZeroU32::new(1).unwrap());
    patch.pbr_materials = Some(PbrMaterialAssignments::new(vec![(0, current)]).unwrap());
    let _ = world.patch_object(patch).unwrap();
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.released.len(), 2);
    assert!(work.released.contains(&first.ticket));
    assert!(work.released.contains(&image.ticket));
    assert_eq!(work.builds, 0);
    let next = work.requested[0];
    scene.complete_asset(first.ticket, material_asset(old));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!((work.reads, work.builds, work.requested.len()), (0, 0, 0));
    let face = &view.objects().next().unwrap().materials().unwrap()[0];
    assert!(matches!(face.material, assets::MaterialDescription::Legacy(_)));
    assert_eq!(face.pbr_state.unwrap().asset, current);
    assert_eq!(face.pbr_state.unwrap().resolution, Resolution::Pending);
    scene.complete_asset(next.ticket, assets::Status::Failed { failure: assets::Failure::NotFound, retry_at: None });
    settle_materials(&mut scene, &mut view, &world, &mut now);
    let face = &view.objects().next().unwrap().materials().unwrap()[0];
    assert_eq!(face.pbr_state.unwrap().resolution, Resolution::Failed(assets::Failure::NotFound));
    assert!(matches!(face.material, assets::MaterialDescription::Legacy(_)));
    scene.close();
    scene.complete_asset(next.ticket, material_asset(current));
    assert!(matches!(scene.poll(now), ReconcileStep::Closed));
}

#[test]
fn shared_demand_releases_last_user() {
    let mut world = world();
    for id in 1..=2 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 10.0, id as u64 * 2);
        let mut patch = ObjectPatch::new(source(1, id as u64 * 2 + 1), NonZeroU32::new(id as u32).unwrap());
        patch.surface_appearance = Some(
            SurfaceAppearance::new(
                FaceAppearance { texture: Some(AssetId::new(Uuid::from_u128(100))), ..FaceAppearance::default() },
                Vec::new(),
            )
            .unwrap(),
        );
        let _ = world.patch_object(patch).unwrap();
    }
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(camera(0.0));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.requested.len(), 1);
    let ticket = work.requested[0].ticket;
    for local in 1..=2 {
        let _ = world.remove_object(source(1, 10 + u64::from(local)), NonZeroU32::new(local).unwrap()).unwrap();
        scene.notify_world_change();
        let work = settle_materials(&mut scene, &mut view, &world, &mut now);
        assert!(work.requested.is_empty());
        if local == 1 {
            assert!(work.released.is_empty());
        } else {
            assert_eq!(work.released, vec![ticket]);
        }
    }
    assert_eq!(view.object_count(), 0);
}

#[test]
fn regional_demands_have_no_cache_cap() {
    let mut world = world();
    for id in 1..=700 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 10.0, id as u64 * 2);
        let mut patch = ObjectPatch::new(source(1, id as u64 * 2 + 1), NonZeroU32::new(id as u32).unwrap());
        patch.surface_appearance = Some(
            SurfaceAppearance::new(
                FaceAppearance { texture: Some(AssetId::new(Uuid::from_u128(10_000 + id * 6))), ..FaceAppearance::default() },
                (1..6)
                    .map(|slot| {
                        (
                            slot,
                            FaceAppearance {
                                texture: Some(AssetId::new(Uuid::from_u128(10_000 + id * 6 + u128::from(slot)))),
                                ..FaceAppearance::default()
                            },
                        )
                    })
                    .collect(),
            )
            .unwrap(),
        );
        let _ = world.patch_object(patch).unwrap();
    }
    let budget = Budget::default();
    let (mut scene, mut view) = Reconciler::new(budget.clone());
    let mut now = Instant::now();
    scene.set_camera(camera(0.0));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let active: Vec<_> = work.requested.iter().filter(|request| !work.released.contains(&request.ticket)).collect();
    assert_eq!(active.len(), 4200, "all observed faces receive demand without a 4096-interest cache cutoff");
    assert!(
        view.objects().flat_map(|object| object.materials().unwrap()).all(|face| face
            .base_color_texture
            .as_ref()
            .unwrap()
            .state()
            .resolution
            == Resolution::Pending)
    );
    assert!(budget.charged_bytes() <= render::scene::MEMORY_BYTES);
    let removed = *active[0];
    let local = ((removed.request.resource().id.uuid().as_u128() - 10_000) / 6) as u32;
    let _ = world.remove_object(source(1, 10_000), NonZeroU32::new(local).unwrap()).unwrap();
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.released.len(), 6, "retiring an object releases its six unique texture interests");
    assert!(work.released.contains(&removed.ticket));
    assert!(work.requested.is_empty());
}

#[test]
fn shared_texture_outlives_scene() {
    let mut world = world();
    for id in 1..=25 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 10.0, id as u64 * 2);
        let mut patch = ObjectPatch::new(source(1, id as u64 * 2 + 1), NonZeroU32::new(id as u32).unwrap());
        patch.surface_appearance = Some(
            SurfaceAppearance::new(
                FaceAppearance { texture: Some(AssetId::new(Uuid::from_u128(101))), ..FaceAppearance::default() },
                Vec::new(),
            )
            .unwrap(),
        );
        let _ = world.patch_object(patch).unwrap();
    }
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(camera(0.0));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.requested.len(), 1, "faces share one texture demand");
    let ready = prepared_asset(Request::new(work.requested[0].request.resource()), GRAY_4096_J2C.to_vec());
    scene.complete_asset(work.requested[0].ticket, ready);
    let completed = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(completed.released.is_empty());
    let held = view.objects().next().unwrap().materials().unwrap()[0].base_color_texture.as_ref().unwrap().image().unwrap().clone();
    assert!(view.objects().all(|object| color_image(&object.materials().unwrap()[0]).id() == held.id()));
    let weak = held.downgrade();
    scene.close();
    drop(scene);
    drop(view);
    assert!(weak.is_alive(), "the explicit consumer retains ownership after scene shutdown");
    drop(held);
    assert!(!weak.is_alive(), "no scene cache retains a hidden strong handle");
}
