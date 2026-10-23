use super::*;
use crate::asset_coordinator::AssetCoordinator;
use crate::world_reader::WorldReadHost;
use futures::join;
use math::{DVec3, Vec3};
use scene::Status;
use std::{
    future::{pending, ready},
    num::NonZeroU32,
};
use utils::ExperienceId;
use uuid::Uuid;
use world::{
    Appearance, FaceAppearance, FieldUpdate, Kind, ObjectObservation, ObjectPatch, OrderEvidence::Ordered, ParentUpdate, Pose, Region,
    RegionInfo, Shape, SurfaceAppearance, UpdateSource, World, WorldLimits,
};

fn world() -> World {
    World::new(WorldLimits {
        max_region_registrations: 2,
        max_entities: 16,
        max_entities_per_region: 16,
        max_tombstones_per_region: 16,
        max_tombstones: 16,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(16),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(16),
        max_journal_entries: 32,
        max_journal_changes: 128,
        max_region_name_bytes: 64,
        max_recovery_targets: 8,
    })
}

#[test]
fn ocean_follows_agent_region() {
    let mut world = world();
    let agent = Uuid::from_u128(1);
    for region in [Region::new(1), Region::new(2)] {
        let _ = world.adopt_region(region).unwrap();
        let _ = world
            .patch_region(
                region,
                RegionInfo { origin: Some(DVec3::new((region.get() - 1) as f64 * 256.0, 0.0, 7.0)), ..RegionInfo::default() },
            )
            .unwrap();
    }
    let _ = world
        .observe_object(ObjectObservation::new(
            UpdateSource::new(Region::new(1), 1, Ordered(1), 0),
            agent,
            NonZeroU32::new(1).unwrap(),
            Kind::Avatar,
            ParentUpdate::Root,
        ))
        .unwrap();
    let _ = world.observe_water_height(UpdateSource::new(Region::new(2), 1, Ordered(1), 0), 31.0).unwrap();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut hydrate = |world: &World| {
        scene.notify_world_change();
        let now = Instant::now();
        for _ in 0..1000 {
            match scene.poll(now) {
                ReconcileStep::Read(job) => {
                    let reply = match job.query() {
                        ReadQuery::Barrier => Ok(ReadReply::Barrier(world.read_barrier())),
                        ReadQuery::Objects(cursor) => world.object_list_page(*cursor, 256).map(ReadReply::Objects),
                        ReadQuery::Regions(cursor) => world.region_page(*cursor, 64).map(ReadReply::Regions),
                        ReadQuery::Details(entities) => world.object_detail_batch(entities).map(ReadReply::Details),
                        ReadQuery::Changes(cursor) => world.change_page(*cursor, 256).map(ReadReply::Changes),
                        ReadQuery::Terrain(region, cursor) => world.terrain_page(*region, *cursor, 8).map(ReadReply::Terrain),
                        ReadQuery::Environment(region) => Ok(ReadReply::Environment(world.environment(*region))),
                    };
                    scene.complete_read(job, reply.map_err(ReadFailure::World), now);
                }
                ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
                ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
                ReconcileStep::Continue => {}
                ReconcileStep::Idle => return water_height(&view, agent),
                _ => panic!("expected scene reconciliation"),
            }
        }
        panic!("scene did not settle");
    };
    assert_eq!(hydrate(&world), None, "another region's water must not supply the ocean height");
    let _ = world.observe_water_height(UpdateSource::new(Region::new(1), 2, Ordered(2), 0), 17.0).unwrap();
    assert_eq!(hydrate(&world), Some(24.0));
    let _ = world.retire_region(Region::new(1)).unwrap();
    assert_eq!(hydrate(&world), None, "retirement must remove the previous ocean height");
    let _ = world
        .observe_object(ObjectObservation::new(
            UpdateSource::new(Region::new(2), 3, Ordered(3), 0),
            agent,
            NonZeroU32::new(1).unwrap(),
            Kind::Avatar,
            ParentUpdate::Root,
        ))
        .unwrap();
    assert_eq!(hydrate(&world), Some(38.0), "the new avatar region supplies its own observed height");
}

