use super::*;
mod tests {
    use super::*;
    use crate::asset_coordinator::AssetCoordinator;
    use crate::world_reader::ReadRequest;
    use crate::world_reader::WorldReadHost;
    use futures::{Future, FutureExt, join};
    use math::{DVec3, Quat, Vec3};
    use render::scene::{EnvironmentProperty, EnvironmentPush, Status};
    use std::{
        future::{pending, ready},
        num::NonZeroU32,
        time::Duration,
    };
    use utils::{AssetId, ExperienceId};
    use uuid::Uuid;
    use world::{
        Appearance, EnvironmentAssignment, EnvironmentSource, FaceAppearance, FieldUpdate, Kind, ObjectObservation, ObjectPatch,
        OrderEvidence::Ordered, ParentUpdate, Pose, Region, RegionInfo, Shape, SurfaceAppearance, UpdateSource, World, WorldLimits,
    };

    async fn select_read<T>(service: &mut WorldReadHost, intake: impl Future<Output = T>) -> Result<T, ReadRequest> {
        if let Some(request) = service.take_reserved() {
            return Err(request);
        }
        let result = {
            let intake = intake.fuse();
            let request = service.next_request().fuse();
            pin_mut!(intake, request);
            futures::select_biased! {
                input = intake => Ok(input),
                request = request => Err(request),
            }
        };
        if result.is_ok() {
            service.record_intake_turn();
        }
        result
    }

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

    fn settle_feed(scene: &mut Reconciler, feed: &mut SceneFeed, sender: &mpsc::Sender<Batch>, world: &World) {
        let mut now = Instant::now();
        for _ in 0..10_000 {
            match scene.poll(now) {
                ReconcileStep::Asset(_) => {}
                ReconcileStep::Read(job) => {
                    let result = match job.query() {
                        ReadQuery::Barrier => Ok(ReadReply::Barrier(world.read_barrier())),
                        ReadQuery::Objects(cursor) => world.object_list_page(*cursor, 256).map(ReadReply::Objects),
                        ReadQuery::Regions(cursor) => world.region_page(*cursor, 64).map(ReadReply::Regions),
                        ReadQuery::Details(entities) => world.object_detail_batch(entities).map(ReadReply::Details),
                        ReadQuery::Changes(cursor) => world.change_page(*cursor, 256).map(ReadReply::Changes),
                        ReadQuery::Terrain(region, cursor) => world.terrain_page(*region, *cursor, 8).map(ReadReply::Terrain),
                        ReadQuery::Environment(region) => Ok(ReadReply::Environment(world.environment(*region))),
                    };
                    scene.complete_read(job, result.map_err(ReadFailure::World), now);
                }
                ReconcileStep::Publish(batch) => {
                    sender.try_send(batch).unwrap();
                    feed.drain().unwrap();
                }
                ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
                ReconcileStep::Continue => {}
                ReconcileStep::Wait(until) => now = until,
                ReconcileStep::Idle => return,
                ReconcileStep::Closed => panic!("unexpected closure"),
            }
        }
        panic!("scene feed did not settle");
    }

    fn settle_view(scene: &mut Reconciler, view: &mut render::scene::SceneView, world: &World) {
        let mut now = Instant::now();
        for _ in 0..10_000 {
            match scene.poll(now) {
                ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
                ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
                ReconcileStep::Idle => return,
                ReconcileStep::Wait(until) => now = until,
                ReconcileStep::Read(job) => {
                    let result = match job.query() {
                        ReadQuery::Barrier => Ok(ReadReply::Barrier(world.read_barrier())),
                        ReadQuery::Objects(cursor) => world.object_list_page(*cursor, 256).map(ReadReply::Objects),
                        ReadQuery::Regions(cursor) => world.region_page(*cursor, 64).map(ReadReply::Regions),
                        ReadQuery::Details(entities) => world.object_detail_batch(entities).map(ReadReply::Details),
                        ReadQuery::Changes(cursor) => world.change_page(*cursor, 256).map(ReadReply::Changes),
                        ReadQuery::Terrain(region, cursor) => world.terrain_page(*region, *cursor, 8).map(ReadReply::Terrain),
                        ReadQuery::Environment(region) => Ok(ReadReply::Environment(world.environment(*region))),
                    };
                    scene.complete_read(job, result.map_err(ReadFailure::World), now);
                }
                ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
                ReconcileStep::Closed => panic!("unexpected scene closure"),
            }
        }
        panic!("viewer environment did not settle");
    }

