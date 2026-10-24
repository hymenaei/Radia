use std::{collections::BTreeMap, sync::Arc};

use llsd::{Limits, Map, Value, binary, notation, xml};
use utils::AssetId;
use uuid::Uuid;

use crate::{Failure, TextureRef};

const MAX_KEYS: usize = 56;
const MAX_FRAMES: usize = 280;

#[derive(Debug)]
pub struct EnvironmentDescription {
    pub name: String,
    pub flags: u32,
    pub settings: EnvironmentSettings,
}

impl EnvironmentDescription {
    pub(crate) fn retained_bytes(&self) -> usize {
        let shared = 2 * size_of::<usize>();
        size_of::<Self>()
            + self.name.capacity()
            + match &self.settings {
                EnvironmentSettings::Sky(sky) => sky.retained_bytes(),
                EnvironmentSettings::Water(_) => size_of::<Water>(),
                EnvironmentSettings::DayCycle(day) => {
                    day.water.capacity() * size_of::<Keyframe<Water>>()
                        + day.water.len() * (shared + size_of::<Water>())
                        + day
                            .skies
                            .iter()
                            .map(|track| {
                                track.capacity() * size_of::<Keyframe<Sky>>()
                                    + track.iter().map(|frame| shared + frame.settings.retained_bytes()).sum::<usize>()
                            })
                            .sum::<usize>()
                }
            }
    }
}

#[derive(Debug)]
pub enum EnvironmentSettings {
    Sky(Box<Sky>),
    Water(Box<Water>),
    DayCycle(DayCycle),
}

#[derive(Debug)]
pub struct DayCycle {
    water: Vec<Keyframe<Water>>,
    skies: [Vec<Keyframe<Sky>>; 4],
}

impl DayCycle {
    pub fn water(&self) -> &[Keyframe<Water>] {
        &self.water
    }

    pub fn skies(&self) -> &[Vec<Keyframe<Sky>>; 4] {
        &self.skies
    }
}

#[derive(Debug)]
pub struct Keyframe<T> {
    position: f32,
    settings: Arc<T>,
}

impl<T> Keyframe<T> {
    pub fn position(&self) -> f32 {
        self.position
    }

    pub fn settings(&self) -> &T {
        &self.settings
    }
}

#[derive(Debug, PartialEq)]
pub struct CelestialBody {
    pub rotation: Option<[f32; 4]>,
    pub texture: TextureRef,
    pub scale: Option<f32>,
}

#[derive(Debug, PartialEq)]
pub struct Clouds {
    pub color: Option<[f32; 3]>,
    pub density: [Option<[f32; 3]>; 2],
    pub scale: Option<f32>,
    pub scroll_rate: Option<[f32; 2]>,
    pub shadow: Option<f32>,
    pub variance: Option<f32>,
    pub texture: TextureRef,
}

#[derive(Debug, PartialEq)]
pub struct Haze {
    pub ambient: Option<[f32; 3]>,
    pub blue_density: Option<[f32; 3]>,
    pub blue_horizon: Option<[f32; 3]>,
    pub density_multiplier: Option<f32>,
    pub distance_multiplier: Option<f32>,
    pub density: Option<f32>,
    pub horizon: Option<f32>,
}

#[derive(Debug, PartialEq)]
pub struct DensityLayer {
    pub width: Option<f32>,
    pub exponential_term: Option<f32>,
    pub exponential_scale: Option<f32>,
    pub linear_term: Option<f32>,
    pub constant_term: Option<f32>,
    pub anisotropy: Option<f32>,
}

#[derive(Debug, PartialEq)]
pub struct Atmosphere {
    pub planet_radius: Option<f32>,
    pub bottom_radius: Option<f32>,
    pub top_radius: Option<f32>,
    pub sun_arc: Option<f32>,
    pub rayleigh: Option<Vec<DensityLayer>>,
    pub mie: Option<Vec<DensityLayer>>,
    pub absorption: Option<Vec<DensityLayer>>,
    pub moisture: Option<f32>,
    pub droplet_radius: Option<f32>,
    pub ice: Option<f32>,
}

