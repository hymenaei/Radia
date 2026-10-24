use super::support::{
    BTreeMap, BTreeSet, Budget, DVec3, Instant, Mesh, ParentUpdate, PreparedMesh, ReadFailure, ReadQuery, ReadReply, ReconcileStep,
    Reconciler, RecoveryReason, Region, RegionInfo, SceneView, Status, SurfaceKind, TerrainPatch, TerrainPatchId, Vec3, add, move_root,
    next_terrain_job, positions, reply, settle, settle_work, source, world, world_at, world_with_journal,
};

#[test]
fn terrain_and_water_lifetimes() {
    let mut world = world();
    let first = world::TerrainPatch::new(0, 0, Box::new([7.0; 256])).unwrap();
    let _ = world.observe_terrain_patch(source(1, 1), first).unwrap();
    let _ = world.observe_water_height(source(1, 2), 0.0).unwrap();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.patch_count(), 1);
    assert_eq!(view.water_height(Region::new(1)), Some(0.0));
    assert_eq!(view.patches().next().unwrap().1.source().heights(), &[7.0; 256]);
    let updated = world::TerrainPatch::new(0, 0, Box::new([8.0; 256])).unwrap();
    let _ = world.observe_terrain_patch(source(1, 3), updated).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.patch_count(), 1);
    assert_eq!(view.patches().next().unwrap().1.source().heights(), &[8.0; 256]);
    let _ = world.retire_region(Region::new(1)).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.region_count(), 0);
    assert_eq!(view.patch_count(), 0);
    assert_eq!(view.water_height(Region::new(1)), None);
    assert_eq!(view.surfaces().count(), 0);
}

#[test]
fn terrain_loading_keeps_history() {
    let mut world = world_with_journal(DVec3::ZERO, 8);
    add(&mut world, 1, 1, ParentUpdate::Root, 0.0, 1);
    for y in 0..16 {
        for x in 0..16 {
            let patch = TerrainPatch::new(x, y, Box::new([1.0; 256])).unwrap();
            let _ = world.observe_terrain_patch(source(1, 2 + u64::from(y) * 16 + u64::from(x)), patch).unwrap();
        }
    }
    let _ = world.observe_water_height(source(1, 300), 20.0).unwrap();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    let mut terrain_reads = 0;
    for _ in 0..10_000 {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                if matches!(job.query(), ReadQuery::Terrain(..)) {
                    terrain_reads += 1;
                    move_root(&mut world, terrain_reads as f32, 1000 + terrain_reads);
                    scene.notify_world_change();
                }
                scene.complete_read(job, result, now);
            }
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            ReconcileStep::Idle => break,
            _ => panic!("unexpected terrain loading state"),
        }
        assert!(
            !matches!(scene.status(), Status::Recovering { .. } | Status::Degraded { .. }),
            "terrain paging must not starve journal reads"
        );
    }
    assert_eq!(terrain_reads, 32);
    assert_eq!(view.patch_count(), 256);
    assert_eq!(view.water_height(Region::new(1)), Some(20.0));
    assert_eq!(view.revision(), Some(world.revision()));
    assert_eq!(positions(&view), BTreeMap::from([(1, 32.0)]));
}

