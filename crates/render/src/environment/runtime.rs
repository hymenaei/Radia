use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use assets::{EnvironmentDescription, EnvironmentSettings};
use world::Region;

use super::{Evaluated, Local, Schedule, SkyState, Sources, TextureBlend, Transition, WaterState, evaluate, star_brightness};
use crate::scene::{LocalChoice, LocalEnvironment, SceneView, ViewerEnvironment};

const DEFAULT_TRANSITION: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CompositionIdentity {
    region: Region,
    environment: Option<u64>,
    viewer_version: u64,
}

#[derive(Clone, Debug, PartialEq)]
enum ActiveLocal {
    Sky { choice: LocalChoice, water: WaterState },
    Water { choice: LocalChoice, sky: SkyState },
    DayCycle { choice: LocalChoice },
}

impl ActiveLocal {
    fn choice(&self) -> &LocalChoice {
        match self {
            Self::Sky { choice, .. } | Self::Water { choice, .. } | Self::DayCycle { choice } => choice,
        }
    }

    fn matches(&self, local: &LocalEnvironment) -> bool {
        self.choice() == local.choice()
            && matches!(
                (self, &local.environment().settings),
                (Self::Sky { .. }, EnvironmentSettings::Sky(_))
                    | (Self::Water { .. }, EnvironmentSettings::Water(_))
                    | (Self::DayCycle { .. }, EnvironmentSettings::DayCycle(_))
            )
    }
}

#[derive(Default)]
pub struct ActiveEnvironment {
    transition: Transition,
    transition_identity: u64,
    composition: Option<CompositionIdentity>,
    altitude: Option<f64>,
    evaluated: Option<Evaluated>,
    next_deadline: Option<Instant>,
    cloud_scroll: [f64; 2],
    cloud_rate: [f32; 2],
    cloud_time: Option<SystemTime>,
    animation_origin: Option<Instant>,
    animation_time: f32,
    water_scroll: [f32; 4],
    local: Option<ActiveLocal>,
}

impl ActiveEnvironment {
    pub fn update(&mut self, view: &SceneView, agent: Option<(Region, f64)>, wall_time: SystemTime, now: Instant) -> bool {
        self.advance_cloud_scroll(wall_time);
        let animation_origin = *self.animation_origin.get_or_insert(now);
        self.animation_time = now.duration_since(animation_origin).as_secs_f32();
        let Some((region, altitude)) = agent.filter(|(_, altitude)| altitude.is_finite()) else {
            return self.clear_environment();
        };
        let viewer = view.viewer_environment();
        let local = viewer.and_then(|viewer| viewer.local());
        let local_changed = match (&self.local, local) {
            (None, None) => false,
            (Some(active), Some(local)) => !active.matches(local),
            _ => true,
        };
        let leaving_local = local_changed && local.is_none();
        let identity = composition_identity(view, region);
        let changed = self.composition != Some(identity) || self.altitude != Some(altitude);
        let deadline = self.next_deadline.is_some_and(|deadline| now >= deadline);
        if !changed && !local_changed && !deadline {
            self.water_scroll = [self.animation_time * 0.5, 0.0, 0.0, 0.0];
            return false;
        }
        if self.composition != Some(identity) {
            self.transition_identity = self.transition_identity.checked_add(1).expect("environment transition identities exhausted");
            self.composition = Some(identity);
        }
        self.altitude = Some(altitude);
        let utc = wall_time.duration_since(UNIX_EPOCH).map_or(0.0, |duration| duration.as_secs_f64());
        if local_changed {
            self.local = local.map(|local| {
                let choice = local.choice().clone();
                let shared = self.evaluated.as_ref().cloned().unwrap_or_else(|| {
                    let sources = compose_sources(view, region, viewer, None);
                    evaluate(&sources, utc, altitude)
                });
                match &local.environment().settings {
                    EnvironmentSettings::Sky(_) => ActiveLocal::Sky { choice, water: shared.water },
                    EnvironmentSettings::Water(_) => ActiveLocal::Water { choice, sky: shared.sky },
                    EnvironmentSettings::DayCycle(_) => ActiveLocal::DayCycle { choice },
                }
            });
        }
        let sources = compose_sources(view, region, viewer, self.local.as_ref());
        let has_environment = sources.base.is_some()
            || sources.experience_day.is_some()
            || sources.experience_sky.is_some()
            || sources.experience_water.is_some()
            || sources.sky_override.is_some()
            || sources.water_override.is_some()
            || sources.local.is_some();
        if !has_environment {
            let changed = self.clear_environment();
            self.composition = Some(identity);
            self.altitude = Some(altitude);
            return changed;
        }
        let target = evaluate(&sources, utc, altitude);
        let duration = match self.local.as_ref().map(ActiveLocal::choice) {
            Some(LocalChoice::Preset(_)) => Duration::ZERO,
            Some(LocalChoice::Inventory { .. }) => DEFAULT_TRANSITION,
            None if leaving_local => DEFAULT_TRANSITION,
            None => viewer.and_then(|viewer| viewer.transition_for_region(region)).unwrap_or(DEFAULT_TRANSITION),
        };
        let (evaluated, next) = self.transition.update(self.transition_identity, target, duration, now);
        let output_changed = self.evaluated.as_ref() != Some(&evaluated);
        self.cloud_rate = evaluated.sky.clouds.scroll_rate;
        self.water_scroll = [self.animation_time * 0.5, 0.0, 0.0, 0.0];
        self.evaluated = Some(evaluated);
        self.next_deadline = next;
        output_changed
    }

