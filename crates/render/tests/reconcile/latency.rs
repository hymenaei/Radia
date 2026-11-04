use std::collections::HashSet;

use assets::mesh::MeshLod;
use render::scene::{Batch, DrawnMeshKey, Geometry, PreparedMesh};
use world::{PrimShape, SculptFlags, Shape};

use super::support::{
    AssetChange, AssetId, Budget, Detail, Duration, FieldUpdate, Instant, NonZeroU32, ObjectPatch, ParentUpdate, ReconcileStep, Reconciler,
    Request, Uuid, World, add_shaped, camera, prepared_asset, reply, source, world_with_capacity,
};

const OBJECTS: u32 = 1001;
const FRAME: Duration = Duration::from_nanos(16_666_667);
const MAX_POLLS: usize = 20_000;

struct Latency {
    builds_during_requests: usize,
    first_high_frame: usize,
    acknowledgements_at_first_high: usize,
    prim_high_targets_at_first_request: usize,
    prim_high_at_first_request: usize,
    completion_frames: usize,
    high_requests: usize,
    high_water_bytes: usize,
}

fn add_mesh(world: &mut World, id: u32, asset: AssetId) {
    let order = u64::from(id) * 4;
    add_shaped(world, u128::from(id), id, ParentUpdate::Root, 100.0, order, PrimShape::default());
    let mut patch = ObjectPatch::new(source(1, order + 1), NonZeroU32::new(id).expect("fixture local ID is nonzero"));
    patch.shape = FieldUpdate::Set(Shape::Mesh { asset_id: asset, flags: SculptFlags::default() });
    let _ = world.patch_object(patch).expect("fixture mesh shape applies");
}

fn fixture(with_prims: bool) -> (World, HashSet<AssetId>) {
    let count = usize::try_from(OBJECTS).expect("fixture count fits usize");
    let mut world = world_with_capacity(count * if with_prims { 2 } else { 1 });
    if with_prims {
        for id in 1..=OBJECTS {
            let shape =
                PrimShape { profile_hollow: u16::try_from(id).expect("fixture hollow fits its wire field"), ..PrimShape::default() };
            add_shaped(&mut world, u128::from(id), id, ParentUpdate::Root, 100.0, u64::from(id) * 4, shape);
        }
    }
    let mut assets = HashSet::with_capacity(count);
    for index in 0..OBJECTS {
        let id = (if with_prims { OBJECTS } else { 0 }) + index + 1;
        let asset = AssetId::new(Uuid::from_u128(100_000 + u128::from(id)));
        assets.insert(asset);
        add_mesh(&mut world, id, asset);
    }
    (world, assets)
}

fn warm(
    scene: &mut Reconciler,
    view: &mut render::scene::SceneView,
    world: &World,
    now: &mut Instant,
    mesh_assets: &HashSet<AssetId>,
    bytes: &[u8],
) {
    let mut pending = None;
    let mut wait_until = None;
    let mut headers = HashSet::with_capacity(mesh_assets.len());
    let mut lowest = HashSet::with_capacity(mesh_assets.len());
    scene.set_camera(camera(-300.0));
    for _ in 0..MAX_POLLS {
        if let Some(until) = wait_until.take() {
            *now = (*now + FRAME).max(until);
        } else if pending.is_some() {
            *now += FRAME;
        }
        if let Some(batch) = pending.take() {
            view.apply(batch).expect("warm publication applies");
        }
        match scene.poll(*now) {
            ReconcileStep::Asset(AssetChange::Request(request)) => {
                let asset = request.request.resource().id;
                match request.request {
                    Request::MeshHeader(_) => {
                        headers.insert(asset);
                    }
                    Request::MeshLod(block) if block.lod() == MeshLod::Lowest => {
                        lowest.insert(asset);
                    }
                    request => panic!("unexpected warm request: {request:?}"),
                }
                scene.complete_asset(request.ticket, prepared_asset(request.request, bytes.to_vec()));
            }
            ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
            ReconcileStep::Read(job) => {
                let result = reply(world, &job);
                scene.complete_read(job, result, *now);
            }
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Publish(batch) => pending = Some(batch),
            ReconcileStep::Wait(until) => wait_until = Some(until),
            ReconcileStep::Idle => {
                assert!(pending.is_none(), "all warm publications were acknowledged");
                assert_eq!(&headers, mesh_assets, "each mesh header is warm");
                assert_eq!(&lowest, mesh_assets, "each Lowest LOD is warm");
                return;
            }
            ReconcileStep::Closed => panic!("warm scene closed"),
        }
    }
    panic!("warm scene did not settle");
}

