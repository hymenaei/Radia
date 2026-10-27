use std::assert_matches;
use std::collections::HashSet;

use render::scene::Geometry;
use world::SculptFlags;

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

fn request_lod(work: &[render::scene::AssetRequest], lod: assets::mesh::MeshLod) -> render::scene::AssetRequest {
    work.iter()
        .find(|request| matches!(request.request, Request::MeshLod(block) if block.lod() == lod))
        .copied()
        .expect("the selected mesh LOD was requested")
}

fn request_header(work: &[render::scene::AssetRequest]) -> render::scene::AssetRequest {
    work.iter().find(|request| matches!(request.request, Request::MeshHeader(_))).copied().expect("the mesh header was requested")
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
    let medium = request_lod(&work.requested, assets::mesh::MeshLod::Medium);
    assert_eq!(work.requested.len(), 2, "only the desired and nearest fallback LOD are requested");

    scene.complete_asset(medium.ticket, prepared_asset(medium.request, bytes.clone()));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    let object = view.objects().next().unwrap();
    assert_eq!(object.mesh_lod(), Some((asset, assets::mesh::MeshLod::Medium)));

    scene.complete_asset(high.ticket, prepared_asset(high.request, bytes));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.released.contains(&medium.ticket), "a fallback LOD is released once the selected LOD is ready");
    let object = view.objects().next().unwrap();
    assert_eq!(object.mesh_lod(), Some((asset, assets::mesh::MeshLod::High)));
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
    assert_eq!(work.requested.len(), 2, "objects share each selected and fallback LOD request");
    let high = request_lod(&work.requested, assets::mesh::MeshLod::High);
    let medium = request_lod(&work.requested, assets::mesh::MeshLod::Medium);
    scene.complete_asset(medium.ticket, prepared_asset(medium.request, bytes));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    let mesh_ids: Vec<_> = view.objects().filter_map(|object| object.render_mesh().map(|mesh| mesh.id())).collect();
    let first = mesh_ids.first().copied().unwrap();
    assert!(mesh_ids.iter().all(|id| *id == first), "both objects share one published mesh handle");

    let _ = world.remove_object(source(1, 10), NonZeroU32::new(1).unwrap()).unwrap();
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.released.is_empty(), "one remaining object keeps the shared demand alive");

    let _ = world.remove_object(source(1, 11), NonZeroU32::new(2).unwrap()).unwrap();
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.released.contains(&header.ticket));
    assert!(work.released.contains(&high.ticket));
    assert!(work.released.contains(&medium.ticket));
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
    assert!(view.objects().next().unwrap().render_mesh().is_none());

    scene.complete_asset(old_header.ticket, prepared_asset(old_header.request, bytes.clone()));
    assert!(settle_materials(&mut scene, &mut view, &world, &mut now).requested.is_empty());
    scene.complete_asset(new_header.ticket, prepared_asset(new_header.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let lod = request_lod(&work.requested, assets::mesh::MeshLod::High);
    scene.complete_asset(lod.ticket, prepared_asset(lod.request, bytes));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    let object = view.objects().next().unwrap();
    assert_eq!(object.mesh_lod().map(|(id, _)| id), Some(second_asset));

    let mut patch = world::ObjectPatch::new(source(1, 12), NonZeroU32::new(1).unwrap());
    patch.shape = FieldUpdate::Set(Shape::Sculpt {
        asset_id: AssetId::new(Uuid::from_u128(704)),
        sculpt_type: world::SculptType::Sphere,
        flags: SculptFlags::default(),
    });
    let _ = world.patch_object(patch).unwrap();
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.released.contains(&new_header.ticket));
    assert!(work.released.contains(&lod.ticket));
    let object = view.objects().next().unwrap();
    assert_matches!(object.geometry(), Geometry::Unsupported(Shape::Sculpt { .. }));
    assert!(object.render_mesh().is_none());
}

#[test]
fn prim_to_mesh_drops_prim_geometry_until_mesh_lod_arrives() {
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
    let prim_mesh_id = object.render_mesh().unwrap().id();

    let mut patch = world::ObjectPatch::new(source(1, 8), NonZeroU32::new(1).unwrap());
    patch.shape = FieldUpdate::Set(Shape::Mesh { asset_id: asset, flags: SculptFlags::default() });
    let _ = world.patch_object(patch).unwrap();
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let header = request_header(&work.requested);
    let object = view.objects().next().unwrap();
    assert_matches!(object.geometry(), Geometry::Mesh(id) if id == asset);
    assert!(object.render_mesh().is_none());

    scene.complete_asset(header.ticket, prepared_asset(header.request, bytes.clone()));
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    let high = request_lod(&work.requested, assets::mesh::MeshLod::High);
    scene.complete_asset(high.ticket, prepared_asset(high.request, bytes));
    settle_materials(&mut scene, &mut view, &world, &mut now);
    let object = view.objects().next().unwrap();
    assert_eq!(object.mesh_lod(), Some((asset, assets::mesh::MeshLod::High)));
    assert_ne!(object.render_mesh().unwrap().id(), prim_mesh_id);
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
    assert!(view.objects().all(|object| object.render_mesh().is_none()));
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
    let medium = request_lod(&work.requested, assets::mesh::MeshLod::Medium);
    scene.complete_asset(medium.ticket, prepared_asset(medium.request, bytes));
    settle_materials(&mut scene, &mut view, &world, &mut now);

    let mesh_ids: HashSet<_> = view.objects().filter_map(|object| object.render_mesh().map(|mesh| mesh.id())).collect();
    assert_eq!(mesh_ids.len(), 1, "thousands of objects reuse one decoded LOD");
    assert!(view.objects().all(|object| object.mesh_lod() == Some((asset, assets::mesh::MeshLod::Medium))));
    assert!(budget.charged_bytes() <= render::scene::MEMORY_BYTES);
}
