use std::time::{Duration, Instant};

use assets::{Budget, Content, Environment, EnvironmentDescription, EnvironmentSettings, LoadJob, Priority, Request, Status, Store};
use utils::AssetId;
use uuid::Uuid;

use super::{Evaluated, Schedule, Sources, Transition, evaluate};

const CYCLE: &[u8] = br#"<?llsd/notation?>
{'type':'daycycle','frames':{
 'water_a':{'type':'water','water_fog_density':r1,'normal_map':u00000000-0000-0000-0000-000000000003},
 'water_b':{'type':'water','water_fog_density':r3,'normal_map':u00000000-0000-0000-0000-000000000004},
 'sky_a':{'type':'sky','sunlight_color':[r1,r0,r0],'sun_rotation':[r0,r0,r0,r1],
  'sun_id':u00000000-0000-0000-0000-000000000001,'cloud_id':u00000000-0000-0000-0000-000000000005},
 'sky_b':{'type':'sky','sunlight_color':[r0,r0,r1],'sun_rotation':[r0,r0,r0.7071067812,r0.7071067812],
  'sun_id':u00000000-0000-0000-0000-000000000002,'cloud_id':u00000000-0000-0000-0000-000000000006},
 'sky_c':{'type':'sky','sunlight_color':[r0,r1,r0]}},
 'tracks':[
  [{'key_keyframe':r0.25,'key_name':'water_a'},{'key_keyframe':r0.75,'key_name':'water_b'}],
  [{'key_keyframe':r0.25,'key_name':'sky_a'},{'key_keyframe':r0.75,'key_name':'sky_b'}],
  [],
  [{'key_keyframe':r0.5,'key_name':'sky_c'}],
  [{'key_keyframe':r0.5,'key_name':'sky_b'}]]}
"#;

const GREEN_DAY: &[u8] = br#"<?llsd/notation?>
{'type':'daycycle','frames':{
 'water':{'type':'water','water_fog_density':r5},
 'sky':{'type':'sky','sunlight_color':[r0,r1,r0]}},
 'tracks':[
  [{'key_keyframe':r0,'key_name':'water'}],
  [{'key_keyframe':r0,'key_name':'sky'}]]}
"#;

const BLUE_SKY: &[u8] = br#"<?llsd/notation?>
{'type':'sky','sunlight_color':[r0,r0,r1]}
"#;

const RED_SKY: &[u8] = br#"<?llsd/notation?>
{'type':'sky','sunlight_color':[r1,r0,r0]}
"#;

const GREEN_SKY: &[u8] = br#"<?llsd/notation?>
{'type':'sky','sunlight_color':[r0,r1,r0]}
"#;

const HAZE_SKY: &[u8] = br#"<?llsd/notation?>
{'type':'sky','ambient':[r0.1,r0.2,r0.3],'blue_density':[r0.11,r0.12,r0.13],
 'blue_horizon':[r0.2,r0.3,r0.4],'density_multiplier':r0.15,
 'distance_multiplier':r0.16,'haze_density':r0.17,
 'legacy_haze':{'ambient':[r0.8,r0.7,r0.6],'blue_horizon':[r0.5,r0.4,r0.3],
  'distance_multiplier':r0.25,'haze_density':r0.35}}
"#;

const BLUE_WATER: &[u8] = br#"<?llsd/notation?>
{'type':'water','water_fog_density':r7,'normal_map':u00000000-0000-0000-0000-000000000000}
"#;

const SKY_OVERRIDE: &[u8] = br#"<?llsd/notation?>
{'type':'sky','sunlight_color':[r0,r0,r1],'moon_brightness':r0.8,
 'moon_id':u00000000-0000-0000-0000-000000000000}
"#;

fn environment(bytes: &[u8]) -> Environment {
    let mut store = Store::new(Budget::default());
    let now = Instant::now();
    let Ok(route) = store.register_route() else { panic!("route admission failed") };
    let Ok(demand) = store.request(route, Request::Environment(AssetId::new(Uuid::from_u128(1))), Priority::Visible, now) else {
        panic!("environment request failed")
    };
    let Some(LoadJob::Fetch(job)) = store.next_job(now) else { panic!("environment fetch missing") };
    let Ok(input) = job.complete(Ok(bytes.to_vec())) else { panic!("environment fetch completion failed") };
    let Ok(decode) = store.start_decode(input) else { panic!("environment decode admission failed") };
    store.complete(decode.run(), now);
    let Status::Ready(Content::Environment(environment)) = demand.status() else { panic!("environment decode failed") };
    environment
}

fn sources<'a>(base: Option<&'a EnvironmentDescription>) -> Sources<'a> {
    Sources {
        base,
        schedule: Schedule::default(),
        experience_day: None,
        experience_sky: None,
        experience_water: None,
        sky_override: None,
        water_override: None,
    }
}