    fn push_experience(
        scene: &mut Reconciler,
        view: &mut render::scene::SceneView,
        world: &World,
        region: Region,
        experience: ExperienceId,
        transition: Duration,
        bytes: &[u8],
    ) {
        let asset = AssetId::new(experience.uuid());
        scene.push_environment(region, EnvironmentPush::Full { experience, asset, transition }).unwrap();
        let now = Instant::now();
        let request = loop {
            match scene.poll(now) {
                ReconcileStep::Asset(AssetChange::Request(request)) if request.request == assets::Request::Environment(asset) => {
                    break request;
                }
                ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
                ReconcileStep::Wait(until) => assert!(until <= now),
                ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
                ReconcileStep::Idle => panic!("experience request was not published"),
                ReconcileStep::Read(_) | ReconcileStep::Build(_) | ReconcileStep::Closed => panic!("unexpected experience work"),
            }
        };
        scene.complete_asset(request.ticket, ready_environment(asset, bytes));
        settle_view(scene, view, world);
    }

    fn ready_environment(asset: AssetId, bytes: &[u8]) -> assets::Status {
        let now = Instant::now();
        let mut store = assets::Store::new(assets::Budget::default());
        let route = store.register_route().unwrap();
        let demand = store.request(route, assets::Request::Environment(asset), assets::Priority::Visible, now).unwrap();
        let assets::LoadJob::Fetch(job) = store.next_job(now).unwrap() else { panic!("fresh environment request used cached bytes") };
        let input = job.complete(Ok(bytes.to_vec())).unwrap();
        let decode = store.start_decode(input).unwrap();
        store.complete(decode.run(), now);
        demand.status()
    }

    fn assignment(source: EnvironmentSource, altitudes: Option<[f32; 3]>) -> EnvironmentAssignment {
        EnvironmentAssignment { source, version: Some(1), day_length_seconds: None, day_offset_seconds: None, altitudes }
    }