#[derive(Debug, PartialEq)]
pub struct Sky {
    pub sun: CelestialBody,
    pub moon: CelestialBody,
    pub sunlight: Option<[f32; 3]>,
    pub moon_brightness: Option<f32>,
    pub star_brightness: Option<f32>,
    pub clouds: Clouds,
    pub haze: Haze,
    pub legacy_haze: Option<Haze>,
    pub atmosphere: Atmosphere,
    pub gamma: Option<f32>,
    pub glow: Option<[f32; 3]>,
    pub max_y: Option<f32>,
    pub dome_offset: Option<f32>,
    pub dome_radius: Option<f32>,
    pub reflection_probe_ambiance: Option<f32>,
    pub bloom: TextureRef,
    pub rainbow: TextureRef,
    pub halo: TextureRef,
}

impl Sky {
    fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + [&self.atmosphere.rayleigh, &self.atmosphere.mie, &self.atmosphere.absorption]
                .into_iter()
                .map(|layers| layers.as_ref().map_or(0, |layers| layers.capacity() * size_of::<DensityLayer>()))
                .sum::<usize>()
    }
}

#[derive(Debug, PartialEq)]
pub struct Water {
    pub fog_color: Option<[f32; 3]>,
    pub fog_density: Option<f32>,
    pub underwater_fog: Option<f32>,
    pub blur: Option<f32>,
    pub fresnel_offset: Option<f32>,
    pub fresnel_scale: Option<f32>,
    pub transparent_texture: TextureRef,
    pub normal_map: TextureRef,
    pub normal_scale: Option<[f32; 3]>,
    pub scale_above: Option<f32>,
    pub scale_below: Option<f32>,
    pub waves: [Option<[f32; 2]>; 2],
}

pub(crate) fn decode(bytes: &[u8]) -> Result<EnvironmentDescription, Failure> {
    let limits = Limits {
        max_bytes: 2 * 1024 * 1024,
        max_depth: 12,
        max_values: 32_768,
        max_container_items: MAX_FRAMES,
        max_string_bytes: 1024,
        max_map_key_bytes: 128,
        max_binary_bytes: 0,
    };
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(bytes.len());
    let input = bytes.get(start..).ok_or(Failure::Malformed)?;
    let value = if input.starts_with(b"<?llsd/binary") {
        binary::from_slice_with_limits(input, limits)
    } else if input.starts_with(b"<") && !input.starts_with(b"<?llsd/notation") {
        xml::from_slice_with_limits(input, limits)
    } else {
        notation::from_slice_with_limits(input, limits)
    }
    .map_err(|_| Failure::Malformed)?;
    let fields = object(&value)?;
    let name = match fields.get("name") {
        Some(Value::String(name)) => name.clone(),
        None => String::new(),
        _ => return Err(Failure::Malformed),
    };
    let flags = match fields.get("flags") {
        Some(Value::Integer(flags)) => *flags as u32,
        None => 0,
        _ => return Err(Failure::Malformed),
    };
    let settings = match kind(fields)? {
        "sky" => EnvironmentSettings::Sky(Box::new(sky(fields)?)),
        "water" => EnvironmentSettings::Water(Box::new(water(fields)?)),
        "daycycle" => EnvironmentSettings::DayCycle(day_cycle(fields)?),
        _ => return Err(Failure::Unsupported),
    };
    Ok(EnvironmentDescription { name, flags, settings })
}

fn object(value: &Value) -> Result<&Map, Failure> {
    if let Value::Map(fields) = value { Ok(fields) } else { Err(Failure::Malformed) }
}

fn kind(fields: &Map) -> Result<&str, Failure> {
    if let Some(Value::String(kind)) = fields.get("type") { Ok(kind) } else { Err(Failure::Malformed) }
}

fn number(value: &Value) -> Result<f32, Failure> {
    let number = match value {
        Value::Real(number) => *number as f32,
        Value::Integer(number) => *number as f32,
        _ => return Err(Failure::Malformed),
    };
    if number.is_finite() { Ok(number) } else { Err(Failure::Malformed) }
}

fn scalar(fields: &Map, key: &str) -> Result<Option<f32>, Failure> {
    fields.get(key).map(number).transpose()
}

fn vector<const N: usize>(fields: &Map, key: &str) -> Result<Option<[f32; N]>, Failure> {
    let Some(value) = fields.get(key) else { return Ok(None) };
    let Value::Array(values) = value else { return Err(Failure::Malformed) };
    if values.len() != N && !(N == 3 && values.len() == 4) {
        return Err(Failure::Malformed);
    }
    let mut result = [0.0; N];
    for (index, value) in values.iter().enumerate() {
        let value = number(value)?;
        if let Some(component) = result.get_mut(index) {
            *component = value;
        }
    }
    Ok(Some(result))
}

