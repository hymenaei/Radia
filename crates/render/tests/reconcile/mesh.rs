use std::assert_matches;
use std::collections::{HashMap, HashSet};

use render::material::AlphaBehavior;
use render::scene::{DrawnMeshKey, Geometry};
use world::{FaceAppearance, SculptFlags, SurfaceAppearance};

use super::support::{
    AssetId, Budget, Camera, DVec3, FieldUpdate, Instant, NonZeroU32, ParentUpdate, Priority, Reconciler, Request, Shape, Uuid, Vec3, add,
    camera, prepared_asset, settle_materials, source, world,
};

fn add_mesh(world: &mut world::World, id: u32, asset: AssetId, x: f32) {
    add(world, u128::from(id), id, ParentUpdate::Root, x, u64::from(id) * 4);
    let mut patch = world::ObjectPatch::new(source(1, u64::from(id) * 4 + 1), NonZeroU32::new(id).unwrap());
    patch.shape = FieldUpdate::Set(Shape::Mesh { asset_id: asset, flags: SculptFlags::default() });
    let _ = world.patch_object(patch).unwrap();
}

fn high_camera() -> Camera {
    Camera::new(DVec3::ZERO, Vec3::X, std::f64::consts::FRAC_PI_3, [1000, 1000]).unwrap()
}

fn medium_camera() -> Camera {
    Camera::new(DVec3::new(-8.0, 0.0, 0.0), Vec3::X, std::f64::consts::FRAC_PI_3, [1000, 1000]).unwrap()
}

fn request_lod(work: &[render::scene::AssetRequest], lod: assets::mesh::MeshLod) -> render::scene::AssetRequest {
    work.iter()
        .find(|request| matches!(request.request, Request::MeshLod(block) if block.lod() == lod))
        .copied()
        .expect("the selected mesh LOD was requested")
}

fn request_header(work: &[render::scene::AssetRequest]) -> render::scene::AssetRequest {
    work.iter().find(|request| matches!(request.request, Request::MeshHeader(_))).copied().expect("the mesh header was requested")
}

fn asset_lod(object: &render::scene::Object) -> Option<(AssetId, assets::mesh::MeshLod)> {
    match object.mesh().map(|(key, _)| key) {
        Some(DrawnMeshKey::Asset { asset, lod }) => Some((asset, lod)),
        Some(DrawnMeshKey::Prim(_)) | None => None,
    }
}

#[test]
fn mesh_importance_survives_small_motion() {
    let far_asset = AssetId::new(Uuid::from_u128(790));
    let near_asset = AssetId::new(Uuid::from_u128(791));
    let mut world = world();
    add_mesh(&mut world, 1, far_asset, 50.0);
    add_mesh(&mut world, 2, near_asset, 10.0);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(high_camera());

    let initial = settle_materials(&mut scene, &mut view, &world, &mut now);
    let far = request_header(&initial.requested).importance;
    let near_request = initial
        .requested
        .iter()
        .find(|request| matches!(request.request, Request::MeshHeader(id) if id == near_asset))
        .copied()
        .expect("near mesh header is requested");
    assert!(near_request.importance > far);

    scene.set_camera(Camera::new(DVec3::new(0.01, 0.0, 0.0), Vec3::X, std::f64::consts::FRAC_PI_3, [1000, 1000]).unwrap());
    let small_move = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(small_move.requested.is_empty());
    assert!(small_move.released.is_empty());
    assert!(small_move.importance_updates.is_empty());

    scene.set_camera(Camera::new(DVec3::X * 3.0, Vec3::X, std::f64::consts::FRAC_PI_3, [1000, 1000]).unwrap());
    let larger_move = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(larger_move.requested.is_empty(), "an importance change retains its original request");
    assert!(larger_move.released.is_empty(), "an importance change keeps the in-flight ticket alive");
    let refreshed = larger_move
        .importance_updates
        .iter()
        .find(|(ticket, _)| *ticket == near_request.ticket)
        .map(|(_, importance)| importance)
        .expect("a changed importance bucket updates the near mesh request in place");
    assert!(*refreshed > near_request.importance);
}