    #[tokio::test]
    async fn feed_composes_the_agent_region_and_uses_global_avatar_altitude() {
        let mut world = world();
        let first_region = Region::new(1);
        let agent_region = Region::new(2);
        let _ = world.adopt_region(first_region).unwrap();
        let _ = world.patch_region(first_region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).unwrap();
        let _ = world.adopt_region(agent_region).unwrap();
        let _ =
            world.patch_region(agent_region, RegionInfo { origin: Some(DVec3::new(256.0, 0.0, 500.0)), ..RegionInfo::default() }).unwrap();
        for (region, bytes, altitudes) in [
            (first_region, b"{'type':'sky','sunlight_color':[r1,r0,r0]}".as_slice(), None),
            (
                agent_region,
                b"{'type':'daycycle','frames':{'w':{'type':'water'},'a':{'type':'sky','sunlight_color':[r1,r0,r0]},'b':{'type':'sky','sunlight_color':[r0,r1,r0]},'c':{'type':'sky','sunlight_color':[r1,r1,r0]},'d':{'type':'sky','sunlight_color':[r0,r0,r1]}},'tracks':[[{'key_keyframe':r0,'key_name':'w'}],[{'key_keyframe':r0,'key_name':'a'}],[{'key_keyframe':r0,'key_name':'b'}],[{'key_keyframe':r0,'key_name':'c'}],[{'key_keyframe':r0,'key_name':'d'}]]}".as_slice(),
                Some([100.0, 200.0, 300.0]),
            ),
        ] {
            let target = world.environment_target(region, world::EnvironmentScope::Region).unwrap();
            let assignment = assignment(EnvironmentSource::Inline(bytes.into()), altitudes);
            let _ = world
                .observe_environment(target, UpdateSource::new(region, 1, Ordered(1), 0), assignment)
                .unwrap();
        }
        let agent_id = Uuid::from_u128(100);
        let mut observation = ObjectObservation::new(
            UpdateSource::new(agent_region, 1, Ordered(2), 0),
            agent_id,
            NonZeroU32::new(1).unwrap(),
            Kind::Avatar,
            ParentUpdate::Root,
        );
        observation.pose = Some(Pose { position: Some(Vec3::new(0.0, 0.0, 10.0)), rotation: Some(Quat::IDENTITY) });
        observation.scale = Some(Vec3::ONE);
        let _ = world.observe_object(observation).unwrap();

        let budget = Budget::default();
        let (mut scene, view) = Reconciler::new(budget.clone());
        let (sender, incoming) = mpsc::channel(1);
        let (cancel, _) = oneshot::channel();
        let (camera, _) = watch::channel(None);
        let mut feed = SceneFeed {
            runtime: Handle::current(),
            view,
            budget,
            agent_id,
            agent: None,
            water_height: None,
            transition: Transition::default(),
            transition_identity: 0,
            composition: None,
            altitude: None,
            evaluated: None,
            next_environment: None,
            cloud_scroll: [0.0; 2],
            cloud_rate: [0.0; 2],
            cloud_time: None,
            camera,
            incoming,
            _cancel: cancel,
        };
        scene.notify_world_change();
        settle_feed(&mut scene, &mut feed, &sender, &world);
        let agent = feed.agent().expect("agent detail is published");
        assert_eq!(agent.detail().summary.address.region(), agent_region);
        assert_eq!(avatar_altitude(agent), 510.0);
        assert_ne!(composition_identity(&feed.view, first_region), composition_identity(&feed.view, agent_region));
        assert!(feed.update_environment(Instant::now()));
        let evaluated = feed.evaluated().expect("current agent region has an environment");
        assert_eq!(evaluated.sky_track, Some(4));
        assert_eq!(evaluated.sky.sunlight, [0.0, 0.0, 1.0]);
        let first = evaluate(&compose_sources(&feed.view, first_region, None), 0.0, 510.0);
        assert_eq!(first.sky.sunlight, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn experience_day_sky_and_water_keep_their_precedence() {
        let world = world();
        let region = Region::new(1);
        let (mut scene, mut view) = Reconciler::new(Budget::default());
        let day = ExperienceId::new(Uuid::from_u128(1)).unwrap();
        let sky = ExperienceId::new(Uuid::from_u128(2)).unwrap();
        let water = ExperienceId::new(Uuid::from_u128(3)).unwrap();
        let day_settings = b"{'type':'daycycle','frames':{'w':{'type':'water','water_fog_density':r2},'s':{'type':'sky','sunlight_color':[r0,r1,r0]}},'tracks':[[{'key_keyframe':r0,'key_name':'w'}],[{'key_keyframe':r0,'key_name':'s'}],[],[],[]]}";
        settle_view(&mut scene, &mut view, &world);
        push_experience(&mut scene, &mut view, &world, region, day, Duration::from_secs(1), day_settings);
        let sources = compose_sources(&view, region, view.viewer_environment());
        let day_output = evaluate(&sources, 0.0, 0.0);
        assert_eq!(day_output.sky.sunlight, [0.0, 1.0, 0.0]);
        assert_eq!(day_output.water.fog_density, 2.0);

        push_experience(&mut scene, &mut view, &world, region, sky, Duration::from_secs(2), b"{'type':'sky','sunlight_color':[r1,r0,r0]}");
        push_experience(&mut scene, &mut view, &world, region, water, Duration::from_secs(3), b"{'type':'water','water_fog_density':r7}");
        let viewer = view.viewer_environment().expect("experience settings are published");
        let sources = compose_sources(&view, region, Some(viewer));
        let output = evaluate(&sources, 0.0, 0.0);
        assert_eq!(output.sky.sunlight, [1.0, 0.0, 0.0]);
        assert_eq!(output.water.fog_density, 7.0);
        assert_eq!(viewer.transition_for_region(region), Some(Duration::from_secs(3)));
        assert!(sources.experience_day.is_some());
        assert!(sources.experience_sky.is_some());
        assert!(sources.experience_water.is_some());
        assert_ne!(composition_identity(&view, region).viewer_version, 0);
    }

    #[tokio::test]
    async fn water_height_follows_agent_region() {
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
        let budget = Budget::default();
        let (mut scene, view) = Reconciler::new(budget.clone());
        let (sender, incoming) = mpsc::channel(1);
        let (cancel, _) = oneshot::channel();
        let (camera, _) = watch::channel(None);
        let mut feed = SceneFeed {
            runtime: Handle::current(),
            view,
            budget,
            agent_id: agent,
            agent: None,
            water_height: None,
            transition: Transition::default(),
            transition_identity: 0,
            composition: None,
            altitude: None,
            evaluated: None,
            next_environment: None,
            cloud_scroll: [0.0; 2],
            cloud_rate: [0.0; 2],
            cloud_time: None,
            camera,
            movement: watch::channel(None).0,
            last_movement: None,
            incoming,
            _cancel: cancel,
        };
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
                    ReconcileStep::Publish(batch) => {
                        sender.try_send(batch).unwrap();
                        feed.drain().unwrap();
                    }
                    ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
                    ReconcileStep::Continue => {}
                    ReconcileStep::Idle => return (feed.water_height, feed.agent().map(|object| object.detail().summary.entity)),
                    _ => panic!("expected scene reconciliation"),
                }
            }
            panic!("scene did not settle");
        };
        let (height, original) = hydrate(&world);
        assert_eq!(height, None, "another region's water must not supply the water height");
        let original = original.expect("the observed avatar is available before its water height");
        let _ = world.observe_water_height(UpdateSource::new(Region::new(1), 2, Ordered(2), 0), 17.0).unwrap();
        assert_eq!(hydrate(&world), (Some(24.0), Some(original)));
        let _ = world.retire_region(Region::new(1)).unwrap();
        assert_eq!(hydrate(&world), (None, None), "retirement must remove both the avatar and previous water height");
        let _ = world
            .observe_object(ObjectObservation::new(
                UpdateSource::new(Region::new(2), 3, Ordered(3), 0),
                agent,
                NonZeroU32::new(1).unwrap(),
                Kind::Avatar,
                ParentUpdate::Root,
            ))
            .unwrap();
        let (height, replacement) = hydrate(&world);
        assert_eq!(height, Some(38.0), "the new avatar region supplies its own observed height");
        assert_ne!(replacement.expect("the replacement avatar is available"), original, "a retired entity must not remain cached");
    }

    #[tokio::test]
    async fn pending_asset_preserves_scene() {
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
        object.pose = Some(Pose { position: Some(Vec3::new(10.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
        object.scale = Some(Vec3::ONE);
        object.shape = FieldUpdate::Set(Shape::Prim(Default::default()));
        object.appearance = FieldUpdate::Set(Appearance::Surface);
        object.surface_appearance = Some(
            SurfaceAppearance::new(
                FaceAppearance { texture: Some(AssetId::new(Uuid::from_u128(100))), color: [255, 0, 0, 255], ..FaceAppearance::default() },
                Vec::new(),
            )
            .unwrap(),
        );
        let _ = world.observe_object(object).unwrap();
        let mut reads = WorldReadHost::new(&world, Handle::current());
        let assets = AssetCoordinator::new(Handle::current(), pending(), None);
        let (_pushes, environments) = crate::session::EnvironmentInbox::channel();
        let mut feed = SceneFeed::start(
            SceneSources { reader: reads.reader(), environments, assets: assets.handle() },
            Uuid::from_u128(2),
            watch::channel(None).0,
            watch::channel(None).0,
            Handle::current(),
            || {},
        );
        feed.set_camera(Some(Camera::new(DVec3::ZERO, Vec3::X, 1.0, [1000, 1000]).unwrap()));
        let receiving = async {
            loop {
                let batch = feed.incoming.recv().await.expect("scene remains active");
                feed.view.apply(batch).unwrap();
                if let Some(face) = feed.view.objects().next().and_then(|object| object.materials()).and_then(|faces| faces.first())
                    && let Some(binding) = &face.base_color_texture
                    && binding.state().resolution == render::material::Resolution::Pending
                {
                    assert_eq!(face.base_color_factor(), [1.0, 0.0, 0.0, 1.0]);
                    assert!(binding.image().is_none());
                    assert_eq!(face.alpha, render::material::AlphaBehavior::Unknown);
                    break;
                }
            }
        }
        .fuse();
        let serving = async {
            loop {
                let Err(request) = select_read(&mut reads, pending::<()>()).await else { unreachable!() };
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
        let assets = AssetCoordinator::new(Handle::current(), pending(), None);
        let (pushes, environments) = crate::session::EnvironmentInbox::channel();
        let mut feed = SceneFeed::start(
            SceneSources { reader: reader.clone(), environments, assets: assets.handle() },
            Uuid::from_u128(1),
            watch::channel(None).0,
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
                    let Err(request) = select_read(&mut service, pending::<()>()).await else { unreachable!() };
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
                match select_read(&mut service, ready(())).await {
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
        let push = EnvironmentPush::Partial {
            experience,
            sky: vec![EnvironmentProperty { name: "gamma".into(), source: b"r1.4".as_slice().into() }],
            water: Vec::new(),
            transition: Duration::ZERO,
        };
        assert!(pushes.try_send(crate::session::EnvironmentCommand { region, push }).is_ok());
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
                    let Err(request) = select_read(&mut service, pending::<()>()).await else { unreachable!() };
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
}