fn prim_high_targets(view: &render::scene::SceneView) -> usize {
    view.objects()
        .filter(|object| matches!(object.geometry(), Geometry::Prim(_)))
        .filter(|object| {
            object
                .mesh_state()
                .is_some_and(|(key, state)| key.detail == Detail::High && matches!(state, PreparedMesh::Pending | PreparedMesh::Ready(_)))
        })
        .count()
}

fn prims_are_high(view: &render::scene::SceneView) -> bool {
    view.objects()
        .filter(|object| matches!(object.geometry(), Geometry::Prim(_)))
        .all(|object| object.mesh_state().is_some_and(|(key, state)| key.detail == Detail::High && matches!(state, PreparedMesh::Ready(_))))
}

fn prim_lowest_ready(view: &render::scene::SceneView) -> usize {
    view.objects()
        .filter(|object| matches!(object.geometry(), Geometry::Prim(_)))
        .filter(|object| {
            object.mesh_state().is_some_and(|(key, state)| key.detail == Detail::Lowest && matches!(state, PreparedMesh::Ready(_)))
        })
        .count()
}

fn prim_high_ready(view: &render::scene::SceneView) -> usize {
    view.objects()
        .filter(|object| matches!(object.geometry(), Geometry::Prim(_)))
        .filter(|object| {
            object.mesh_state().is_some_and(|(key, state)| key.detail == Detail::High && matches!(state, PreparedMesh::Ready(_)))
        })
        .count()
}

fn meshes_at_lod(view: &render::scene::SceneView, lod: MeshLod) -> usize {
    view.objects()
        .filter(|object| matches!(object.geometry(), Geometry::Mesh(_)))
        .filter(|object| matches!(object.mesh(), Some((DrawnMeshKey::Asset { lod: published, .. }, _)) if published == lod))
        .count()
}