#[test]
fn unpublished_mesh_slots_bound_and_release() {
    let asset = AssetId::new(Uuid::from_u128(792));
    let textures = (0..13).map(|slot| AssetId::new(Uuid::from_u128(20_000 + slot))).collect::<Vec<_>>();
    let overrides = textures
        .iter()
        .skip(1)
        .enumerate()
        .map(|(index, texture)| (u8::try_from(index + 1).unwrap(), FaceAppearance { texture: Some(*texture), ..FaceAppearance::default() }))
        .collect();
    let mut world = world();
    add_mesh(&mut world, 1, asset, 10.0);
    let mut patch = world::ObjectPatch::new(source(1, 8), NonZeroU32::new(1).unwrap());
    patch.surface_appearance =
        Some(SurfaceAppearance::new(FaceAppearance { texture: Some(textures[0]), ..FaceAppearance::default() }, overrides).unwrap());
    let _ = world.patch_object(patch).unwrap();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(high_camera());

    let initial = settle_materials(&mut scene, &mut view, &world, &mut now);
    let texture_tickets = initial
        .requested
        .iter()
        .filter_map(|request| match request.request {
            Request::Texture { id, .. } => Some((id, request.ticket)),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    assert_eq!(texture_tickets.len(), 8);
    assert_eq!(texture_tickets.keys().copied().collect::<HashSet<_>>(), textures.iter().take(8).copied().collect::<HashSet<_>>());
    let header = request_header(&initial.requested);
    assert!(view.objects().next().is_some_and(|object| object.mesh().is_none()));

    let bytes = test_support::mesh::asset_with_lod_faces(&[(2, 1)]);
    scene.complete_asset(header.ticket, prepared_asset(header.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let medium = request_lod(&work.requested, assets::mesh::MeshLod::Medium);
    scene.complete_asset(medium.ticket, prepared_asset(medium.request, bytes));
    let published = settle_materials(&mut scene, &mut view, &world, &mut now);
    let released = published.released.into_iter().collect::<HashSet<_>>();
    for (id, ticket) in texture_tickets {
        assert_eq!(released.contains(&ticket), id != textures[0], "unexpected release state for {id:?}");
    }
    assert!(view.objects().next().is_some_and(|object| object.mesh().is_some()));
}

#[test]
fn face_budget_retries_overflow_meshes_and_prims_after_shared_mesh_publication() {
    const ADMITTED_MESHES: u32 = 4096;
    let shared_asset = AssetId::new(Uuid::from_u128(50_000));
    let rejected_asset = AssetId::new(Uuid::from_u128(50_001));
    let rejected_texture = AssetId::new(Uuid::from_u128(50_002));
    let mut world = super::support::world_with_capacity(8192);
    let overrides = (1..8).map(|slot| (slot, FaceAppearance { color: [slot, 30, 60, 255], ..FaceAppearance::default() })).collect();
    let eight_faces = SurfaceAppearance::new(FaceAppearance::default(), overrides).unwrap();
    for id in 1..=ADMITTED_MESHES {
        add_mesh(&mut world, id, shared_asset, 10.0);
        let mut patch = world::ObjectPatch::new(source(1, u64::from(id) * 4 + 2), NonZeroU32::new(id).unwrap());
        patch.surface_appearance = Some(eight_faces.clone());
        let _ = world.patch_object(patch).unwrap();
    }
    let prim_id = ADMITTED_MESHES + 1;
    add(&mut world, u128::from(prim_id), prim_id, ParentUpdate::Root, 10.0, u64::from(prim_id) * 4);
    let mut prim_appearance = world::ObjectPatch::new(source(1, u64::from(prim_id) * 4 + 1), NonZeroU32::new(prim_id).unwrap());
    prim_appearance.surface_appearance =
        Some(SurfaceAppearance::new(FaceAppearance { texture: Some(rejected_texture), ..FaceAppearance::default() }, Vec::new()).unwrap());
    let _ = world.patch_object(prim_appearance).unwrap();
    let mesh_id = prim_id + 1;
    add_mesh(&mut world, mesh_id, rejected_asset, 10.0);
    let mut mesh_appearance = world::ObjectPatch::new(source(1, u64::from(mesh_id) * 4 + 2), NonZeroU32::new(mesh_id).unwrap());
    mesh_appearance.surface_appearance = Some(eight_faces);
    let _ = world.patch_object(mesh_appearance).unwrap();

    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(high_camera());
    let initial = settle_materials(&mut scene, &mut view, &world, &mut now);
    let world_revision = world.revision();
    assert_eq!(view.object_count(), usize::try_from(mesh_id).unwrap());
    assert!(initial.requested.iter().any(|request| matches!(request.request, Request::MeshHeader(id) if id == shared_asset)));
    let rejected_header = initial
        .requested
        .iter()
        .find(|request| matches!(request.request, Request::MeshHeader(id) if id == rejected_asset))
        .expect("the rejected mesh starts provisional header demand before the face check");
    assert!(initial.released.contains(&rejected_header.ticket));
    assert!(!initial.requested.iter().any(|request| matches!(request.request, Request::Texture { id, .. } if id == rejected_texture)));

    let shared_bytes = test_support::mesh::asset_with_lod_faces(&[(2, 1)]);
    let header = request_header(&initial.requested);
    scene.complete_asset(header.ticket, prepared_asset(header.request, shared_bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let medium = request_lod(&work.requested, assets::mesh::MeshLod::Medium);
    scene.complete_asset(medium.ticket, prepared_asset(medium.request, shared_bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let requeued_texture = work
        .requested
        .iter()
        .find(|request| matches!(request.request, Request::Texture { id, .. } if id == rejected_texture))
        .unwrap_or_else(|| {
            panic!(
                "the rejected prim receives its material demand after mesh slots shrink; requests: {:?}",
                work.requested.iter().map(|request| request.request).collect::<Vec<_>>()
            )
        });
    let requeued_header = work
        .requested
        .iter()
        .find(|request| matches!(request.request, Request::MeshHeader(id) if id == rejected_asset))
        .expect("the rejected mesh receives its header demand after mesh slots shrink");
    assert!(!work.released.contains(&requeued_texture.ticket));
    assert!(!work.released.contains(&requeued_header.ticket));

    scene.complete_asset(requeued_texture.ticket, prepared_asset(requeued_texture.request, test_support::texture::GRAY_4096_J2C.to_vec()));
    scene.complete_asset(requeued_header.ticket, prepared_asset(requeued_header.request, shared_bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let rejected_lod = request_lod(&work.requested, assets::mesh::MeshLod::Medium);
    assert_eq!(rejected_lod.request.resource().id, rejected_asset);
    scene.complete_asset(rejected_lod.ticket, prepared_asset(rejected_lod.request, shared_bytes));
    let _ = settle_materials(&mut scene, &mut view, &world, &mut now);

    assert_eq!(world.revision(), world_revision);
    let prim = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(u128::from(prim_id))).expect("rejected prim");
    assert!(prim.materials().is_some());
    assert!(prim.materials().unwrap().iter().any(|material| material.base_color_texture.is_some()));
    let mesh = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(u128::from(mesh_id))).expect("rejected mesh");
    assert!(mesh.materials().is_some());
    assert_eq!(asset_lod(mesh), Some((rejected_asset, assets::mesh::MeshLod::Medium)));
    assert_eq!(view.object_count(), usize::try_from(mesh_id).unwrap());

    for id in 1..=ADMITTED_MESHES {
        let order = 20_000 + u64::from(id);
        let _ = world.remove_object(source(1, order), NonZeroU32::new(id).unwrap()).unwrap();
    }
    scene.notify_world_change();
    let _ = settle_materials(&mut scene, &mut view, &world, &mut now);
    let reacquired_id = mesh_id + 1;
    add_mesh(&mut world, reacquired_id, shared_asset, 10.0);
    scene.notify_world_change();
    let reacquired = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(reacquired.requested.iter().any(|request| matches!(request.request, Request::MeshHeader(id) if id == shared_asset)));
}

#[test]
fn mesh_lod_falls_back_then_refines() {
    let asset = AssetId::new(Uuid::from_u128(700));
    let bytes = test_support::mesh::asset(&[0, 2, 3], 1);
    let mut world = world();
    add_mesh(&mut world, 1, asset, 2.0);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(high_camera());

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let header = request_header(&work.requested);
    assert_eq!(header.priority, Priority::Visible);
    scene.complete_asset(header.ticket, prepared_asset(header.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let high = request_lod(&work.requested, assets::mesh::MeshLod::High);
    let lowest = request_lod(&work.requested, assets::mesh::MeshLod::Lowest);
    assert_eq!(work.requested.len(), 2, "the selected and smallest fallback LOD are requested");

    scene.complete_asset(lowest.ticket, prepared_asset(lowest.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.released.contains(&lowest.ticket), "decoded LOD content no longer needs its fetch ticket");
    let object = view.objects().next().unwrap();
    assert_eq!(asset_lod(object), Some((asset, assets::mesh::MeshLod::Lowest)));

    scene.complete_asset(high.ticket, prepared_asset(high.request, bytes));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.released.contains(&high.ticket), "decoded selected LOD content no longer needs its fetch ticket");
    let object = view.objects().next().unwrap();
    assert_eq!(asset_lod(object), Some((asset, assets::mesh::MeshLod::High)));
}

#[test]
fn fallback_skips_missing_lowest() {
    let asset = AssetId::new(Uuid::from_u128(719));
    let bytes = test_support::mesh::asset(&[1, 3], 1);
    let mut world = world();
    add_mesh(&mut world, 1, asset, 2.0);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(high_camera());

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let header = request_header(&work.requested);
    scene.complete_asset(header.ticket, prepared_asset(header.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let high = request_lod(&work.requested, assets::mesh::MeshLod::High);
    let low = request_lod(&work.requested, assets::mesh::MeshLod::Low);
    assert_eq!(work.requested.len(), 2);

    scene.complete_asset(low.ticket, prepared_asset(low.request, bytes.clone()));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(asset_lod(view.objects().next().unwrap()), Some((asset, assets::mesh::MeshLod::Low)));
    scene.complete_asset(high.ticket, prepared_asset(high.request, bytes));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(asset_lod(view.objects().next().unwrap()), Some((asset, assets::mesh::MeshLod::High)));
}

#[test]
fn zero_face_lod_uses_loaded_fallback() {
    let asset = AssetId::new(Uuid::from_u128(710));
    let bytes = test_support::mesh::asset_with_empty_lod(&[2], &[3], 1);
    let mut world = world();
    add_mesh(&mut world, 1, asset, 2.0);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(high_camera());

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let header = request_header(&work.requested);
    scene.complete_asset(header.ticket, prepared_asset(header.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let high = request_lod(&work.requested, assets::mesh::MeshLod::High);
    let medium = request_lod(&work.requested, assets::mesh::MeshLod::Medium);

    scene.complete_asset(medium.ticket, prepared_asset(medium.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.released.contains(&medium.ticket));
    assert_eq!(asset_lod(view.objects().next().unwrap()), Some((asset, assets::mesh::MeshLod::Medium)));

    scene.complete_asset(high.ticket, prepared_asset(high.request, bytes));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    let object = view.objects().next().unwrap();
    assert_eq!(asset_lod(object), Some((asset, assets::mesh::MeshLod::Medium)));
}

#[test]
fn material_slots_follow_the_published_fallback_lod() {
    let asset = AssetId::new(Uuid::from_u128(712));
    let texture = AssetId::new(Uuid::from_u128(713));
    let bytes = test_support::mesh::asset_with_lod_faces(&[(2, 2), (3, 1)]);
    let mut world = world();
    add_mesh(&mut world, 1, asset, 2.0);
    let mut patch = world::ObjectPatch::new(source(1, 8), NonZeroU32::new(1).unwrap());
    patch.surface_appearance = Some(
        SurfaceAppearance::new(
            FaceAppearance::default(),
            vec![(1, FaceAppearance { texture: Some(texture), color: [32, 160, 48, 0], ..FaceAppearance::default() })],
        )
        .unwrap(),
    );
    let _ = world.patch_object(patch).unwrap();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(high_camera());

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let header = request_header(&work.requested);
    scene.complete_asset(header.ticket, prepared_asset(header.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let high = request_lod(&work.requested, assets::mesh::MeshLod::High);
    let medium = request_lod(&work.requested, assets::mesh::MeshLod::Medium);

    scene.complete_asset(medium.ticket, prepared_asset(medium.request, bytes.clone()));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    let fallback = view.objects().next().unwrap();
    assert_eq!(asset_lod(fallback), Some((asset, assets::mesh::MeshLod::Medium)));
    let face = fallback.materials().unwrap().iter().find(|face| face.slot == 1).expect("fallback face appearance resolves");
    assert_eq!(face.alpha, AlphaBehavior::Blend);
    assert_eq!(face.base_color_factor()[3], 0.0);
    assert_eq!(face.base_color_texture.as_ref().unwrap().state().asset, texture);

    scene.complete_asset(high.ticket, prepared_asset(high.request, bytes));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    let object = view.objects().next().unwrap();
    assert_eq!(asset_lod(object), Some((asset, assets::mesh::MeshLod::High)));
    assert!(object.materials().is_some_and(|materials| materials.iter().all(|face| face.slot != 1)));
}

#[test]
fn current_ready_lod_stays_while_selection_loads() {
    let asset = AssetId::new(Uuid::from_u128(711));
    let bytes = test_support::mesh::asset(&[0, 1, 2, 3], 1);
    let mut world = world();
    add_mesh(&mut world, 1, asset, 2.0);
    add_mesh(&mut world, 2, asset, 50.0);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(high_camera());

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let header = request_header(&work.requested);
    scene.complete_asset(header.ticket, prepared_asset(header.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let high = request_lod(&work.requested, assets::mesh::MeshLod::High);
    let lowest = request_lod(&work.requested, assets::mesh::MeshLod::Lowest);

    scene.complete_asset(lowest.ticket, prepared_asset(lowest.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(!work.released.contains(&high.ticket));
    let high_object = view.objects().find(|object| object.detail().summary.id.as_u128() == 1).unwrap();
    assert_eq!(asset_lod(high_object), Some((asset, assets::mesh::MeshLod::Lowest)));

    scene.set_camera(medium_camera());
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let medium = request_lod(&work.requested, assets::mesh::MeshLod::Medium);
    let high_object = view.objects().find(|object| object.detail().summary.id.as_u128() == 1).unwrap();
    assert_eq!(asset_lod(high_object), Some((asset, assets::mesh::MeshLod::Lowest)));

    scene.complete_asset(medium.ticket, prepared_asset(medium.request, bytes));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    let high_object = view.objects().find(|object| object.detail().summary.id.as_u128() == 1).unwrap();
    assert_eq!(asset_lod(high_object), Some((asset, assets::mesh::MeshLod::Medium)));
}

#[test]
fn missing_medium_selects_lower_available_lod() {
    let asset = AssetId::new(Uuid::from_u128(716));
    let bytes = test_support::mesh::asset(&[0, 3], 1);
    let mut world = world();
    add_mesh(&mut world, 1, asset, 10.0);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(high_camera());

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let header = request_header(&work.requested);
    scene.complete_asset(header.ticket, prepared_asset(header.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.requested.len(), 1);
    let lowest = request_lod(&work.requested, assets::mesh::MeshLod::Lowest);

    scene.complete_asset(lowest.ticket, prepared_asset(lowest.request, bytes));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(asset_lod(view.objects().next().unwrap()), Some((asset, assets::mesh::MeshLod::Lowest)));
}

#[test]
fn current_high_lod_stays_until_medium_is_ready() {
    let asset = AssetId::new(Uuid::from_u128(717));
    let bytes = test_support::mesh::asset(&[0, 1, 2, 3], 1);
    let mut world = world();
    add_mesh(&mut world, 1, asset, 2.0);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(high_camera());

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let header = request_header(&work.requested);
    scene.complete_asset(header.ticket, prepared_asset(header.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let high = request_lod(&work.requested, assets::mesh::MeshLod::High);
    scene.complete_asset(high.ticket, prepared_asset(high.request, bytes.clone()));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(asset_lod(view.objects().next().unwrap()), Some((asset, assets::mesh::MeshLod::High)));

    scene.set_camera(medium_camera());
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let medium = request_lod(&work.requested, assets::mesh::MeshLod::Medium);
    assert_eq!(asset_lod(view.objects().next().unwrap()), Some((asset, assets::mesh::MeshLod::High)));

    scene.complete_asset(medium.ticket, prepared_asset(medium.request, bytes));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(asset_lod(view.objects().next().unwrap()), Some((asset, assets::mesh::MeshLod::Medium)));
}

#[test]
fn camera_sweep_releases_unneeded_lods() {
    let asset = AssetId::new(Uuid::from_u128(718));
    let bytes = test_support::mesh::asset(&[0, 1, 2, 3], 1);
    let mut world = world();
    add_mesh(&mut world, 1, asset, 2.0);
    add_mesh(&mut world, 2, asset, 10.0);
    let budget = Budget::default();
    let (mut scene, mut view) = Reconciler::new(budget.clone());
    let mut now = Instant::now();
    scene.set_camera(high_camera());

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let header = request_header(&work.requested);
    scene.complete_asset(header.ticket, prepared_asset(header.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let medium = request_lod(&work.requested, assets::mesh::MeshLod::Medium);
    let high = request_lod(&work.requested, assets::mesh::MeshLod::High);
    scene.complete_asset(medium.ticket, prepared_asset(medium.request, bytes.clone()));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    scene.complete_asset(high.ticket, prepared_asset(high.request, bytes.clone()));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    let high_and_medium = budget.charged_bytes();
    assert!(high_and_medium > 0);
    assert_eq!(
        asset_lod(view.objects().find(|object| object.detail().summary.id.as_u128() == 1).unwrap()),
        Some((asset, assets::mesh::MeshLod::High))
    );
    assert_eq!(
        asset_lod(view.objects().find(|object| object.detail().summary.id.as_u128() == 2).unwrap()),
        Some((asset, assets::mesh::MeshLod::Medium))
    );

    scene.set_camera(Camera::new(DVec3::new(-48.0, 0.0, 0.0), Vec3::X, std::f64::consts::FRAC_PI_3, [1000, 1000]).unwrap());
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let lowest = request_lod(&work.requested, assets::mesh::MeshLod::Lowest);
    assert_eq!(
        asset_lod(view.objects().find(|object| object.detail().summary.id.as_u128() == 1).unwrap()),
        Some((asset, assets::mesh::MeshLod::High)),
        "the current mesh remains published while the new selection loads"
    );
    assert_eq!(budget.charged_bytes(), high_and_medium, "published LOD content remains retained during the transition");

    scene.complete_asset(lowest.ticket, prepared_asset(lowest.request, bytes));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(budget.charged_bytes() < high_and_medium, "the old LOD contents are released after both objects publish Lowest");
    assert!(budget.charged_bytes() > 0, "the published LOD remains retained while it is drawn");
    assert!(view.objects().all(|object| asset_lod(object) == Some((asset, assets::mesh::MeshLod::Lowest))));
}

#[test]
fn camera_change_refines_distinct_meshes() {
    const OBJECTS: u32 = 64;

    let assets: Vec<_> = (0..OBJECTS).map(|index| AssetId::new(Uuid::from_u128(20_000 + u128::from(index)))).collect();
    let bytes = test_support::mesh::asset(&[2, 3], 1);
    let mut world = world();
    for (index, asset) in assets.iter().copied().enumerate() {
        add_mesh(&mut world, index as u32 + 1, asset, 2.0);
    }
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(high_camera());

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let headers: Vec<_> = work.requested.iter().filter(|request| matches!(request.request, Request::MeshHeader(_))).copied().collect();
    assert_eq!(headers.len(), assets.len());
    for header in headers {
        scene.complete_asset(header.ticket, prepared_asset(header.request, bytes.clone()));
    }

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let highs: Vec<_> = work
        .requested
        .iter()
        .filter(|request| matches!(request.request, Request::MeshLod(block) if block.lod() == assets::mesh::MeshLod::High))
        .copied()
        .collect();
    let mediums: Vec<_> = work
        .requested
        .iter()
        .filter(|request| matches!(request.request, Request::MeshLod(block) if block.lod() == assets::mesh::MeshLod::Medium))
        .copied()
        .collect();
    assert_eq!(highs.len(), assets.len());
    assert_eq!(mediums.len(), assets.len());
    let high_tickets: HashSet<_> = highs.iter().map(|request| request.ticket).collect();
    let first_medium_tickets: HashSet<_> = mediums.iter().map(|request| request.ticket).collect();
    for high in highs {
        scene.complete_asset(high.ticket, prepared_asset(high.request, bytes.clone()));
    }

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let released_first_lods: HashSet<_> = work.released.iter().copied().collect();
    let mut expected_releases = high_tickets;
    expected_releases.extend(first_medium_tickets);
    assert_eq!(released_first_lods, expected_releases, "decoded LODs release their fetch tickets");
    let high_meshes: HashSet<_> = view.objects().filter_map(asset_lod).collect();
    let expected_highs: HashSet<_> = assets.iter().copied().map(|asset| (asset, assets::mesh::MeshLod::High)).collect();
    assert_eq!(high_meshes, expected_highs);

    scene.set_camera(Camera::new(DVec3::new(-8.0, 0.0, 0.0), Vec3::X, std::f64::consts::FRAC_PI_3, [1000, 1000]).unwrap());
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let requested_mediums: Vec<_> = work
        .requested
        .iter()
        .filter(|request| matches!(request.request, Request::MeshLod(block) if block.lod() == assets::mesh::MeshLod::Medium))
        .copied()
        .collect();
    assert_eq!(requested_mediums.len(), assets.len(), "camera movement requests the newly selected LOD for each asset");
    assert!(requested_mediums.iter().all(|request| !released_first_lods.contains(&request.ticket)));
    assert_eq!(
        view.objects().filter_map(asset_lod).collect::<HashSet<_>>(),
        expected_highs,
        "the prior meshes stay published while medium LODs are pending"
    );
    let medium_tickets: HashSet<_> = requested_mediums.iter().map(|request| request.ticket).collect();
    for medium in requested_mediums {
        scene.complete_asset(medium.ticket, prepared_asset(medium.request, bytes.clone()));
    }

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.released.into_iter().collect::<HashSet<_>>(), medium_tickets, "decoded refinement requests are released");
    let medium_meshes: HashSet<_> = view.objects().filter_map(asset_lod).collect();
    let expected_mediums: HashSet<_> = assets.iter().copied().map(|asset| (asset, assets::mesh::MeshLod::Medium)).collect();
    assert_eq!(medium_meshes, expected_mediums);
}

#[test]
fn same_asset_shares_geometry_and_releases_on_last_user() {
    let asset = AssetId::new(Uuid::from_u128(701));
    let bytes = test_support::mesh::asset(&[0, 2, 3], 1);
    let mut world = world();
    add_mesh(&mut world, 1, asset, 2.0);
    add_mesh(&mut world, 2, asset, 2.0);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(high_camera());

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.requested.len(), 1, "objects share one header request");
    let header = work.requested[0];
    scene.complete_asset(header.ticket, prepared_asset(header.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.released.contains(&header.ticket));
    assert_eq!(work.requested.len(), 2, "objects share each selected and fallback LOD request");
    let high = request_lod(&work.requested, assets::mesh::MeshLod::High);
    let lowest = request_lod(&work.requested, assets::mesh::MeshLod::Lowest);
    scene.complete_asset(lowest.ticket, prepared_asset(lowest.request, bytes));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.released.contains(&lowest.ticket));
    let mesh_ids: Vec<_> = view.objects().filter_map(|object| object.mesh().map(|(_, mesh)| mesh.id())).collect();
    let first = mesh_ids.first().copied().unwrap();
    assert!(mesh_ids.iter().all(|id| *id == first), "both objects share one published mesh handle");

    let _ = world.remove_object(source(1, 10), NonZeroU32::new(1).unwrap()).unwrap();
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.released.is_empty(), "one remaining object keeps the shared demand alive");

    let _ = world.remove_object(source(1, 11), NonZeroU32::new(2).unwrap()).unwrap();
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.released.contains(&high.ticket));
    assert_eq!(view.object_count(), 0);
}

#[test]
fn shape_and_asset_changes_release_stale_fetches() {
    let first_asset = AssetId::new(Uuid::from_u128(702));
    let second_asset = AssetId::new(Uuid::from_u128(703));
    let bytes = test_support::mesh::asset(&[3], 1);
    let mut world = world();
    add_mesh(&mut world, 1, first_asset, 10.0);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(camera(0.0));

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let old_header = request_header(&work.requested);
    let mut patch = world::ObjectPatch::new(source(1, 8), NonZeroU32::new(1).unwrap());
    patch.shape = FieldUpdate::Set(Shape::Mesh { asset_id: second_asset, flags: SculptFlags::default() });
    let _ = world.patch_object(patch).unwrap();
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let new_header = request_header(&work.requested);
    assert!(work.released.contains(&old_header.ticket));
    assert_ne!(old_header.ticket, new_header.ticket);
    assert!(view.objects().next().unwrap().mesh().is_none());

    scene.complete_asset(old_header.ticket, prepared_asset(old_header.request, bytes.clone()));
    assert!(settle_materials(&mut scene, &mut view, &world, &mut now).requested.is_empty());
    scene.complete_asset(new_header.ticket, prepared_asset(new_header.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.released.contains(&new_header.ticket));
    let lod = request_lod(&work.requested, assets::mesh::MeshLod::High);
    scene.complete_asset(lod.ticket, prepared_asset(lod.request, bytes));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.released.contains(&lod.ticket));
    let object = view.objects().next().unwrap();
    assert_eq!(asset_lod(object).map(|(id, _)| id), Some(second_asset));

    let mut patch = world::ObjectPatch::new(source(1, 12), NonZeroU32::new(1).unwrap());
    patch.shape = FieldUpdate::Set(Shape::Sculpt {
        asset_id: AssetId::new(Uuid::from_u128(704)),
        sculpt_type: world::SculptType::Sphere,
        flags: SculptFlags::default(),
    });
    let _ = world.patch_object(patch).unwrap();
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.requested.is_empty());
    let object = view.objects().next().unwrap();
    assert_matches!(object.geometry(), Geometry::Unsupported(Shape::Sculpt { .. }));
    assert!(object.mesh().is_none());
}

#[test]
fn replaced_prim_mesh_is_not_published_for_a_mesh_asset_shape() {
    let asset = AssetId::new(Uuid::from_u128(709));
    let bytes = test_support::mesh::asset(&[3], 1);
    let mut world = world();
    add(&mut world, 1, 1, ParentUpdate::Root, 2.0, 4);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(high_camera());

    settle_materials(&mut scene, &mut view, &world, &mut now);
    let object = view.objects().next().unwrap();
    assert_matches!(object.geometry(), Geometry::Prim(_));
    let (DrawnMeshKey::Prim(_), prim_mesh) = object.mesh().unwrap() else { panic!("fixture prim mesh") };
    let prim_mesh_id = prim_mesh.id();

    let mut patch = world::ObjectPatch::new(source(1, 8), NonZeroU32::new(1).unwrap());
    patch.shape = FieldUpdate::Set(Shape::Mesh { asset_id: asset, flags: SculptFlags::default() });
    let _ = world.patch_object(patch).unwrap();
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let header = request_header(&work.requested);
    let object = view.objects().next().unwrap();
    assert_matches!(object.geometry(), Geometry::Mesh(id) if id == asset);
    assert!(object.mesh().is_none(), "the previous primitive slot is dropped when geometry changes");

    scene.complete_asset(header.ticket, prepared_asset(header.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let high = request_lod(&work.requested, assets::mesh::MeshLod::High);
    scene.complete_asset(high.ticket, prepared_asset(high.request, bytes));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    let object = view.objects().next().unwrap();
    assert_eq!(asset_lod(object), Some((asset, assets::mesh::MeshLod::High)));
    assert_ne!(object.mesh().unwrap().1.id(), prim_mesh_id);
}

#[test]
fn removing_mesh_during_header_fetch_ignores_late_completion() {
    let asset = AssetId::new(Uuid::from_u128(705));
    let mut world = world();
    add_mesh(&mut world, 1, asset, 10.0);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(camera(0.0));

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let header = request_header(&work.requested);
    let _ = world.remove_object(source(1, 8), NonZeroU32::new(1).unwrap()).unwrap();
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.released.contains(&header.ticket));
    scene.complete_asset(header.ticket, prepared_asset(header.request, test_support::mesh::asset(&[0, 1, 2, 3], 1)));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.requested.is_empty());
    assert_eq!(view.object_count(), 0);
}

#[test]
fn leaving_region_releases_mesh_fetch_and_ignores_late_completion() {
    let asset = AssetId::new(Uuid::from_u128(715));
    let mut world = world();
    add_mesh(&mut world, 1, asset, 10.0);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(camera(0.0));

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let header = request_header(&work.requested);
    let _ = world.retire_region(world::Region::new(1)).unwrap();
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.released.contains(&header.ticket));
    assert_eq!(view.object_count(), 0);

    scene.complete_asset(header.ticket, prepared_asset(header.request, test_support::mesh::asset(&[0, 1, 2, 3], 1)));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.requested.is_empty());
    assert_eq!(view.object_count(), 0);
}

#[test]
fn malformed_headers_and_oversized_face_lists_are_not_drawn() {
    let malformed = AssetId::new(Uuid::from_u128(707));
    let oversized = AssetId::new(Uuid::from_u128(708));
    let oversized_bytes = test_support::mesh::asset(&[3], 9);
    let mut world = world();
    add_mesh(&mut world, 1, malformed, 2.0);
    add_mesh(&mut world, 2, oversized, 2.0);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.set_camera(high_camera());

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let header_requests: Vec<_> =
        work.requested.iter().filter(|request| matches!(request.request, Request::MeshHeader(_))).copied().collect();
    assert_eq!(header_requests.len(), 2);
    for request in header_requests {
        let content = if request.request.resource().id == malformed {
            prepared_asset(request.request, vec![0xff])
        } else {
            prepared_asset(request.request, oversized_bytes.clone())
        };
        scene.complete_asset(request.ticket, content);
    }
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.requested.len(), 1);
    let lod = request_lod(&work.requested, assets::mesh::MeshLod::High);
    scene.complete_asset(lod.ticket, prepared_asset(lod.request, oversized_bytes));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(view.objects().all(|object| object.mesh().is_none()));
}

#[test]
fn thousands_of_mesh_objects_share_bounded_asset_and_scene_state() {
    let asset = AssetId::new(Uuid::from_u128(706));
    let bytes = test_support::mesh::asset(&[0, 2, 3], 1);
    let mut world = world();
    for id in 1..=2000 {
        add_mesh(&mut world, id, asset, 2.0);
    }
    let budget = Budget::default();
    let (mut scene, mut view) = Reconciler::new(budget.clone());
    let mut now = Instant::now();
    scene.set_camera(high_camera());

    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.object_count(), 2000);
    assert_eq!(work.requested.len(), 1, "shared asset demand progresses through the large scene");
    let header = work.requested[0];
    scene.complete_asset(header.ticket, prepared_asset(header.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(work.requested.len(), 2);
    let lowest = request_lod(&work.requested, assets::mesh::MeshLod::Lowest);
    scene.complete_asset(lowest.ticket, prepared_asset(lowest.request, bytes));
    settle_materials(&mut scene, &mut view, &world, &mut now);

    let mesh_ids: HashSet<_> = view.objects().filter_map(|object| object.mesh().map(|(_, mesh)| mesh.id())).collect();
    assert_eq!(mesh_ids.len(), 1, "thousands of objects reuse one decoded LOD");
    assert!(view.objects().all(|object| asset_lod(object) == Some((asset, assets::mesh::MeshLod::Lowest))));
    assert!(budget.charged_bytes() <= render::scene::MEMORY_BYTES);

    scene.set_camera(Camera::new(DVec3::new(0.01, 0.0, 0.0), Vec3::X, std::f64::consts::FRAC_PI_3, [1000, 1000]).unwrap());
    assert!(settle_materials(&mut scene, &mut view, &world, &mut now).requested.is_empty());
    for id in 1..=2000 {
        let _ = world.remove_object(source(1, u64::from(id) * 4 + 2), NonZeroU32::new(id).unwrap()).unwrap();
    }
    scene.notify_world_change();
    let teardown = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(teardown.released.contains(&request_lod(&work.requested, assets::mesh::MeshLod::High).ticket));
    assert_eq!(view.object_count(), 0);
}