fn evaluated(environment: &Environment, utc: f64, altitude: f64, schedule: Schedule) -> Evaluated {
    evaluate(
        &Sources {
            base: Some(environment),
            schedule,
            experience_day: None,
            experience_sky: None,
            experience_water: None,
            sky_override: None,
            water_override: None,
        },
        utc,
        altitude,
    )
}

fn close(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 0.0001, "expected {expected}, got {actual}");
}

#[test]
fn track_wrap_interpolates() {
    let day = environment(CYCLE);
    let result = evaluated(&day, 0.0, 0.0, Schedule::default());
    assert_eq!(result.sky.sunlight, [0.5, 0.0, 0.5]);
    assert_eq!(result.sky_track, Some(1));
    assert_eq!(result.water.fog_density, 2.0);
    assert_eq!(
        result.sky.sun.texture,
        super::TextureBlend { current: Some(AssetId::new(Uuid::from_u128(2))), next: Some(AssetId::new(Uuid::from_u128(1))), factor: 0.5 }
    );
    assert_eq!(
        result.sky.clouds.texture,
        super::TextureBlend { current: Some(AssetId::new(Uuid::from_u128(6))), next: Some(AssetId::new(Uuid::from_u128(5))), factor: 0.5 }
    );
    assert_eq!(
        result.water.normal_map,
        super::TextureBlend { current: Some(AssetId::new(Uuid::from_u128(4))), next: Some(AssetId::new(Uuid::from_u128(3))), factor: 0.5 }
    );
}

#[test]
fn track_hit_is_exact() {
    let day = environment(CYCLE);
    let result = evaluated(&day, 3_600.0, 0.0, Schedule::default());
    assert_eq!(result.sky.sunlight, [1.0, 0.0, 0.0]);
    assert_eq!(result.water.fog_density, 1.0);
}

#[test]
fn single_key_stays_constant() {
    let day = environment(CYCLE);
    let result = evaluated(&day, 0.0, 15.0, Schedule { day_length: 14_400.0, day_offset: 0.0, altitudes: [0.0, 10.0, 20.0] });
    assert_eq!(result.sky_track, Some(3));
    assert_eq!(result.sky.sunlight, [0.0, 1.0, 0.0]);
}

#[test]
fn day_position_wraps_safely() {
    let day = environment(CYCLE);
    let schedule = Schedule::default();
    let quarter = evaluated(&day, 3_600.0, 0.0, schedule);
    close(quarter.day_position as f32, 0.25);
    let negative = evaluated(&day, -1.0, 0.0, schedule);
    assert!(negative.day_position.is_finite());
    assert!((0.0..=1.0).contains(&negative.day_position));
    close(negative.day_position as f32, 14_399.0 / 14_400.0);
    let huge = evaluated(&day, f64::MAX, 0.0, schedule);
    assert!(huge.day_position.is_finite());
    assert!((0.0..=1.0).contains(&huge.day_position));
}

#[test]
fn altitude_tracks_fallback() {
    let day = environment(CYCLE);
    let schedule = Schedule { day_length: 14_400.0, day_offset: 0.0, altitudes: [0.0, 10.0, 20.0] };
    let ground = evaluated(&day, 0.0, 0.0, schedule);
    assert_eq!(ground.sky_track, Some(1));
    let empty_second = evaluated(&day, 0.0, 5.0, schedule);
    assert_eq!(empty_second.sky_track, Some(1));
    let upper_empty_second = evaluated(&day, 0.0, 10.0, schedule);
    assert_eq!(upper_empty_second.sky_track, Some(1));
    let third = evaluated(&day, 0.0, 10.1, schedule);
    assert_eq!(third.sky_track, Some(3));
    let upper_third = evaluated(&day, 0.0, 20.0, schedule);
    assert_eq!(upper_third.sky_track, Some(3));
    let fourth = evaluated(&day, 0.0, 20.1, schedule);
    assert_eq!(fourth.sky_track, Some(4));
}