    pub fn evaluated(&self) -> Option<&Evaluated> {
        self.evaluated.as_ref()
    }

    pub fn cloud_scroll(&self) -> [f32; 2] {
        [self.cloud_scroll[0] as f32, self.cloud_scroll[1] as f32]
    }

    pub fn animation_time(&self) -> f32 {
        self.animation_time
    }

    pub fn water_scroll(&self) -> [f32; 4] {
        self.water_scroll
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.next_deadline
    }

    pub fn clouds_animate(&self, view: &SceneView) -> bool {
        self.evaluated.as_ref().is_some_and(|evaluated| {
            evaluated.sky.clouds.scale >= 0.001
                && texture_available(view, evaluated.sky.clouds.texture)
                && self.cloud_rate.iter().any(|rate| *rate != 0.0)
        })
    }

    pub fn stars_animate(&self, view: &SceneView) -> bool {
        self.evaluated.as_ref().is_some_and(|evaluated| {
            star_brightness(evaluated.sky.star_brightness) >= 0.001 && texture_available(view, evaluated.sky.bloom)
        })
    }

    pub fn water_animate(&self, view: &SceneView) -> bool {
        self.evaluated.as_ref().is_some_and(|evaluated| {
            texture_available(view, evaluated.water.normal_map)
                && evaluated.water.normal_scale.iter().any(|scale| *scale != 0.0)
                && evaluated.water.waves.iter().flatten().any(|component| *component != 0.0)
        })
    }

    fn advance_cloud_scroll(&mut self, now: SystemTime) {
        if let Some(previous) = self.cloud_time
            && let Ok(delta) = now.duration_since(previous)
        {
            let seconds = delta.as_secs_f64();
            for (offset, rate) in self.cloud_scroll.iter_mut().zip(self.cloud_rate) {
                *offset = if rate == 0.0 { 0.0 } else { (*offset + f64::from(rate) * seconds / 100.0).rem_euclid(1.0) };
            }
        }
        self.cloud_time = Some(now);
    }

    fn clear_environment(&mut self) -> bool {
        let changed = self.evaluated.take().is_some();
        self.transition = Transition::default();
        self.composition = None;
        self.altitude = None;
        self.next_deadline = None;
        self.cloud_scroll = [0.0; 2];
        self.cloud_rate = [0.0; 2];
        self.water_scroll = [0.0; 4];
        self.local = None;
        changed
    }
}

fn texture_available(view: &SceneView, texture: TextureBlend) -> bool {
    [texture.current, texture.next].into_iter().flatten().any(|id| view.environment_texture(id).is_some())
}

struct ExperienceSources<'a> {
    day: Option<&'a EnvironmentDescription>,
    sky: Option<&'a EnvironmentDescription>,
    water: Option<&'a EnvironmentDescription>,
}

fn experience_sources(viewer: Option<&ViewerEnvironment>, region: Region) -> ExperienceSources<'_> {
    let mut sources = ExperienceSources { day: None, sky: None, water: None };
    let Some(viewer) = viewer else { return sources };
    for setting in [viewer.day(), viewer.sky(), viewer.water()].into_iter().flatten() {
        if setting.region() != region {
            continue;
        }
        let target = match &setting.environment().settings {
            EnvironmentSettings::DayCycle(_) => &mut sources.day,
            EnvironmentSettings::Sky(_) => &mut sources.sky,
            EnvironmentSettings::Water(_) => &mut sources.water,
        };
        *target = Some(setting.environment());
    }
    sources
}