fn measure(with_prims: bool) -> Latency {
    let (world, mesh_assets) = fixture(with_prims);
    let budget = Budget::default();
    let (mut scene, mut view) = Reconciler::new(budget.clone());
    let mut now = Instant::now();
    let bytes = test_support::mesh::asset_with_lod_faces(&[(0, 1), (3, 1)]);
    warm(&mut scene, &mut view, &world, &mut now, &mesh_assets, &bytes);
    assert_eq!(
        meshes_at_lod(&view, MeshLod::Lowest),
        usize::try_from(OBJECTS).expect("fixture count fits usize"),
        "warm scene publishes Lowest before the camera change"
    );
    if with_prims {
        assert_eq!(
            prim_lowest_ready(&view),
            usize::try_from(OBJECTS).expect("fixture count fits usize"),
            "each prim has cached Lowest geometry"
        );
    }

    let mut pending: Option<Batch> = None;
    let mut wait_until = None;
    let mut first_high = None;
    let mut high_requests = HashSet::with_capacity(mesh_assets.len());
    let mut acknowledgements = 0;
    let mut frame = 0;
    let mut builds_during_requests = 0;
    scene.set_camera(camera(99.0));
    for _ in 0..MAX_POLLS {
        let advanced = if let Some(until) = wait_until.take() {
            now = (now + FRAME).max(until);
            true
        } else if pending.is_some() {
            now += FRAME;
            true
        } else {
            false
        };
        if advanced {
            frame += 1;
        }
        if let Some(batch) = pending.take() {
            view.apply(batch).expect("refinement publication applies");
            acknowledgements += 1;
            if let Some((first_frame, acks_at_first, targets_at_first, ready_at_first)) = first_high
                && meshes_at_lod(&view, MeshLod::High) == usize::try_from(OBJECTS).expect("fixture count fits usize")
                && (!with_prims || prims_are_high(&view))
            {
                assert_eq!(high_requests, mesh_assets, "each mesh received its High request");
                return Latency {
                    first_high_frame: first_frame,
                    acknowledgements_at_first_high: acks_at_first,
                    prim_high_targets_at_first_request: targets_at_first,
                    prim_high_at_first_request: ready_at_first,
                    completion_frames: frame - first_frame,
                    high_requests: high_requests.len(),
                    high_water_bytes: budget.high_water_bytes(),
                    builds_during_requests,
                };
            }
        }

        if advanced {
            scene.set_camera(camera(99.0 + (frame % 5) as f64 * 0.01));
        }
        match scene.poll(now) {
            ReconcileStep::Asset(AssetChange::Request(request)) => match request.request {
                Request::MeshLod(block) if block.lod() == MeshLod::High => {
                    if first_high.is_none() {
                        first_high = Some((frame, acknowledgements, prim_high_targets(&view), prim_high_ready(&view)));
                    }
                    high_requests.insert(request.request.resource().id);
                    scene.complete_asset(request.ticket, prepared_asset(request.request, bytes.clone()));
                }
                request => panic!("unexpected refinement request: {request:?}"),
            },
            ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete_read(job, result, now);
            }
            ReconcileStep::Build(job) => {
                if !high_requests.is_empty() && high_requests.len() < mesh_assets.len() {
                    builds_during_requests += 1;
                }
                scene.complete_mesh(job.run());
            }
            ReconcileStep::Publish(batch) => pending = Some(batch),
            ReconcileStep::Wait(until) => wait_until = Some(until),
            ReconcileStep::Idle => panic!("scene idled before publishing High for every mesh"),
            ReconcileStep::Closed => panic!("refinement scene closed"),
        }
    }
    panic!("High refinement did not settle");
}

#[test]
fn mixed_work_progresses_fairly() {
    let mesh_only = measure(false);
    let with_prims = measure(true);
    assert!(with_prims.builds_during_requests > 0, "prim builds must progress before the mesh request burst drains");
    println!(
        "synthetic 60 Hz scheduler ticks, not elapsed time: first High mesh/mixed {}/{}, mixed prim targets {}/{}, ready {}/{}, acks before High mesh/mixed {}/{}, finish frames mesh/mixed {}/{}, High requests {}/{}, peak bytes {}/{}",
        mesh_only.first_high_frame,
        with_prims.first_high_frame,
        with_prims.prim_high_targets_at_first_request,
        OBJECTS,
        with_prims.prim_high_at_first_request,
        OBJECTS,
        mesh_only.acknowledgements_at_first_high,
        with_prims.acknowledgements_at_first_high,
        mesh_only.completion_frames,
        with_prims.completion_frames,
        mesh_only.high_requests,
        with_prims.high_requests,
        mesh_only.high_water_bytes,
        with_prims.high_water_bytes,
    );
    assert!(
        with_prims.prim_high_targets_at_first_request > 0 && with_prims.prim_high_at_first_request < usize::try_from(OBJECTS).unwrap(),
        "a camera change must issue bounded mesh demand while prim LOD work is still progressing; first High request saw {}/{} targets and {}/{} ready after {} synthetic frames",
        with_prims.prim_high_targets_at_first_request,
        OBJECTS,
        with_prims.prim_high_at_first_request,
        OBJECTS,
        with_prims.first_high_frame,
    );
}