#[tokio::test]
async fn missing_asset_preserves_scene() {
    let mut world = world();
    let region = Region::new(1);
    let _ = world.adopt_region(region).unwrap();
    let _ = world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).unwrap();
    let mut object = ObjectObservation::new(
        UpdateSource::new(region, 1, Ordered(1), 0),
        Uuid::from_u128(1),
        NonZeroU32::new(1).unwrap(),
        Kind::Object,
        ParentUpdate::Root,
    );
    object.pose = Some(Pose { position: Some(Vec3::new(10.0, 0.0, 0.0)), rotation: Some(math::Quat::IDENTITY) });
    object.scale = Some(Vec3::ONE);
    object.shape = FieldUpdate::Set(Shape::Prim(Default::default()));
    object.appearance = FieldUpdate::Set(Appearance::Surface);
    object.surface_appearance = Some(
        SurfaceAppearance::new(
            FaceAppearance { texture_id: Uuid::from_u128(100), color: [255, 0, 0, 255], ..FaceAppearance::default() },
            Vec::new(),
        )
        .unwrap(),
    );
    let _ = world.observe_object(object).unwrap();
    let mut reads = WorldReadHost::new(&world, Handle::current());
    let assets = AssetCoordinator::new(Handle::current(), pending());
    let (_pushes, environments) = crate::session::EnvironmentInbox::channel();
    let mut feed = SceneFeed::start(
        SceneSources { reader: reads.reader(), environments, assets: assets.handle() },
        Uuid::from_u128(2),
        watch::channel(None).0,
        Handle::current(),
        || {},
    );
    feed.set_camera(Camera::new(DVec3::ZERO, Vec3::X, 1.0, [1000, 1000]).unwrap());
    let receiving = async {
        loop {
            let batch = feed.incoming.recv().await.expect("scene remains active");
            feed.view.apply(batch).unwrap();
            if let Some(face) = feed.view.objects().next().and_then(|object| object.materials()).and_then(|faces| faces.first())
                && face
                    .base_color_state
                    .is_some_and(|state| matches!(state.resolution, scene::Resolution::Failed(assets::Failure::RouteClosed)))
            {
                assert_eq!(face.base_color(), [1.0, 0.0, 0.0, 1.0]);
                assert!(face.base_color.is_none());
                assert_eq!(face.alpha, scene::Alpha::Unknown);
                break;
            }
        }
    }
    .fuse();
    let serving = async {
        loop {
            let Err(request) = reads.select(pending::<()>()).await else { unreachable!() };
            request.serve(&world);
        }
    }
    .fuse();
    pin_mut!(receiving, serving);
    futures::select_biased! {
        _ = receiving => {}
        _ = serving => unreachable!(),
    }
}

#[tokio::test]
async fn stalled_publication_preserves_intake() {
    let mut world = world();
    let region = Region::new(1);
    let _ = world.adopt_region(region).unwrap();
    let _ = world
        .observe_object(ObjectObservation::new(
            UpdateSource::new(region, 1, Ordered(1), 0),
            Uuid::from_u128(1),
            NonZeroU32::new(1).unwrap(),
            Kind::Object,
            ParentUpdate::Root,
        ))
        .unwrap();
    let mut service = WorldReadHost::new(&world, Handle::current());
    let reader = service.reader();
    let assets = AssetCoordinator::new(Handle::current(), pending());
    let (pushes, environments) = crate::session::EnvironmentInbox::channel();
    let mut feed = SceneFeed::start(
        SceneSources { reader: reader.clone(), environments, assets: assets.handle() },
        Uuid::from_u128(1),
        watch::channel(None).0,
        Handle::current(),
        || {},
    );
    let batch = {
        let receiving = async {
            loop {
                let batch = feed.incoming.recv().await.expect("live Scene publication");
                if batch.status() == Status::Ready {
                    break batch;
                }
                feed.view.apply(batch).unwrap();
            }
        }
        .fuse();
        let serving = async {
            loop {
                let Err(request) = service.select(pending::<()>()).await else { unreachable!() };
                request.serve(&world);
            }
        }
        .fuse();
        pin_mut!(receiving, serving);
        futures::select_biased! {
            batch = receiving => batch,
            _ = serving => unreachable!(),
        }
    };
    assert_eq!(feed.view.status(), Status::Loading);
    let initial = world.revision();
    let (barrier, turns) = join!(reader.barrier(), async {
        let mut turns = 0;
        while turns < 1024 {
            match service.select(ready(())).await {
                Ok(()) => {
                    turns += 1;
                    let mut patch = ObjectPatch::new(
                        UpdateSource::new(region, turns + 1, Ordered(u64::from(turns + 1)), 0),
                        NonZeroU32::new(1).unwrap(),
                    );
                    patch.pose = Some(Pose { position: Some(Vec3::new(turns as f32, 0.0, 0.0)), rotation: None });
                    let _ = world.patch_object(patch).unwrap();
                    service.publish(&world);
                }
                Err(request) => request.serve(&world),
            }
        }
        turns
    });
    assert_eq!(turns, 1024);
    assert!(barrier.unwrap().revision > initial);
    assert!(world.revision() > initial);
    let experience = ExperienceId::new(Uuid::from_u128(44)).unwrap();
    let push = scene::EnvironmentPush::Partial {
        experience,
        sky: vec![scene::EnvironmentProperty { name: "gamma".into(), source: b"r1.4".as_slice().into() }],
        water: Vec::new(),
        transition: Duration::ZERO,
    };
    assert!(pushes.send(crate::session::EnvironmentCommand { region, push }).await.is_ok());
    feed.view.apply(batch).unwrap();
    assert_eq!(feed.view.object_count(), 1);
    {
        let receiving = async {
            loop {
                let batch = feed.incoming.recv().await.expect("live Scene publication");
                feed.view.apply(batch).unwrap();
                if let Some(environment) = feed.view.viewer_environment() {
                    assert_eq!(environment.sky_properties()["gamma"].experience(), experience);
                    break;
                }
            }
        }
        .fuse();
        let serving = async {
            loop {
                let Err(request) = service.select(pending::<()>()).await else { unreachable!() };
                request.serve(&world);
            }
        }
        .fuse();
        {
            pin_mut!(receiving, serving);
            futures::select_biased! { _ = receiving => {}, _ = serving => unreachable!() }
        }
    }
    service.finish();
    assert!(feed.incoming.recv().await.is_none());
    assert_eq!(feed.view.status(), Status::Closed);
}