fn compose_sources<'a>(
    view: &'a SceneView,
    region: Region,
    viewer: Option<&'a ViewerEnvironment>,
    active_local: Option<&'a ActiveLocal>,
) -> Sources<'a> {
    let base = view.environment(region);
    let experience = experience_sources(viewer, region);
    let mut schedule = base.map_or_else(Schedule::default, |environment| environment.schedule());
    let local = viewer.and_then(|viewer| viewer.local());
    if matches!(active_local, Some(ActiveLocal::DayCycle { .. })) {
        let default = Schedule::default();
        schedule = Schedule { altitudes: schedule.altitudes, ..default };
    }
    let local = match (local, active_local) {
        (Some(local), Some(ActiveLocal::Sky { water, .. })) => match &local.environment().settings {
            EnvironmentSettings::Sky(sky) => Some(Local::Sky { sky, water }),
            _ => None,
        },
        (Some(local), Some(ActiveLocal::Water { sky, .. })) => match &local.environment().settings {
            EnvironmentSettings::Water(water) => Some(Local::Water { water, sky }),
            _ => None,
        },
        (Some(local), Some(ActiveLocal::DayCycle { .. })) => match &local.environment().settings {
            EnvironmentSettings::DayCycle(day) => Some(Local::DayCycle(day)),
            _ => None,
        },
        _ => None,
    };
    Sources {
        base: base.map(|environment| environment.description()),
        schedule,
        experience_day: experience.day,
        experience_sky: experience.sky,
        experience_water: experience.water,
        sky_override: viewer.and_then(|viewer| viewer.sky_overrides()),
        water_override: viewer.and_then(|viewer| viewer.water_overrides()),
        local,
    }
}