#[test]
fn terrain_prepares_during_publication() {
    let mut world = world();
    for x in 0..8 {
        let patch = TerrainPatch::new(x, 0, Box::new([1.0; 256])).unwrap();
        let _ = world.observe_terrain_patch(source(1, u64::from(x) + 1), patch).unwrap();
    }
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    for x in 0..8 {
        let patch = TerrainPatch::new(x, 0, Box::new([2.0; 256])).unwrap();
        let _ = world.observe_terrain_patch(source(1, u64::from(x) + 20), patch).unwrap();
    }
    scene.notify_world_change();
    let mut running = Vec::new();
    let held = loop {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete_read(job, result, now);
            }
            ReconcileStep::Publish(batch) => break batch,
            ReconcileStep::Build(job) => running.push(job),
            ReconcileStep::Continue => {}
            _ => panic!("expected terrain update"),
        }
    };
    for job in running {
        scene.complete_mesh(job.run());
    }
    let mut built = 0;
    for _ in 0..100 {
        match scene.poll(now) {
            ReconcileStep::Build(job) => {
                built += 1;
                scene.complete_mesh(job.run());
            }
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete_read(job, result, now);
            }
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(_) | ReconcileStep::Idle => break,
            ReconcileStep::Publish(_) => panic!("only one publication may await delivery"),
            _ => panic!("unexpected terrain preparation state"),
        }
    }
    assert!(built > 0, "a slow frame must not stop terrain preparation");
    assert_eq!(terrain_mesh(&view, 1, 0, 0).bounds().min.z, 1.0, "undelivered work must not alter the published view");
    view.apply(held).unwrap();
    settle(&mut scene, &mut view, &world, &mut now);
    for x in 0..8 {
        assert_eq!(terrain_mesh(&view, 1, x, 0).bounds().min.z, 2.0);
    }
}

#[test]
fn hydration_allows_ready_surfaces() {
    let mut world = world();
    let coordinates = [(0, 0), (14, 0), (15, 0), (14, 1), (15, 1), (0, 15), (1, 15), (2, 15), (3, 15), (4, 15)];
    for (index, (x, y)) in coordinates.into_iter().enumerate() {
        let patch = TerrainPatch::new(x, y, Box::new([3.0; 256])).unwrap();
        let _ = world.observe_terrain_patch(source(1, index as u64 + 1), patch).unwrap();
    }
    let budget = Budget::default();
    let (mut scene, mut view) = Reconciler::new(budget.clone());
    let mut now = Instant::now();
    let pending = loop {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                if matches!(job.query(), ReadQuery::Terrain(_, Some(_))) {
                    break job;
                }
                let result = reply(&world, &job);
                scene.complete_read(job, result, now);
            }
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Continue => {}
            _ => panic!("expected terrain hydration progress"),
        }
    };
    assert!(
        view.surfaces().any(|surface| surface.kind == SurfaceKind::Terrain(TerrainPatchId::new(15, 0).unwrap())),
        "ready terrain must publish before the next hydration reply"
    );
    assert_eq!(terrain_mesh(&view, 1, 15, 0).bounds().min.z, 3.0);
    assert!(
        !view.surfaces().any(|surface| surface.kind == SurfaceKind::Terrain(TerrainPatchId::new(0, 0).unwrap())),
        "incomplete terrain must still wait for hydration"
    );
    let result = reply(&world, &pending);
    scene.complete_read(pending, result, now);
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.surfaces().count(), coordinates.len(), "deferred patches must resume after hydration finishes");
    assert_eq!(terrain_mesh(&view, 1, 0, 0).bounds().min.z, 3.0);
    assert_eq!(view.status(), Status::Ready);
    drop(scene);
    drop(view);
    assert_eq!(budget.charged_bytes(), 0);
}

#[test]
fn terrain_streams_during_bootstrap() {
    let mut world = world();
    for id in 1..=300 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 0.0, id as u64);
    }
    for x in 0..8 {
        let patch = TerrainPatch::new(x, 0, Box::new([1.0; 256])).unwrap();
        let _ = world.observe_terrain_patch(source(1, u64::from(x) + 400), patch).unwrap();
    }
    let _ = world.observe_water_height(source(1, 500), 20.0).unwrap();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    let mut observed = false;
    for _ in 0..1000 {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                if matches!(job.query(), ReadQuery::Objects(_)) && !observed {
                    observed = true;
                    assert_eq!(view.patch_count(), 8, "terrain must appear before object enumeration completes");
                    assert_eq!(view.surfaces().count(), 9);
                    assert_eq!(view.status(), Status::Loading);
                    assert_eq!(view.revision(), None, "progressive startup must not acknowledge unreconciled World changes");
                }
                let result = reply(&world, &job);
                scene.complete_read(job, result, now);
            }
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            ReconcileStep::Idle => break,
            _ => panic!("unexpected bootstrap state"),
        }
    }
    assert!(observed);
    assert_eq!(view.object_count(), 300);
    assert_eq!(view.revision(), Some(world.revision()));
    assert_eq!(view.status(), Status::Ready);
}

