use std::time::Instant;

use super::*;
use crate::{Budget, Content, Priority, Request, Status, Store};

const SKY: &[u8] = br#"<?llsd/notation?>
{'type':'sky','name':'Authored sky','flags':i3,
 'sun_rotation':[r0,r0,r0,r2],'moon_rotation':[r0,r0,r1,r0],
 'sunlight_color':[r0.7342,r0.7815,r0.8999,r0],
 'cloud_scroll_rate':[r0.2,r0.01],'cloud_color':[r0.4099,r0.4099,r0.4099,r0],
 'legacy_haze':{'ambient':[r0.25,r0.25,r0.25],'haze_density':r0.7},
 'rayleigh_config':[{'width':r0,'exp_term':r1,'exp_scale':r-0.000125}],
 'reflection_probe_ambiance':r0,
 'sun_id':u00112233-4455-6677-8899-aabbccddeeff,'moon_id':u00000000-0000-0000-0000-000000000000}
"#;

const DAY: &[u8] = br#"{'type':'daycycle','frames':{
 'ocean':{'type':'water','water_fog_color':[r0.0156,r0.149,r0.2509],
 'water_fog_density':r2,'normal_scale':[r2,r2,r2],
 'wave1_direction':[r1.04999,r-0.42],'wave2_direction':[r1.10999,r-1.16]},
 'morning':{'type':'sky','sunlight_color':[r0.5,r0.4,r0.3]},
 'night':{'type':'sky','star_brightness':r250}},
 'tracks':[[{'key_keyframe':r0,'key_name':'ocean'}],
 [{'key_keyframe':r0.75,'key_name':'night'},{'key_keyframe':r0.25,'key_name':'morning'}],
 [],[],[{'key_keyframe':r0.5,'key_name':'night'}]]}
"#;

#[test]
fn authored_sky_survives() {
    let definition = decode(SKY).unwrap();
    assert_eq!(definition.name, "Authored sky");
    assert_eq!(definition.flags, 3);
    let EnvironmentSettings::Sky(sky) = definition.settings else { panic!("sky") };
    assert_eq!(sky.sun.rotation, Some([0.0, 0.0, 0.0, 1.0]));
    assert_eq!(sky.sun.texture, TextureRef::Asset(AssetId::new(Uuid::from_u128(0x00112233445566778899aabbccddeeff))));
    assert_eq!(sky.sunlight, Some([0.7342, 0.7815, 0.8999]));
    assert_eq!(sky.clouds.scroll_rate, Some([0.2, 0.01]));
    assert_eq!(sky.legacy_haze.unwrap().density, Some(0.7));
    assert_eq!(sky.atmosphere.rayleigh.as_ref().unwrap()[0].exponential_scale, Some(-0.000125));
    assert_eq!(sky.reflection_probe_ambiance, Some(0.0), "authored zero must remain distinct from automatic ambiance");
    assert_eq!(sky.gamma, None, "missing parameters remain absent until evaluation applies defaults");
    assert_eq!(sky.moon.texture, TextureRef::Clear, "authored nil must remain distinct from a missing texture field");
    assert_eq!(sky.clouds.texture, TextureRef::Absent);
}

#[test]
fn day_tracks_preserve_layers() {
    let definition = decode(DAY).unwrap();
    let EnvironmentSettings::DayCycle(day) = definition.settings else { panic!("day cycle") };
    assert_eq!(day.water().len(), 1);
    assert_eq!(day.water()[0].settings().waves, [Some([1.04999, -0.42]), Some([1.10999, -1.16])]);
    assert_eq!(day.skies()[0].iter().map(Keyframe::position).collect::<Vec<_>>(), [0.25, 0.75]);
    assert!(day.skies()[1].is_empty());
    assert!(day.skies()[2].is_empty());
    assert_eq!(day.skies()[3][0].settings().star_brightness, Some(250.0));
    assert!(Arc::ptr_eq(&day.skies()[0][1].settings, &day.skies()[3][0].settings), "shared named frames decode once");
}