fn composition_identity(view: &SceneView, region: Region) -> CompositionIdentity {
    CompositionIdentity {
        region,
        environment: view.environment(region).map(|environment| environment.identity()),
        viewer_version: view.viewer_environment().map_or(0, |viewer| viewer.version()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use math::{DVec3, Quat, Vec3};
    use utils::{AssetId, ExperienceId};
    use uuid::Uuid;
    use world::{
        EnvironmentAssignment, EnvironmentScope, EnvironmentSource, Kind, ObjectObservation, OrderEvidence::Ordered, ParcelLocalId,
        ParentUpdate, Pose, RegionInfo, UpdateSource, World, WorldLimits,
    };

    use super::*;
    use crate::scene::{
        AssetChange, Budget, EnvironmentProperty, EnvironmentPush, ReadFailure, ReadQuery, ReadReply, ReconcileStep, Reconciler,
    };

    fn world() -> World {
        World::new(WorldLimits {
            max_region_registrations: 2,
            max_entities: 8,
            max_entities_per_region: 8,
            max_tombstones_per_region: 8,
            max_tombstones: 8,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(8),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(8),
            max_journal_entries: 16,
            max_journal_changes: 64,
            max_region_name_bytes: 64,
            max_recovery_targets: 4,
        })
    }

    fn settle(scene: &mut Reconciler, view: &mut SceneView, world: &World) {
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
        panic!("environment scene did not settle");
    }

    fn add_region_environment(world: &mut World, region: Region, bytes: &[u8], altitudes: Option<[f32; 3]>) {
        let _ = world.adopt_region(region).unwrap();
        let origin = DVec3::new((region.get() - 1) as f64 * 256.0, 0.0, if region.get() == 2 { 500.0 } else { 0.0 });
        let _ = world.patch_region(region, RegionInfo { origin: Some(origin), ..RegionInfo::default() }).unwrap();
        let target = world.environment_target(region, world::EnvironmentScope::Region).unwrap();
        let assignment = EnvironmentAssignment {
            source: EnvironmentSource::Inline(bytes.into()),
            version: Some(1),
            day_length_seconds: None,
            day_offset_seconds: None,
            altitudes,
        };
        let _ = world.observe_environment(target, UpdateSource::new(region, 1, Ordered(1), 0), assignment).unwrap();
    }

    fn set_parcel_environment(world: &mut World, region: Region, bytes: &[u8]) {
        let parcel = ParcelLocalId::new(7).unwrap();
        let _ = world.observe_agent_parcel(UpdateSource::new(region, 2, Ordered(2), 0), Some(parcel)).unwrap();
        let target = world.environment_target(region, EnvironmentScope::Parcel(parcel)).unwrap();
        let assignment = EnvironmentAssignment {
            source: EnvironmentSource::Inline(bytes.into()),
            version: Some(1),
            day_length_seconds: None,
            day_offset_seconds: None,
            altitudes: None,
        };
        let _ = world.observe_environment(target, UpdateSource::new(region, 2, Ordered(3), 0), assignment).unwrap();
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

    fn push_experience(
        scene: &mut Reconciler,
        view: &mut SceneView,
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
        settle(scene, view, world);
    }

    fn push_local(
        scene: &mut Reconciler,
        view: &mut SceneView,
        world: &World,
        region: Region,
        preset: crate::scene::EnvironmentPreset,
        bytes: &[u8],
    ) {
        let asset = preset.asset_id().expect("local sky preset has an asset");
        push_local_asset(scene, view, world, region, crate::scene::EnvironmentChoice::Local(LocalChoice::Preset(preset)), asset, bytes);
    }

    fn push_local_asset(
        scene: &mut Reconciler,
        view: &mut SceneView,
        world: &World,
        region: Region,
        choice: crate::scene::EnvironmentChoice,
        asset: AssetId,
        bytes: &[u8],
    ) {
        scene.set_local_environment(choice, region).unwrap();
        let now = Instant::now();
        let request = loop {
            match scene.poll(now) {
                ReconcileStep::Asset(AssetChange::Request(request)) if request.request == assets::Request::Environment(asset) => {
                    break request;
                }
                ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
                ReconcileStep::Wait(until) => assert!(until <= now),
                ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
                ReconcileStep::Idle => panic!("local preset request was not published"),
                ReconcileStep::Read(_) | ReconcileStep::Build(_) | ReconcileStep::Closed => panic!("unexpected local preset work"),
            }
        };
        scene.complete_asset(request.ticket, ready_environment(asset, bytes));
        settle(scene, view, world);
    }

    #[test]
    fn active_environment_uses_agent_region_and_restarts_identity_on_region_change() {
        let mut world = world();
        let first_region = Region::new(1);
        let second_region = Region::new(2);
        add_region_environment(&mut world, first_region, b"{'type':'sky','sunlight_color':[r1,r0,r0]}", None);
        add_region_environment(
            &mut world,
            second_region,
            b"{'type':'daycycle','frames':{'w':{'type':'water'},'a':{'type':'sky','sunlight_color':[r1,r0,r0]},'b':{'type':'sky','sunlight_color':[r0,r1,r0]},'c':{'type':'sky','sunlight_color':[r1,r1,r0]},'d':{'type':'sky','sunlight_color':[r0,r0,r1]}},'tracks':[[{'key_keyframe':r0,'key_name':'w'}],[{'key_keyframe':r0,'key_name':'a'}],[{'key_keyframe':r0,'key_name':'b'}],[{'key_keyframe':r0,'key_name':'c'}],[{'key_keyframe':r0,'key_name':'d'}]]}",
            Some([100.0, 200.0, 300.0]),
        );
        let mut agent = ObjectObservation::new(
            UpdateSource::new(second_region, 1, Ordered(2), 0),
            Uuid::from_u128(100),
            std::num::NonZeroU32::new(1).unwrap(),
            Kind::Avatar,
            ParentUpdate::Root,
        );
        agent.pose = Some(Pose { position: Some(Vec3::new(0.0, 0.0, 10.0)), rotation: Some(Quat::IDENTITY) });
        agent.scale = Some(Vec3::ONE);
        let _ = world.observe_object(agent).unwrap();
        assert!(world.environment(first_region).is_some());
        assert!(world.environment(second_region).is_some());
        let (mut scene, mut view) = Reconciler::new(Budget::default());
        scene.notify_world_change();
        settle(&mut scene, &mut view, &world);
        assert!(view.environment(first_region).is_some());
        assert!(
            view.environment(second_region).is_some(),
            "regions={} status={:?} origins={:?}",
            view.region_count(),
            view.status(),
            view.regions().map(|region| region.summary().info.resolved_origin()).collect::<Vec<_>>()
        );
        let mut active = ActiveEnvironment::default();
        let now = Instant::now();
        let wall_time = UNIX_EPOCH + Duration::from_secs(1);
        assert!(active.update(&view, Some((first_region, 510.0)), wall_time, now));
        assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([1.0, 0.0, 0.0]));
        let first_identity = active.composition;
        active.update(&view, Some((second_region, 510.0)), wall_time, now + Duration::from_millis(1));
        assert_ne!(active.composition, first_identity);
        assert!(active.next_deadline().is_some());
        active.update(&view, Some((second_region, 510.0)), wall_time, now + Duration::from_secs(6));
        let evaluated = active.evaluated().expect("second region has an environment");
        assert_eq!(evaluated.sky_track, Some(4));
        assert_eq!(evaluated.sky.sunlight, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn experience_settings_keep_day_sky_and_water_precedence() {
        let world = world();
        let region = Region::new(1);
        let (mut scene, mut view) = Reconciler::new(Budget::default());
        settle(&mut scene, &mut view, &world);
        let geometry_version = view.geometry_version();
        let day = ExperienceId::new(Uuid::from_u128(1)).unwrap();
        let sky = ExperienceId::new(Uuid::from_u128(2)).unwrap();
        let water = ExperienceId::new(Uuid::from_u128(3)).unwrap();
        let day_settings = b"{'type':'daycycle','frames':{'w':{'type':'water','water_fog_density':r2},'s':{'type':'sky','sunlight_color':[r0,r1,r0]}},'tracks':[[{'key_keyframe':r0,'key_name':'w'}],[{'key_keyframe':r0,'key_name':'s'}],[],[],[]]}";
        push_experience(&mut scene, &mut view, &world, region, day, Duration::from_secs(1), day_settings);
        push_experience(&mut scene, &mut view, &world, region, sky, Duration::from_secs(2), b"{'type':'sky','sunlight_color':[r1,r0,r0]}");
        push_experience(&mut scene, &mut view, &world, region, water, Duration::from_secs(3), b"{'type':'water','water_fog_density':r7}");
        let mut active = ActiveEnvironment::default();
        let now = Instant::now();
        assert!(active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now));
        let evaluated = active.evaluated().expect("experience settings are published");
        assert_eq!(evaluated.sky.sunlight, [1.0, 0.0, 0.0]);
        assert_eq!(evaluated.water.fog_density, 7.0);
        assert_eq!(view.viewer_environment().and_then(|viewer| viewer.transition_for_region(region)), Some(Duration::from_secs(3)));
        assert_ne!(active.composition.map(|identity| identity.viewer_version), Some(0));
        push_experience(&mut scene, &mut view, &world, region, sky, Duration::from_secs(2), b"{'type':'sky','sunlight_color':[r0,r0,r1]}");
        active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(1));
        active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_secs(1));
        assert_ne!(active.evaluated().map(|value| value.sky.sunlight), Some([1.0, 0.0, 0.0]));
        assert_eq!(
            view.geometry_version(),
            geometry_version,
            "experience and evaluated environment updates do not invalidate geometry preparation"
        );
    }

    #[test]
    fn local_preset_beats_shared_and_experience_and_clear_restores_experience() {
        let mut world = world();
        let region = Region::new(1);
        let second_region = Region::new(2);
        add_region_environment(&mut world, region, b"{'type':'sky','sunlight_color':[r0,r1,r0]}", None);
        add_region_environment(&mut world, second_region, b"{'type':'sky','sunlight_color':[r1,r0,r1]}", None);
        set_parcel_environment(&mut world, region, b"{'type':'sky','sunlight_color':[r1,r1,r0]}");
        let (mut scene, mut view) = Reconciler::new(Budget::default());
        settle(&mut scene, &mut view, &world);
        let parcel_sources = compose_sources(&view, region, view.viewer_environment(), None);
        assert_eq!(evaluate(&parcel_sources, 0.0, 0.0).sky.sunlight, [1.0, 1.0, 0.0]);
        let experience = ExperienceId::new(Uuid::from_u128(4)).unwrap();
        push_experience(
            &mut scene,
            &mut view,
            &world,
            region,
            experience,
            Duration::from_secs(2),
            b"{'type':'sky','sunlight_color':[r1,r0,r0]}",
        );
        let override_property = EnvironmentProperty { name: "sunlight_color".into(), source: b"[r0,r1,r0]".as_slice().into() };
        scene
            .push_environment(
                region,
                EnvironmentPush::Partial { experience, sky: vec![override_property], water: Vec::new(), transition: Duration::ZERO },
            )
            .unwrap();
        settle(&mut scene, &mut view, &world);
        let water_experience = ExperienceId::new(Uuid::from_u128(5)).unwrap();
        push_experience(
            &mut scene,
            &mut view,
            &world,
            region,
            water_experience,
            Duration::from_secs(2),
            b"{'type':'water','water_fog_density':r7}",
        );
        let now = Instant::now();
        let mut active = ActiveEnvironment::default();
        assert!(active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now));
        assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 1.0, 0.0]));
        push_local(
            &mut scene,
            &mut view,
            &world,
            region,
            crate::scene::EnvironmentPreset::Midday,
            b"{'type':'sky','sunlight_color':[r0,r0,r1]}",
        );
        assert!(active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(1)));
        assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 0.0, 1.0]));
        assert_eq!(active.evaluated().map(|value| value.water.fog_density), Some(7.0));
        assert_eq!(active.next_deadline(), None);
        assert_eq!(
            view.viewer_environment().map(|environment| environment.selected()),
            Some(&crate::scene::EnvironmentChoice::Local(crate::scene::LocalChoice::Preset(crate::scene::EnvironmentPreset::Midday)))
        );

        let _ = world.observe_agent_parcel(UpdateSource::new(region, 2, Ordered(4), 0), None).unwrap();
        scene.notify_world_change();
        settle(&mut scene, &mut view, &world);
        assert!(!active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(2)));
        assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 0.0, 1.0]));
        assert!(!active.update(&view, Some((second_region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(3)));
        assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 0.0, 1.0]));
        assert_eq!(active.evaluated().map(|value| value.water.fog_density), Some(7.0));
        assert!(!active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(4)));

        scene.set_local_environment(crate::scene::EnvironmentChoice::Shared, region).unwrap();
        settle(&mut scene, &mut view, &world);
        assert!(active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(5)));
        assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 0.0, 1.0]));
        active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_secs(6));
        assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 1.0, 0.0]));
        assert_eq!(active.evaluated().map(|value| value.water.fog_density), Some(7.0));
        assert_eq!(view.viewer_environment().map(|environment| environment.selected()), Some(&crate::scene::EnvironmentChoice::Shared));
    }

    #[test]
    fn completed_local_preset_uses_agent_region_and_updates_active_sky() {
        let mut world = world();
        let first_region = Region::new(1);
        let agent_region = Region::new(2);
        add_region_environment(&mut world, first_region, b"{'type':'sky','sunlight_color':[r1,r0,r0]}", None);
        add_region_environment(&mut world, agent_region, b"{'type':'sky','sunlight_color':[r0,r1,r0]}", None);
        let (mut scene, mut view) = Reconciler::new(Budget::default());
        settle(&mut scene, &mut view, &world);
        let mut active = ActiveEnvironment::default();
        let now = Instant::now();
        assert!(active.update(&view, Some((agent_region, 0.0)), UNIX_EPOCH, now));
        assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 1.0, 0.0]));

        scene
            .set_local_environment(
                crate::scene::EnvironmentChoice::Local(crate::scene::LocalChoice::Preset(crate::scene::EnvironmentPreset::Midday)),
                agent_region,
            )
            .unwrap();
        let request = loop {
            match scene.poll(now) {
                ReconcileStep::Asset(AssetChange::Request(request)) => break request,
                ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
                ReconcileStep::Wait(until) => assert!(until <= now),
                ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
                ReconcileStep::Idle => panic!("local preset request was not published"),
                ReconcileStep::Read(_) | ReconcileStep::Build(_) | ReconcileStep::Closed => panic!("unexpected local preset work"),
            }
        };
        assert_eq!(request.region, agent_region);
        settle(&mut scene, &mut view, &world);
        active.update(&view, Some((agent_region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(1));
        assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 1.0, 0.0]));

        scene.complete_asset(
            request.ticket,
            ready_environment(
                crate::scene::EnvironmentPreset::Midday.asset_id().expect("known sky asset"),
                b"{'type':'sky','sunlight_color':[r0,r0,r1]}",
            ),
        );
        settle(&mut scene, &mut view, &world);
        assert!(active.update(&view, Some((agent_region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(2)));
        assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 0.0, 1.0]));
    }

    #[test]
    fn crossing_retries_local_asset_in_agent_region() {
        let mut world = world();
        let first_region = Region::new(1);
        let agent_region = Region::new(2);
        add_region_environment(&mut world, first_region, b"{'type':'sky'}", None);
        add_region_environment(&mut world, agent_region, b"{'type':'sky'}", None);
        let (mut scene, mut view) = Reconciler::new(Budget::default());
        settle(&mut scene, &mut view, &world);
        let choice = crate::scene::EnvironmentChoice::Local(crate::scene::LocalChoice::Preset(crate::scene::EnvironmentPreset::Midday));
        let asset = crate::scene::EnvironmentPreset::Midday.asset_id().expect("known sky asset");
        let now = Instant::now();
        scene.set_local_environment(choice.clone(), first_region).unwrap();
        let first = loop {
            match scene.poll(now) {
                ReconcileStep::Asset(AssetChange::Request(request)) if request.request == assets::Request::Environment(asset) => {
                    break request;
                }
                ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
                ReconcileStep::Wait(until) => assert!(until <= now),
                ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
                ReconcileStep::Idle => panic!("first local request was not published"),
                ReconcileStep::Read(_) | ReconcileStep::Build(_) | ReconcileStep::Closed => panic!("unexpected scene work"),
            }
        };
        assert_eq!(first.region, first_region);

        scene.set_local_environment(choice, agent_region).unwrap();
        assert!(matches!(scene.poll(now), ReconcileStep::Asset(AssetChange::Release(ticket)) if ticket == first.ticket));
        let retry = loop {
            match scene.poll(now) {
                ReconcileStep::Asset(AssetChange::Request(request)) if request.request == assets::Request::Environment(asset) => {
                    break request;
                }
                ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
                ReconcileStep::Wait(until) => assert!(until <= now),
                ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
                ReconcileStep::Idle => panic!("local request was not retried after the crossing"),
                ReconcileStep::Read(_) | ReconcileStep::Build(_) | ReconcileStep::Closed => panic!("unexpected scene work"),
            }
        };
        assert_eq!(retry.region, agent_region);
        assert_ne!(retry.ticket, first.ticket);
        scene.complete_asset(first.ticket, ready_environment(asset, b"{'type':'sky','sunlight_color':[r1,r0,r0]}"));
        scene.complete_asset(retry.ticket, ready_environment(asset, b"{'type':'sky','sunlight_color':[r0,r0,r1]}"));
        settle(&mut scene, &mut view, &world);
        let mut active = ActiveEnvironment::default();
        assert!(active.update(&view, Some((agent_region, 0.0)), UNIX_EPOCH, now));
        assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 0.0, 1.0]));
    }

    #[test]
    fn inventory_water_preserves_local_sky_and_daycycle_replaces_both() {
        let mut world = world();
        let region = Region::new(1);
        add_region_environment(
            &mut world,
            region,
            b"{'type':'daycycle','frames':{'water':{'type':'water','water_fog_density':r3},'sky':{'type':'sky','sunlight_color':[r1,r0,r0]}},'tracks':[[{'key_keyframe':r0,'key_name':'water'}],[{'key_keyframe':r0,'key_name':'sky'}],[],[],[]]}",
            None,
        );
        let (mut scene, mut view) = Reconciler::new(Budget::default());
        settle(&mut scene, &mut view, &world);
        let now = Instant::now();
        let mut active = ActiveEnvironment::default();
        assert!(active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now));
        let initial_water = active.evaluated().expect("shared environment").water.clone();

        let sky_asset = AssetId::new(Uuid::from_u128(20));
        push_local_asset(
            &mut scene,
            &mut view,
            &world,
            region,
            crate::scene::EnvironmentChoice::Local(crate::scene::LocalChoice::Inventory {
                owner_id: Uuid::from_u128(1),
                item_id: Uuid::from_u128(21),
                asset_id: sky_asset,
            }),
            sky_asset,
            b"{'type':'sky','sunlight_color':[r0,r0,r1]}",
        );
        active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(1));
        assert!(active.next_deadline().is_some());
        active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_secs(6));
        let local_sky = active.evaluated().expect("local sky").sky.clone();
        assert_eq!(local_sky.sunlight, [0.0, 0.0, 1.0]);
        assert_eq!(active.evaluated().map(|value| &value.water), Some(&initial_water));

        let water_asset = AssetId::new(Uuid::from_u128(22));
        push_local_asset(
            &mut scene,
            &mut view,
            &world,
            region,
            crate::scene::EnvironmentChoice::Local(crate::scene::LocalChoice::Inventory {
                owner_id: Uuid::from_u128(1),
                item_id: Uuid::from_u128(23),
                asset_id: water_asset,
            }),
            water_asset,
            b"{'type':'water','water_fog_density':r9}",
        );
        active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_secs(6) + Duration::from_millis(1));
        assert!(active.next_deadline().is_some());
        active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_secs(12));
        assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 0.0, 1.0]));
        assert_eq!(active.evaluated().map(|value| value.water.fog_density), Some(9.0));

        let day_asset = AssetId::new(Uuid::from_u128(24));
        push_local_asset(
            &mut scene,
            &mut view,
            &world,
            region,
            crate::scene::EnvironmentChoice::Local(crate::scene::LocalChoice::Inventory {
                owner_id: Uuid::from_u128(1),
                item_id: Uuid::from_u128(25),
                asset_id: day_asset,
            }),
            day_asset,
            b"{'type':'daycycle','frames':{'water':{'type':'water','water_fog_density':r12},'sky':{'type':'sky','sunlight_color':[r0,r1,r0]}},'tracks':[[{'key_keyframe':r0,'key_name':'water'}],[{'key_keyframe':r0,'key_name':'sky'}],[],[],[]]}",
        );
        active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_secs(12) + Duration::from_millis(1));
        assert!(active.next_deadline().is_some());
        active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_secs(18));
        let daycycle = active.evaluated().expect("local day cycle");
        assert_eq!(daycycle.sky.sunlight, [0.0, 1.0, 0.0]);
        assert_eq!(daycycle.water.fog_density, 12.0);
    }

    #[test]
    fn closing_scene_releases_local_asset_request() {
        let mut world = world();
        let region = Region::new(1);
        add_region_environment(&mut world, region, b"{'type':'sky'}", None);
        let (mut scene, mut view) = Reconciler::new(Budget::default());
        settle(&mut scene, &mut view, &world);
        scene
            .set_local_environment(
                crate::scene::EnvironmentChoice::Local(crate::scene::LocalChoice::Preset(crate::scene::EnvironmentPreset::Midday)),
                region,
            )
            .unwrap();
        let now = Instant::now();
        let request = loop {
            match scene.poll(now) {
                ReconcileStep::Asset(AssetChange::Request(request))
                    if request.request
                        == assets::Request::Environment(crate::scene::EnvironmentPreset::Midday.asset_id().expect("preset asset")) =>
                {
                    break request;
                }
                ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
                ReconcileStep::Wait(until) => assert!(until <= now),
                ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
                ReconcileStep::Idle => panic!("local preset request was not published"),
                ReconcileStep::Read(_) | ReconcileStep::Build(_) | ReconcileStep::Closed => panic!("unexpected scene work"),
            }
        };
        scene.close();
        let released = loop {
            match scene.poll(now) {
                ReconcileStep::Asset(AssetChange::Release(ticket)) if ticket == request.ticket => break true,
                ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
                ReconcileStep::Closed => break false,
                ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
                ReconcileStep::Wait(_) | ReconcileStep::Idle | ReconcileStep::Read(_) | ReconcileStep::Build(_) => {
                    panic!("closed scene did not release the local preset request")
                }
            }
        };
        assert!(released, "scene closure releases the outstanding preset ticket before returning Closed");
    }

    #[test]
    fn failed_local_preset_publishes_a_typed_error() {
        let mut world = world();
        let region = Region::new(1);
        add_region_environment(&mut world, region, b"{'type':'sky'}", None);
        let (mut scene, mut view) = Reconciler::new(Budget::default());
        settle(&mut scene, &mut view, &world);
        scene
            .set_local_environment(
                crate::scene::EnvironmentChoice::Local(crate::scene::LocalChoice::Preset(crate::scene::EnvironmentPreset::Midday)),
                region,
            )
            .unwrap();
        let now = Instant::now();
        let request = loop {
            match scene.poll(now) {
                ReconcileStep::Asset(AssetChange::Request(request)) => break request,
                ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
                ReconcileStep::Wait(until) => assert!(until <= now),
                ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
                ReconcileStep::Idle => panic!("local preset request was not published"),
                ReconcileStep::Read(_) | ReconcileStep::Build(_) | ReconcileStep::Closed => panic!("unexpected scene work"),
            }
        };
        scene.complete_asset(request.ticket, assets::Status::Failed { failure: assets::Failure::NotFound, retry_at: None });
        settle(&mut scene, &mut view, &world);

        let environment = view.viewer_environment().expect("failure outcome is published");
        assert_eq!(environment.selected(), &crate::scene::EnvironmentChoice::default());
        assert_eq!(environment.local_error(), Some(assets::Failure::NotFound));
        assert!(environment.local().is_none());
    }

    #[test]
    fn cloud_scroll_uses_wall_time_and_rate_per_hundred_seconds() {
        let mut world = world();
        let region = Region::new(1);
        add_region_environment(&mut world, region, b"{'type':'sky'}", None);
        let (mut scene, mut view) = Reconciler::new(Budget::default());
        settle(&mut scene, &mut view, &world);
        let mut active = ActiveEnvironment::default();
        let now = Instant::now();
        assert!(active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now));
        active.update(&view, Some((region, 0.0)), UNIX_EPOCH + Duration::from_secs(1), now + Duration::from_millis(16));
        let scroll = active.cloud_scroll();
        assert!((scroll[0] - 0.002).abs() < 0.000001);
        assert!((scroll[1] - 0.0001).abs() < 0.000001);
    }

    #[test]
    fn water_phase_uses_elapsed_runtime_seconds() {
        let mut world = world();
        let region = Region::new(1);
        add_region_environment(&mut world, region, b"{'type':'water'}", None);
        let (mut scene, mut view) = Reconciler::new(Budget::default());
        scene.notify_world_change();
        settle(&mut scene, &mut view, &world);
        let mut active = ActiveEnvironment::default();
        let now = Instant::now();
        active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now);
        assert_eq!(active.water_scroll(), [0.0; 4]);
        active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_secs(2));
        assert_eq!(active.animation_time(), 2.0);
        assert_eq!(active.water_scroll(), [1.0, 0.0, 0.0, 0.0]);
    }
}