#[test]
fn bootstrap_ignores_frame_latency() {
    let mut world = world();
    for id in 1..=300 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 0.0, id as u64);
    }
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    let held = loop {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete_read(job, result, now);
            }
            ReconcileStep::Publish(batch) => break batch,
            ReconcileStep::Continue => {}
            _ => panic!("expected startup publication"),
        }
    };
    for _ in 0..1000 {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete_read(job, result, now);
            }
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(_) => break,
            ReconcileStep::Publish(_) => panic!("a second batch cannot overtake the held batch"),
            _ => panic!("unexpected startup state"),
        }
    }
    assert_eq!(view.object_count(), 0, "pending work cannot leak into the view");
    view.apply(held).unwrap();
    let reads = settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(reads, 0, "bootstrap reads must complete independently of frame delivery");
    assert_eq!(view.object_count(), 300);
    assert_eq!(view.revision(), Some(world.revision()));
}

#[test]
fn startup_failure_retains_terrain() {
    let mut world = world();
    for id in 1..=300 {
        add(&mut world, id, id as u32, ParentUpdate::Root, 0.0, id as u64);
    }
    let patch = TerrainPatch::new(0, 0, Box::new([1.0; 256])).unwrap();
    let _ = world.observe_terrain_patch(source(1, 400), patch).unwrap();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    loop {
        match scene.poll(now) {
            ReconcileStep::Read(job) if matches!(job.query(), ReadQuery::Objects(_)) => {
                assert_eq!(terrain_mesh(&view, 1, 0, 0).bounds().min.z, 1.0);
                scene.complete_read(job, Err(ReadFailure::Busy), now);
                break;
            }
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete_read(job, result, now);
            }
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            _ => panic!("expected progressive startup"),
        }
    }
    let patch = TerrainPatch::new(0, 0, Box::new([9.0; 256])).unwrap();
    let _ = world.observe_terrain_patch(source(1, 500), patch).unwrap();
    let mut checked = false;
    for _ in 0..1000 {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                if matches!(job.query(), ReadQuery::Objects(_)) && !checked {
                    assert!(matches!(view.status(), Status::Recovering { .. }));
                    let surface = view
                        .surfaces()
                        .find(|surface| matches!(surface.kind, SurfaceKind::Terrain(_)))
                        .expect("recovery must preserve previously published terrain");
                    let PreparedMesh::Ready(mesh) = &surface.mesh else { panic!("retained terrain must stay ready") };
                    assert_eq!(mesh.mesh().bounds().min.z, 1.0, "recovery cannot expose partially rebuilt terrain");
                    checked = true;
                }
                let result = reply(&world, &job);
                scene.complete_read(job, result, now);
            }
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            ReconcileStep::Idle => break,
            _ => panic!("unexpected recovery state"),
        }
    }
    assert!(checked);
    assert_eq!(view.object_count(), 300);
    assert_eq!(terrain_mesh(&view, 1, 0, 0).bounds().min.z, 9.0);
    assert_eq!(view.revision(), Some(world.revision()));
    assert_eq!(view.status(), Status::Ready);
}