#[test]
fn source_precedence_overrides() {
    let base = environment(CYCLE);
    let day = environment(GREEN_DAY);
    let sky = environment(BLUE_SKY);
    let water = environment(BLUE_WATER);
    let inherited = evaluate(
        &Sources {
            base: Some(&base),
            schedule: Schedule { day_length: 14_400.0, day_offset: 0.0, altitudes: [0.0; 3] },
            experience_day: Some(&day),
            experience_sky: None,
            experience_water: None,
            sky_override: None,
            water_override: None,
        },
        0.0,
        0.0,
    );
    assert_eq!(inherited.sky.sunlight, [0.0, 1.0, 0.0]);
    assert_eq!(inherited.water.fog_density, 5.0);
    let explicit = evaluate(
        &Sources {
            base: Some(&base),
            schedule: Schedule { day_length: 14_400.0, day_offset: 0.0, altitudes: [0.0; 3] },
            experience_day: Some(&day),
            experience_sky: Some(&sky),
            experience_water: None,
            sky_override: None,
            water_override: None,
        },
        0.0,
        0.0,
    );
    assert_eq!(explicit.sky.sunlight, [0.0, 0.0, 1.0]);
    assert_eq!(explicit.water.fog_density, 5.0);
    let explicit_water = evaluate(
        &Sources {
            base: Some(&base),
            schedule: Schedule { day_length: 14_400.0, day_offset: 0.0, altitudes: [0.0; 3] },
            experience_day: Some(&day),
            experience_sky: None,
            experience_water: Some(&water),
            sky_override: None,
            water_override: None,
        },
        0.0,
        0.0,
    );
    assert_eq!(explicit_water.sky.sunlight, [0.0, 1.0, 0.0]);
    assert_eq!(explicit_water.water.fog_density, 7.0);
    assert_eq!(explicit_water.water.normal_map.current, None);
}

#[test]
fn property_overrides_are_partial() {
    let base = environment(CYCLE);
    let override_sky = environment(SKY_OVERRIDE);
    let override_water = environment(BLUE_WATER);
    let EnvironmentSettings::Sky(override_sky) = &override_sky.settings else { panic!("sky override expected") };
    let EnvironmentSettings::Water(override_water) = &override_water.settings else { panic!("water override expected") };
    let mut input = sources(Some(&base));
    input.sky_override = Some(override_sky);
    input.water_override = Some(override_water);
    let result = evaluate(&input, 3_600.0, 0.0);
    assert_eq!(result.sky.sunlight, [0.0, 0.0, 1.0]);
    assert_eq!(result.sky.moon_brightness, 0.8);
    assert_eq!(result.sky.star_brightness, 250.0);
    assert_eq!(result.sky.moon.texture.current, None);
    assert_eq!(result.water.fog_density, 7.0);
    assert_eq!(result.water.normal_map.current, None);
}

#[test]
fn defaults_match_viewer() {
    let result = evaluate(&sources(None), 0.0, 0.0);
    assert_eq!(result.sky.clouds.color, [0.4099, 0.4099, 0.4099]);
    assert_eq!(result.sky.sun.texture.current, None);
    assert_eq!(result.sky.moon.texture.current, Some(AssetId::new(Uuid::from_u128(0xd07f6eedb96a47cdb51d400ad4a1c428))));
    assert_eq!(result.water.fog_density, 2.0);
    assert_eq!(result.water.normal_scale, [1.75; 3]);
    assert_eq!(result.sky.haze.ambient, [0.25; 3]);
    assert_eq!(result.sky.haze.horizon, 0.19);
}

#[test]
fn haze_fields_prefer_legacy_then_top_level_then_default() {
    let sky = environment(HAZE_SKY);
    let result = evaluated(&sky, 0.0, 0.0, Schedule::default());
    assert_eq!(result.sky.haze.ambient, [0.8, 0.7, 0.6]);
    assert_eq!(result.sky.haze.blue_density, [0.11, 0.12, 0.13]);
    assert_eq!(result.sky.haze.blue_horizon, [0.5, 0.4, 0.3]);
    close(result.sky.haze.density_multiplier, 0.15);
    close(result.sky.haze.distance_multiplier, 0.25);
    close(result.sky.haze.density, 0.35);
    close(result.sky.haze.horizon, 0.19);
}

#[test]
fn fixed_defaults_stay_static() {
    let sky = environment(BLUE_SKY);
    let result = evaluated(&sky, 7_200.0, 0.0, Schedule { day_length: 14_400.0, day_offset: 0.0, altitudes: [0.0; 3] });
    close(result.sky.sun_direction[0], 80.0_f32.to_radians().cos());
    close(result.sky.sun_direction[1], 0.0);
    close(result.sky.sun_direction[2], 80.0_f32.to_radians().sin());
    assert_eq!(result.water.normal_scale, [1.75; 3]);
}

#[test]
fn rotation_slerps_midpoint() {
    let day = environment(CYCLE);
    let midpoint = evaluated(&day, 7_200.0, 0.0, Schedule::default());
    close(midpoint.sky.sun_direction[0], std::f32::consts::FRAC_1_SQRT_2);
    close(midpoint.sky.sun_direction[1], std::f32::consts::FRAC_1_SQRT_2);
    close(midpoint.sky.sun_direction[2], 0.0);
    let hit = evaluated(&day, 10_800.0, 0.0, Schedule::default());
    close(hit.sky.sun_direction[0], 0.0);
    close(hit.sky.sun_direction[1], 1.0);
}