fn texture(fields: &Map, key: &str) -> Result<TextureRef, Failure> {
    match fields.get(key) {
        Some(Value::Uuid(id)) if *id == Uuid::nil() => Ok(TextureRef::Clear),
        Some(Value::Uuid(id)) => Ok(TextureRef::Asset(AssetId::new(*id))),
        None => Ok(TextureRef::Absent),
        _ => Err(Failure::Malformed),
    }
}

fn body(fields: &Map, rotation: &str, image: &str, scale: &str) -> Result<CelestialBody, Failure> {
    let rotation = vector::<4>(fields, rotation)?
        .map(|mut rotation| {
            let length = rotation.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
            if length < 1e-12 {
                return Err(Failure::Malformed);
            }
            for component in &mut rotation {
                *component = (f64::from(*component) / length) as f32;
            }
            Ok(rotation)
        })
        .transpose()?;
    Ok(CelestialBody { rotation, texture: texture(fields, image)?, scale: scalar(fields, scale)? })
}

fn haze(fields: &Map) -> Result<Haze, Failure> {
    Ok(Haze {
        ambient: vector(fields, "ambient")?,
        blue_density: vector(fields, "blue_density")?,
        blue_horizon: vector(fields, "blue_horizon")?,
        density_multiplier: scalar(fields, "density_multiplier")?,
        distance_multiplier: scalar(fields, "distance_multiplier")?,
        density: scalar(fields, "haze_density")?,
        horizon: scalar(fields, "haze_horizon")?,
    })
}

fn profile(fields: &Map, key: &str) -> Result<Option<Vec<DensityLayer>>, Failure> {
    let Some(value) = fields.get(key) else { return Ok(None) };
    let mut layers = Vec::new();
    profile_layers(value, &mut layers)?;
    Ok(Some(layers))
}

fn profile_layers(value: &Value, layers: &mut Vec<DensityLayer>) -> Result<(), Failure> {
    match value {
        Value::Array(values) => {
            for value in values {
                profile_layers(value, layers)?;
            }
        }
        Value::Map(fields) => layers.push(DensityLayer {
            width: scalar(fields, "width")?,
            exponential_term: scalar(fields, "exp_term")?,
            exponential_scale: scalar(fields, "exp_scale")?,
            linear_term: scalar(fields, "linear_term")?,
            constant_term: scalar(fields, "constant_term")?,
            anisotropy: scalar(fields, "anisotropy")?,
        }),
        _ => return Err(Failure::Malformed),
    }
    Ok(())
}

fn sky(fields: &Map) -> Result<Sky, Failure> {
    Ok(Sky {
        sun: body(fields, "sun_rotation", "sun_id", "sun_scale")?,
        moon: body(fields, "moon_rotation", "moon_id", "moon_scale")?,
        sunlight: vector(fields, "sunlight_color")?,
        moon_brightness: scalar(fields, "moon_brightness")?,
        star_brightness: scalar(fields, "star_brightness")?,
        clouds: Clouds {
            color: vector(fields, "cloud_color")?,
            density: [vector(fields, "cloud_pos_density1")?, vector(fields, "cloud_pos_density2")?],
            scale: scalar(fields, "cloud_scale")?,
            scroll_rate: vector(fields, "cloud_scroll_rate")?,
            shadow: scalar(fields, "cloud_shadow")?,
            variance: scalar(fields, "cloud_variance")?,
            texture: texture(fields, "cloud_id")?,
        },
        haze: haze(fields)?,
        legacy_haze: fields.get("legacy_haze").map(|value| object(value).and_then(haze)).transpose()?,
        atmosphere: Atmosphere {
            planet_radius: scalar(fields, "planet_radius")?,
            bottom_radius: scalar(fields, "sky_bottom_radius")?,
            top_radius: scalar(fields, "sky_top_radius")?,
            sun_arc: scalar(fields, "sun_arc_radians")?,
            rayleigh: profile(fields, "rayleigh_config")?,
            mie: profile(fields, "mie_config")?,
            absorption: profile(fields, "absorption_config")?,
            moisture: scalar(fields, "moisture_level")?,
            droplet_radius: scalar(fields, "droplet_radius")?,
            ice: scalar(fields, "ice_level")?,
        },
        gamma: scalar(fields, "gamma")?,
        glow: vector(fields, "glow")?,
        max_y: scalar(fields, "max_y")?,
        dome_offset: scalar(fields, "dome_offset")?,
        dome_radius: scalar(fields, "dome_radius")?,
        reflection_probe_ambiance: scalar(fields, "reflection_probe_ambiance")?,
        bloom: texture(fields, "bloom_id")?,
        rainbow: texture(fields, "rainbow_id")?,
        halo: texture(fields, "halo_id")?,
    })
}