#[test]
fn water_precedes_terrain_builds() {
    let mut world = world();
    for x in 0..8 {
        let patch = TerrainPatch::new(x, 0, Box::new([1.0; 256])).unwrap();
        let _ = world.observe_terrain_patch(source(1, u64::from(x) + 1), patch).unwrap();
    }
    let _ = world.observe_water_height(source(1, 10), 20.0).unwrap();
    let _ = world.adopt_region(Region::new(2)).unwrap();
    let _ = world.patch_region(Region::new(2), RegionInfo { origin: Some(DVec3::X * 256.0), ..RegionInfo::default() }).unwrap();
    let _ = world.observe_water_height(source(2, 1), 25.0).unwrap();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    let mut appeared = BTreeSet::new();
    for _ in 0..1000 {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete_read(job, result, now);
            }
            ReconcileStep::Publish(batch) => {
                view.apply(batch).unwrap();
                for surface in view.surfaces() {
                    if appeared.insert(surface.region) {
                        assert_eq!(surface.kind, SurfaceKind::Water, "observed water must not wait behind its terrain queue");
                        let PreparedMesh::Ready(mesh) = &surface.mesh else { panic!("water mesh must be ready") };
                        assert_eq!(mesh.mesh().vertices().len(), 4);
                    }
                }
                if appeared.len() == 2 {
                    assert_eq!(view.water_height(Region::new(2)), Some(25.0));
                    return;
                }
            }
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => now = until,
            _ => panic!("expected water build progress"),
        }
    }
    panic!("water did not become visible");
}

fn terrain_mesh(view: &SceneView, region: u64, x: u8, y: u8) -> &Mesh {
    let kind = SurfaceKind::Terrain(TerrainPatchId::new(x, y).unwrap());
    let surface = view.surfaces().find(|surface| surface.region == Region::new(region) && surface.kind == kind).unwrap();
    let PreparedMesh::Ready(mesh) = &surface.mesh else { panic!("expected ready terrain") };
    mesh.mesh()
}