#[test]
fn settings_formats_agree() {
    let value: Value = notation::from_slice(SKY).unwrap();
    for bytes in [xml::to_vec(&value).unwrap(), binary::to_vec(&value).unwrap(), SKY.to_vec()] {
        let description = decode(&bytes).unwrap();
        let EnvironmentSettings::Sky(sky) = description.settings else { panic!("sky") };
        assert_eq!(description.name, "Authored sky");
        assert_eq!(sky.sunlight, Some([0.7342, 0.7815, 0.8999]));
        assert_eq!(sky.reflection_probe_ambiance, Some(0.0));
    }
}

#[test]
fn day_track_limits_hold() {
    let mut value: Value = notation::from_slice(DAY).unwrap();
    let Value::Map(fields) = &mut value else { panic!("day cycle") };
    let Some(Value::Array(tracks)) = fields.get_mut("tracks") else { panic!("tracks") };
    tracks[1] = Value::Array(
        (0..56)
            .map(|index| {
                Value::Map(
                    [("key_keyframe".into(), Value::Real(f64::from(index) / 56.0)), ("key_name".into(), Value::String("morning".into()))]
                        .into(),
                )
            })
            .collect(),
    );
    let bytes = notation::to_vec(&value).unwrap();
    let EnvironmentSettings::DayCycle(day) = decode(&bytes).unwrap().settings else { panic!("day cycle") };
    assert_eq!(day.skies()[0].len(), 56);
    let Value::Map(fields) = &mut value else { panic!("day cycle") };
    let Some(Value::Array(tracks)) = fields.get_mut("tracks") else { panic!("tracks") };
    let Value::Array(keys) = &mut tracks[1] else { panic!("keys") };
    keys.push(Value::Map([("key_keyframe".into(), Value::Real(1.0)), ("key_name".into(), Value::String("morning".into()))].into()));
    assert_eq!(decode(&notation::to_vec(&value).unwrap()).unwrap_err(), Failure::TooLarge);
}

#[test]
fn malformed_settings_fail() {
    for bytes in [
        br#"{'type':'sky','sun_rotation':[r0,r0,r0,r0]}"#.as_slice(),
        br#"{'type':'sky','gamma':rnan}"#,
        br#"{'type':'sky','gamma':r1e300}"#,
        br#"{'type':'sky','sunlight_color':[r1,r2]}"#,
        br#"{'type':'sky','sun_id':'not a uuid'}"#,
        br#"{'type':'daycycle','frames':{},'tracks':[]}"#,
    ] {
        assert_eq!(decode(bytes).unwrap_err(), Failure::Malformed, "{}", String::from_utf8_lossy(bytes));
    }
    for (from, to) in [
        ("'key_name':'morning'", "'key_name':'missing'"),
        ("'key_name':'ocean'", "'key_name':'night'"),
        ("r0.25", "r0.75"),
        ("r0.25", "r-0.1"),
    ] {
        let bytes = String::from_utf8_lossy(DAY).replace(from, to);
        assert_eq!(decode(bytes.as_bytes()).unwrap_err(), Failure::Malformed, "{from} -> {to}");
    }
    assert_eq!(decode(br#"{'type':'future_environment'}"#).unwrap_err(), Failure::Unsupported);
}

#[test]
fn settings_charge_follows_ownership() {
    let budget = Budget::default();
    let mut store = Store::new(budget.clone());
    let now = Instant::now();
    let route = store.register_route().unwrap();
    let demand = store.request(route, Request::Environment(AssetId::new(Uuid::from_u128(1))), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    let prepared = job.prepare(DAY.to_vec());
    store.complete(job, prepared, now);
    let Status::Ready(Content::Environment(environment)) = demand.status() else { panic!("ready environment") };
    let retained = budget.stats().resident_bytes;
    assert!(retained > size_of::<EnvironmentDescription>());
    drop(demand);
    store.close();
    drop(store);
    assert_eq!(budget.stats().resident_bytes, retained, "consumer retains the complete environment charge after shutdown");
    drop(environment);
    assert_eq!(budget.stats().charged_bytes, 0);
}