fn water(fields: &Map) -> Result<Water, Failure> {
    Ok(Water {
        fog_color: vector(fields, "water_fog_color")?,
        fog_density: scalar(fields, "water_fog_density")?,
        underwater_fog: scalar(fields, "underwater_fog_mod")?,
        blur: scalar(fields, "blur_multiplier")?,
        fresnel_offset: scalar(fields, "fresnel_offset")?,
        fresnel_scale: scalar(fields, "fresnel_scale")?,
        transparent_texture: texture(fields, "transparent_texture")?,
        normal_map: texture(fields, "normal_map")?,
        normal_scale: vector(fields, "normal_scale")?,
        scale_above: scalar(fields, "scale_above")?,
        scale_below: scalar(fields, "scale_below")?,
        waves: [vector(fields, "wave1_direction")?, vector(fields, "wave2_direction")?],
    })
}

fn day_cycle(map: &Map) -> Result<DayCycle, Failure> {
    let frames = map.get("frames").ok_or(Failure::Malformed).and_then(object)?;
    if frames.len() > MAX_FRAMES {
        return Err(Failure::TooLarge);
    }
    let mut skies = BTreeMap::new();
    let mut waters = BTreeMap::new();
    for (name, value) in frames {
        let fields = object(value)?;
        match kind(fields)? {
            "sky" => {
                skies.insert(name.as_str(), Arc::new(sky(fields)?));
            }
            "water" => {
                waters.insert(name.as_str(), Arc::new(water(fields)?));
            }
            _ => return Err(Failure::Unsupported),
        }
    }
    let Some(Value::Array(tracks)) = map.get("tracks") else { return Err(Failure::Malformed) };
    if tracks.len() > 5 {
        return Err(Failure::TooLarge);
    }
    let water = track(tracks.first(), &waters)?;
    let mut skies_out = std::array::from_fn(|_| Vec::new());
    for (index, result) in skies_out.iter_mut().enumerate() {
        *result = track(tracks.get(index + 1), &skies)?;
    }
    if water.is_empty() || skies_out.iter().all(Vec::is_empty) {
        return Err(Failure::Malformed);
    }
    Ok(DayCycle { water, skies: skies_out })
}

fn track<T>(value: Option<&Value>, frames: &BTreeMap<&str, Arc<T>>) -> Result<Vec<Keyframe<T>>, Failure> {
    let Some(value) = value else { return Ok(Vec::new()) };
    let Value::Array(keys) = value else { return Err(Failure::Malformed) };
    if keys.len() > MAX_KEYS {
        return Err(Failure::TooLarge);
    }
    let mut result = Vec::with_capacity(keys.len());
    for key in keys {
        let fields = object(key)?;
        let position = fields.get("key_keyframe").ok_or(Failure::Malformed).and_then(number)?;
        if !(0.0..=1.0).contains(&position) {
            return Err(Failure::Malformed);
        }
        let Some(Value::String(name)) = fields.get("key_name") else { return Err(Failure::Malformed) };
        let settings = frames.get(name.as_str()).ok_or(Failure::Malformed)?;
        result.push(Keyframe { position, settings: Arc::clone(settings) });
    }
    result.sort_by(|a, b| a.position.total_cmp(&b.position));
    if result.windows(2).any(|pair| pair[0].position == pair[1].position) {
        return Err(Failure::Malformed);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
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
     'water':{'type':'water','water_fog_color':[r0.0156,r0.149,r0.2509],
     'water_fog_density':r2,'normal_scale':[r2,r2,r2],
     'wave1_direction':[r1.04999,r-0.42],'wave2_direction':[r1.10999,r-1.16]},
     'morning':{'type':'sky','sunlight_color':[r0.5,r0.4,r0.3]},
     'night':{'type':'sky','star_brightness':r250}},
     'tracks':[[{'key_keyframe':r0,'key_name':'water'}],
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
                        [
                            ("key_keyframe".into(), Value::Real(f64::from(index) / 56.0)),
                            ("key_name".into(), Value::String("morning".into())),
                        ]
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
            ("'key_name':'water'", "'key_name':'night'"),
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
}