#[test]
fn terrain_samples_and_gaps() {
    let mut world = world();
    let plane = |x: u8, y: u8| {
        TerrainPatch::new(
            x,
            y,
            Box::new(std::array::from_fn(|index| {
                7.0 + 2.0 * (f32::from(x) * 16.0 + (index % 16) as f32) + 3.0 * (f32::from(y) * 16.0 + (index / 16) as f32)
            })),
        )
        .unwrap()
    };
    let _ = world.observe_terrain_patch(source(1, 1), plane(0, 0)).unwrap();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let mesh = terrain_mesh(&view, 1, 0, 0);
    assert_eq!(mesh.vertices().len(), 256);
    assert_eq!(mesh.indices().len(), 15 * 15 * 6, "unknown neighbors must leave uncovered cells");
    assert_eq!(mesh.vertices()[0].position, Vec3::new(0.0, 0.0, 7.0));
    assert_eq!(mesh.vertices()[255].position, Vec3::new(15.0, 15.0, 82.0));
    assert!(!view.surfaces().any(|surface| surface.kind == SurfaceKind::Water));
    for (order, (x, y)) in [(1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
        let _ = world.observe_terrain_patch(source(1, order as u64 + 2), plane(x, y)).unwrap();
    }
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    let mesh = terrain_mesh(&view, 1, 0, 0);
    assert_eq!(mesh.vertices().len(), 289);
    assert_eq!(mesh.indices().len(), 1536);
    assert_eq!(mesh.vertices()[288].position, Vec3::new(16.0, 16.0, 87.0));
    assert_eq!(&mesh.indices()[..6], &[0, 1, 18, 0, 18, 17]);
    let normal = mesh.vertices()[8 * 17 + 8].normal;
    assert!((normal - Vec3::new(-2.0, -3.0, 1.0).normalize()).length() < 1e-6);
    for triangle in mesh.indices().as_chunks::<3>().0 {
        let [a, b, c] = triangle.map(|index| mesh.vertices()[index as usize].position);
        assert!((b - a).cross(c - a).z > 0.0);
    }
    let extreme =
        TerrainPatch::new(4, 4, Box::new(std::array::from_fn(|index| if index % 2 == 0 { f32::MAX } else { -f32::MAX }))).unwrap();
    let _ = world.observe_terrain_patch(source(1, 10), extreme).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert!(terrain_mesh(&view, 1, 4, 4).vertices().iter().all(|vertex| vertex.position.is_finite() && vertex.normal.is_finite()));
}

#[test]
fn terrain_extent_validation() {
    for dimensions in [[0, 0], [512, 256]] {
        let mut world = world();
        let _ = world.observe_terrain_patch(source(1, 1), TerrainPatch::new(0, 0, Box::new([0.0; 256])).unwrap()).unwrap();
        let (mut scene, mut view) = Reconciler::new(Budget::default());
        let now = Instant::now();
        loop {
            match scene.poll(now) {
                ReconcileStep::Read(job) => {
                    let mut result = reply(&world, &job);
                    if let Ok(ReadReply::Terrain(Some(page))) = &mut result {
                        page.dimensions = dimensions;
                        scene.complete_read(job, result, now);
                        break;
                    }
                    scene.complete_read(job, result, now);
                }
                ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
                ReconcileStep::Continue => {}
                _ => panic!("expected terrain hydration"),
            }
        }
        assert!(matches!(scene.status(), Status::Recovering { reason: RecoveryReason::InvalidReply, .. }));
        assert_eq!(view.surfaces().count(), 0);
    }
}

#[test]
fn terrain_region_edges() {
    let origin = DVec3::new(1_000_000_000.0, 1_000_000_000.0, 0.0);
    let mut world = world_at(origin);
    let _ = world.adopt_region(Region::new(2)).unwrap();
    let _ = world.patch_region(Region::new(2), RegionInfo { origin: Some(origin + DVec3::X * 256.0), ..RegionInfo::default() }).unwrap();
    let _ = world.observe_terrain_patch(source(1, 1), TerrainPatch::new(15, 0, Box::new([10.0; 256])).unwrap()).unwrap();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(terrain_mesh(&view, 1, 15, 0).bounds().max.x, 15.0, "active neighbor without samples must not be clamped");
    let _ = world.observe_terrain_patch(source(2, 2), TerrainPatch::new(0, 0, Box::new([20.0; 256])).unwrap()).unwrap();
    let _ = world.observe_water_height(source(1, 3), 17.0).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    let west = terrain_mesh(&view, 1, 15, 0);
    let east = terrain_mesh(&view, 2, 0, 0);
    assert_eq!(west.bounds().max.x, 16.0);
    let west_edge = west.vertices().iter().find(|v| v.position.x == 16.0 && v.position.y == 8.0).unwrap();
    let east_edge = east.vertices().iter().find(|v| v.position.x == 0.0 && v.position.y == 8.0).unwrap();
    assert_eq!(west_edge.position.z, 20.0);
    assert_eq!(west_edge.normal, east_edge.normal);
    let positions: BTreeMap<_, _> = view.surfaces().map(|s| ((s.region, s.kind), s.position)).collect();
    let west_position = positions[&(Region::new(1), SurfaceKind::Terrain(TerrainPatchId::new(15, 0).unwrap()))];
    let east_position = positions[&(Region::new(2), SurfaceKind::Terrain(TerrainPatchId::new(0, 0).unwrap()))];
    assert_eq!(west_position + west_edge.position.as_dvec3(), east_position + east_edge.position.as_dvec3());
    assert_eq!(west_position, origin + DVec3::X * 240.0);
    let water = view.surfaces().find(|s| s.kind == SurfaceKind::Water).unwrap();
    assert_eq!(water.region, Region::new(1));
    assert_eq!(water.position, origin);
    let PreparedMesh::Ready(mesh) = &water.mesh else { panic!("expected water") };
    assert_eq!(mesh.mesh().bounds().min, Vec3::new(0.0, 0.0, 17.0));
    assert_eq!(mesh.mesh().bounds().max, Vec3::new(256.0, 256.0, 17.0));
    let _ = world.retire_region(Region::new(1)).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert!(view.surfaces().all(|s| s.region == Region::new(2) && s.position == east_position));
}

#[test]
fn terrain_rebuild_bounds() {
    let mut world = world();
    for y in 0..16 {
        for x in 0..16 {
            let _ = world
                .observe_terrain_patch(
                    source(1, u64::from(y) * 16 + u64::from(x) + 1),
                    TerrainPatch::new(x, y, Box::new([0.0; 256])).unwrap(),
                )
                .unwrap();
        }
    }
    let budget = Budget::default();
    let (mut scene, mut view) = Reconciler::new(budget.clone());
    let mut now = Instant::now();
    assert_eq!(settle_work(&mut scene, &mut view, &world, &mut now).1, 256);
    let ids = |view: &SceneView| {
        view.surfaces()
            .map(|s| {
                let PreparedMesh::Ready(mesh) = &s.mesh else { panic!("expected ready mesh") };
                (s.kind, mesh.id())
            })
            .collect::<BTreeMap<_, _>>()
    };
    let baseline = ids(&view);
    for order in 300..332 {
        let _ = world.observe_terrain_patch(source(1, order), TerrainPatch::new(8, 8, Box::new([order as f32; 256])).unwrap()).unwrap();
        scene.notify_world_change();
        assert_eq!(
            settle_work(&mut scene, &mut view, &world, &mut now).1,
            9,
            "isolated patch edit must only rebuild its normal and edge dependencies"
        );
        for (kind, id) in ids(&view) {
            let SurfaceKind::Terrain(patch) = kind else { unreachable!() };
            let (x, y) = patch.coordinates();
            if !(7..=9).contains(&x) || !(7..=9).contains(&y) {
                assert_eq!(id, baseline[&kind], "unaffected patches must retain geometry");
            }
        }
    }
    let before_water = ids(&view);
    let _ = world.observe_water_height(source(1, 400), 24.0).unwrap();
    scene.notify_world_change();
    assert_eq!(settle_work(&mut scene, &mut view, &world, &mut now).1, 1);
    for (kind, id) in before_water {
        assert_eq!(ids(&view)[&kind], id);
    }
    assert!(budget.high_water_bytes() < 16 * 1024 * 1024);
    eprintln!("256 terrain patches, 32 edits: 9 rebuilds per edit, peak {} bytes", budget.high_water_bytes());
    scene.close();
    drop(scene);
    drop(view);
    assert_eq!(budget.charged_bytes(), 0);
}

#[test]
fn retired_terrain_jobs() {
    let mut world = world();
    let _ = world.observe_terrain_patch(source(1, 1), TerrainPatch::new(0, 0, Box::new([f32::MAX; 256])).unwrap()).unwrap();
    let budget = Budget::default();
    let (mut scene, mut view) = Reconciler::new(budget.clone());
    let mut now = Instant::now();
    let job = loop {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let result = reply(&world, &job);
                scene.complete_read(job, result, now);
            }
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Build(job) => break job,
            ReconcileStep::Continue => {}
            _ => panic!("expected terrain work"),
        }
    };
    let _ = world.retire_region(Region::new(1)).unwrap();
    let _ = world.adopt_region(Region::new(2)).unwrap();
    let _ = world.patch_region(Region::new(2), RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).unwrap();
    let _ = world.observe_terrain_patch(source(2, 2), TerrainPatch::new(0, 0, Box::new([9.0; 256])).unwrap()).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    scene.complete_mesh(job.run());
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.surfaces().count(), 1);
    assert_eq!(terrain_mesh(&view, 2, 0, 0).bounds().min.z, 9.0);
    scene.close();
    drop(scene);
    drop(view);
    assert_eq!(budget.charged_bytes(), 0);
}

#[test]
fn abandoned_terrain_work_retries() {
    let mut world = world();
    let _ = world.observe_terrain_patch(source(1, 1), TerrainPatch::new(0, 0, Box::new([1.0; 256])).unwrap()).unwrap();
    let budget = Budget::default();
    let (mut scene, mut view) = Reconciler::new(budget.clone());
    let mut now = Instant::now();

    let job = next_terrain_job(&mut scene, &mut view, &world, &mut now);
    let before_drop = budget.charged_bytes();
    drop(job);
    assert!(budget.charged_bytes() < before_drop, "abandoning a job releases its reservation");

    let job = next_terrain_job(&mut scene, &mut view, &world, &mut now);
    scene.complete_mesh(job.run());
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(terrain_mesh(&view, 1, 0, 0).bounds().min.z, 1.0);
    scene.close();
    drop(scene);
    drop(view);
    assert_eq!(budget.charged_bytes(), 0);
}