#[test]
fn invalid_schedule_uses_defaults() {
    let day = environment(CYCLE);
    let invalid = Schedule { day_length: 0.0, day_offset: f64::NAN, altitudes: [20.0, 10.0, 0.0] };
    let result = evaluated(&day, 3_600.0, 15.0, invalid);
    close(result.day_position as f32, 0.25);
    assert_eq!(result.sky_track, Some(4));
    let non_finite = evaluated(&day, f64::INFINITY, 0.0, Schedule::default());
    assert_eq!(non_finite.day_position, 0.0);
}

#[test]
fn transition_progresses_and_paces() {
    let red = environment(RED_SKY);
    let blue = environment(BLUE_SKY);
    let now = Instant::now();
    let mut transition = Transition::default();
    let (initial, first_redraw) = transition.update(1, evaluated(&red, 0.0, 0.0, Schedule::default()), Duration::ZERO, now);
    assert_eq!(initial.sky.sunlight, [1.0, 0.0, 0.0]);
    assert_eq!(first_redraw, None);
    let (start, redraw) = transition.update(2, evaluated(&blue, 0.0, 0.0, Schedule::default()), Duration::from_secs(2), now);
    assert_eq!(start.sky.sunlight, [1.0, 0.0, 0.0]);
    assert_eq!(redraw, now.checked_add(Duration::from_millis(16)));
    let (middle, next) =
        transition.update(2, evaluated(&blue, 0.0, 0.0, Schedule::default()), Duration::from_secs(2), now + Duration::from_secs(1));
    assert_eq!(middle.sky.sunlight, [0.5, 0.0, 0.5]);
    assert_eq!(next, Some(now + Duration::from_secs(1) + Duration::from_millis(16)));
    let (finished, cadence) =
        transition.update(2, evaluated(&blue, 0.0, 0.0, Schedule::default()), Duration::from_secs(2), now + Duration::from_secs(2));
    assert_eq!(finished.sky.sunlight, [0.0, 0.0, 1.0]);
    assert_eq!(cadence, None);
}

#[test]
fn identity_change_restarts_from_output() {
    let red = environment(RED_SKY);
    let green = environment(GREEN_SKY);
    let blue = environment(BLUE_SKY);
    let now = Instant::now();
    let mut transition = Transition::default();
    transition.update(1, evaluated(&red, 0.0, 0.0, Schedule::default()), Duration::ZERO, now);
    transition.update(2, evaluated(&green, 0.0, 0.0, Schedule::default()), Duration::from_secs(4), now);
    let (middle, _) =
        transition.update(2, evaluated(&green, 0.0, 0.0, Schedule::default()), Duration::from_secs(4), now + Duration::from_secs(2));
    assert_eq!(middle.sky.sunlight, [0.5, 0.5, 0.0]);
    transition.update(3, evaluated(&blue, 0.0, 0.0, Schedule::default()), Duration::from_secs(2), now + Duration::from_secs(2));
    let (restarted, _) =
        transition.update(3, evaluated(&blue, 0.0, 0.0, Schedule::default()), Duration::from_secs(2), now + Duration::from_secs(3));
    assert_eq!(restarted.sky.sunlight, [0.25, 0.25, 0.5]);
}

#[test]
fn cycle_uses_coarse_redraw() {
    let day = environment(CYCLE);
    let now = Instant::now();
    let mut transition = Transition::default();
    let (_, next) = transition.update(1, evaluated(&day, 0.0, 0.0, Schedule::default()), Duration::ZERO, now);
    assert_eq!(next, now.checked_add(Duration::from_secs(60)));
}

#[test]
fn altitude_change_uses_viewer_duration() {
    let day = environment(CYCLE);
    let now = Instant::now();
    let schedule = Schedule { day_length: 14_400.0, day_offset: 3_600.0, altitudes: [0.0, 10.0, 20.0] };
    let mut transition = Transition::default();
    transition.update(1, evaluated(&day, 0.0, 0.0, schedule), Duration::ZERO, now);
    transition.update(1, evaluated(&day, 0.0, 25.0, schedule), Duration::from_secs(1), now);
    let (middle, next) =
        transition.update(1, evaluated(&day, 0.0, 25.0, schedule), Duration::from_secs(1), now + Duration::from_millis(2500));
    assert_eq!(middle.sky_track, Some(4));
    assert_eq!(middle.sky.sunlight, [0.5, 0.0, 0.5]);
    assert_eq!(next, Some(now + Duration::from_millis(2516)));
}
