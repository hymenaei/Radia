mod transition;

use std::f32::consts::PI;

use assets::{
    Atmosphere, CelestialBody, DayCycle, DensityLayer, EnvironmentDescription, EnvironmentSettings, Haze, Keyframe, Sky, TextureRef, Water,
};
use math::{Quat, Vec3};
use utils::AssetId;
use uuid::Uuid;

pub use transition::Transition;

const DEFAULT_DAY_LENGTH: f64 = 14_400.0;
const MIN_DAY_LENGTH: f64 = 14_400.0;
const MAX_DAY_LENGTH: f64 = 604_800.0;
const DEFAULT_DAY_OFFSET: f64 = 57_600.0;
const MAX_DAY_OFFSET: f64 = 86_400.0;

const DEFAULT_SUN_TEXTURE: Option<AssetId> = None;
const DEFAULT_MOON_TEXTURE: Option<AssetId> = Some(AssetId::new(Uuid::from_u128(0xd07f6eedb96a47cdb51d400ad4a1c428)));
const DEFAULT_CLOUD_TEXTURE: Option<AssetId> = Some(AssetId::new(Uuid::from_u128(0x1dc1368fe8fef02da08d9d9f11c1af6b)));
const DEFAULT_BLOOM_TEXTURE: Option<AssetId> = Some(AssetId::new(Uuid::from_u128(0x3c59f7fe9dc8479f8aafa9dd1fbc3bef)));
const DEFAULT_RAINBOW_TEXTURE: Option<AssetId> = Some(AssetId::new(Uuid::from_u128(0x11b4c57c56b304ed1f822004363882e4)));
const DEFAULT_HALO_TEXTURE: Option<AssetId> = Some(AssetId::new(Uuid::from_u128(0x12149143f59991a777acb52a3c0f59cd)));
const DEFAULT_WATER_TRANSPARENT_TEXTURE: Option<AssetId> = Some(AssetId::new(Uuid::from_u128(0x2bfd38847e2769b9ba3a3e673f680004)));
const DEFAULT_WATER_NORMAL_MAP: Option<AssetId> = Some(AssetId::new(Uuid::from_u128(0x822ded499a6cf61ccb896df54f42cdf4)));

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Schedule {
    pub day_length: f64,
    pub day_offset: f64,
    pub altitudes: [f32; 3],
}

impl Default for Schedule {
    fn default() -> Self {
        Self { day_length: DEFAULT_DAY_LENGTH, day_offset: DEFAULT_DAY_OFFSET, altitudes: [0.0; 3] }
    }
}

impl Schedule {
    fn normalized(self) -> Self {
        let default = Self::default();
        let day_length = if self.day_length.is_finite() && self.day_length > 0.0 {
            self.day_length.clamp(MIN_DAY_LENGTH, MAX_DAY_LENGTH)
        } else {
            default.day_length
        };
        let day_offset = if self.day_offset.is_finite() { self.day_offset.clamp(0.0, MAX_DAY_OFFSET) } else { default.day_offset };
        let altitudes = if self.altitudes.iter().all(|value| value.is_finite()) && self.altitudes.windows(2).all(|pair| pair[0] <= pair[1])
        {
            self.altitudes
        } else {
            default.altitudes
        };
        Self { day_length, day_offset, altitudes }
    }
}

pub struct Sources<'a> {
    pub base: Option<&'a EnvironmentDescription>,
    pub schedule: Schedule,
    pub experience_day: Option<&'a EnvironmentDescription>,
    pub experience_sky: Option<&'a EnvironmentDescription>,
    pub experience_water: Option<&'a EnvironmentDescription>,
    pub sky_override: Option<&'a Sky>,
    pub water_override: Option<&'a Water>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureBlend {
    pub current: Option<AssetId>,
    pub next: Option<AssetId>,
    pub factor: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CelestialBodyState {
    pub rotation: [f32; 4],
    pub texture: TextureBlend,
    pub scale: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CloudsState {
    pub color: [f32; 3],
    pub density: [[f32; 3]; 2],
    pub scale: f32,
    pub scroll_rate: [f32; 2],
    pub shadow: f32,
    pub variance: f32,
    pub texture: TextureBlend,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HazeState {
    pub ambient: [f32; 3],
    pub blue_density: [f32; 3],
    pub blue_horizon: [f32; 3],
    pub density_multiplier: f32,
    pub distance_multiplier: f32,
    pub density: f32,
    pub horizon: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DensityLayerState {
    pub width: f32,
    pub exponential_term: f32,
    pub exponential_scale: f32,
    pub linear_term: f32,
    pub constant_term: f32,
    pub anisotropy: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AtmosphereState {
    pub planet_radius: f32,
    pub bottom_radius: f32,
    pub top_radius: f32,
    pub sun_arc: f32,
    pub rayleigh: Vec<DensityLayerState>,
    pub mie: Vec<DensityLayerState>,
    pub absorption: Vec<DensityLayerState>,
    pub moisture: f32,
    pub droplet_radius: f32,
    pub ice: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SkyState {
    pub sun: CelestialBodyState,
    pub moon: CelestialBodyState,
    pub sun_direction: [f32; 3],
    pub moon_direction: [f32; 3],
    pub sunlight: [f32; 3],
    pub moon_brightness: f32,
    pub star_brightness: f32,
    pub clouds: CloudsState,
    pub haze: HazeState,
    pub legacy_haze: Option<HazeState>,
    pub atmosphere: AtmosphereState,
    pub gamma: f32,
    pub glow: [f32; 3],
    pub max_y: f32,
    pub dome_offset: f32,
    pub dome_radius: f32,
    pub reflection_probe_ambiance: f32,
    pub bloom: TextureBlend,
    pub rainbow: TextureBlend,
    pub halo: TextureBlend,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WaterState {
    pub fog_color: [f32; 3],
    pub fog_density: f32,
    pub underwater_fog: f32,
    pub blur: f32,
    pub fresnel_offset: f32,
    pub fresnel_scale: f32,
    pub transparent_texture: TextureBlend,
    pub normal_map: TextureBlend,
    pub normal_scale: [f32; 3],
    pub scale_above: f32,
    pub scale_below: f32,
    pub waves: [[f32; 2]; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Evaluated {
    pub sky: SkyState,
    pub water: WaterState,
    pub sky_track: Option<u8>,
    pub day_position: f64,
    cycle_length: Option<f64>,
}

pub fn evaluate(sources: &Sources<'_>, utc_seconds: f64, altitude: f64) -> Evaluated {
    let schedule = sources.schedule.normalized();
    let day_position = day_position(schedule, utc_seconds);
    let inherited = sources.experience_day.or(sources.base);
    let sky_source = sources.experience_sky.or(inherited);
    let water_source = sources.experience_water.or(inherited);
    let (mut sky, sky_track, sky_cycles) = sky_source
        .and_then(|source| sample_sky(source, day_position, altitude, schedule.altitudes))
        .unwrap_or_else(|| (sky_defaults(0.0), None, false));
    let (mut water, water_cycles) =
        water_source.and_then(|source| sample_water(source, day_position)).unwrap_or_else(|| (water_defaults(0.0), false));
    if let Some(override_sky) = sources.sky_override {
        apply_sky_override(&mut sky, override_sky);
    }
    if let Some(override_water) = sources.water_override {
        apply_water_override(&mut water, override_water);
    }
    Evaluated { sky, water, sky_track, day_position, cycle_length: (sky_cycles || water_cycles).then_some(schedule.day_length) }
}

fn day_position(schedule: Schedule, utc_seconds: f64) -> f64 {
    let utc_seconds = if utc_seconds.is_finite() { utc_seconds } else { 0.0 };
    let position = (utc_seconds + schedule.day_offset) % schedule.day_length / schedule.day_length;
    if position.is_finite() { position.clamp(0.0, 1.0) } else { 0.0 }
}

fn selected_track(altitude: f64, altitudes: [f32; 3]) -> u8 {
    for (index, threshold) in altitudes.iter().enumerate() {
        if altitude.partial_cmp(&f64::from(*threshold)) != Some(std::cmp::Ordering::Greater) {
            return index as u8 + 1;
        }
    }
    4
}

fn sample_sky(source: &EnvironmentDescription, position: f64, altitude: f64, altitudes: [f32; 3]) -> Option<(SkyState, Option<u8>, bool)> {
    match &source.settings {
        EnvironmentSettings::Sky(sky) => Some((sky_state(sky, 0.0), None, false)),
        EnvironmentSettings::DayCycle(day) => {
            let requested = selected_track(altitude, altitudes);
            let (track_number, track) = selected_sky_track(day, requested)?;
            Some((sample_sky_track(track, position as f32)?, Some(track_number), true))
        }
        EnvironmentSettings::Water(_) => None,
    }
}

fn selected_sky_track(day: &DayCycle, requested: u8) -> Option<(u8, &[Keyframe<Sky>])> {
    day.skies()
        .iter()
        .take(usize::from(requested))
        .enumerate()
        .rev()
        .find(|(_, track)| !track.is_empty())
        .or_else(|| day.skies().iter().enumerate().find(|(_, track)| !track.is_empty()))
        .map(|(index, track)| (index as u8 + 1, track.as_slice()))
}

fn sample_sky_track(track: &[Keyframe<Sky>], position: f32) -> Option<SkyState> {
    sample_track(track, position, |frame| sky_state(frame.settings(), frame.position()))
        .map(|(start, end, factor)| if factor == 0.0 { start } else { blend_sky(&start, &end, factor) })
}

fn sample_water(source: &EnvironmentDescription, position: f64) -> Option<(WaterState, bool)> {
    match &source.settings {
        EnvironmentSettings::Water(water) => Some((water_state(water, 0.0), false)),
        EnvironmentSettings::DayCycle(day) => Some((sample_water_track(day.water(), position as f32)?, true)),
        EnvironmentSettings::Sky(_) => None,
    }
}

fn sample_water_track(track: &[Keyframe<Water>], position: f32) -> Option<WaterState> {
    sample_track(track, position, |frame| water_state(frame.settings(), frame.position()))
        .map(|(start, end, factor)| if factor == 0.0 { start } else { blend_water(&start, &end, factor) })
}

fn sample_track<T, R, F>(track: &[Keyframe<T>], position: f32, mut state: F) -> Option<(R, R, f32)>
where
    R: Clone,
    F: FnMut(&Keyframe<T>) -> R,
{
    let first = track.first()?;
    if track.len() == 1 {
        let value = state(first);
        return Some((value.clone(), value, 0.0));
    }
    let mut before = None;
    let mut after = None;
    for frame in track {
        let key = frame.position();
        if key == position {
            let value = state(frame);
            return Some((value.clone(), value, 0.0));
        }
        if key < position {
            before = Some(frame);
        } else {
            after = Some(frame);
            break;
        }
    }
    let (start, end, start_position, end_position, adjusted_position) = match (before, after) {
        (Some(start), Some(end)) => (start, end, start.position(), end.position(), position),
        (Some(start), None) => {
            let end = first;
            (start, end, start.position(), end.position() + 1.0, position)
        }
        (None, Some(end)) => {
            let start = track.last()?;
            (start, end, start.position() - 1.0, end.position(), position)
        }
        (None, None) => return None,
    };
    let span = end_position - start_position;
    if !span.is_finite() || span <= 0.0 {
        return None;
    }
    let adjusted_position = if adjusted_position < start_position { adjusted_position + 1.0 } else { adjusted_position };
    let factor = ((adjusted_position - start_position) / span).clamp(0.0, 1.0);
    Some((state(start), state(end), factor))
}

fn texture(id: TextureRef, default: Option<AssetId>) -> TextureBlend {
    let current = match id {
        TextureRef::Absent => default,
        TextureRef::Clear => None,
        TextureRef::Asset(id) => Some(id),
    };
    TextureBlend { current, next: None, factor: 0.0 }
}

fn texture_at(value: TextureBlend) -> Option<AssetId> {
    if value.factor < 0.5 { value.current } else { value.next.or(value.current) }
}

fn blend_texture(start: TextureBlend, end: TextureBlend, factor: f32) -> TextureBlend {
    let current = texture_at(start);
    let next = texture_at(end);
    if current == next { TextureBlend { current, next: None, factor: 0.0 } } else { TextureBlend { current, next, factor } }
}

fn sky_state(sky: &Sky, position: f32) -> SkyState {
    let defaults = sky_defaults(position);
    let sun = celestial_state(&sky.sun, &defaults.sun);
    let moon = celestial_state(&sky.moon, &defaults.moon);
    SkyState {
        sun_direction: direction(sun.rotation),
        moon_direction: direction(moon.rotation),
        sun,
        moon,
        sunlight: sky.sunlight.unwrap_or(defaults.sunlight),
        moon_brightness: sky.moon_brightness.unwrap_or(defaults.moon_brightness),
        star_brightness: sky.star_brightness.unwrap_or(defaults.star_brightness),
        clouds: CloudsState {
            color: sky.clouds.color.unwrap_or(defaults.clouds.color),
            density: [
                sky.clouds.density[0].unwrap_or(defaults.clouds.density[0]),
                sky.clouds.density[1].unwrap_or(defaults.clouds.density[1]),
            ],
            scale: sky.clouds.scale.unwrap_or(defaults.clouds.scale),
            scroll_rate: sky.clouds.scroll_rate.unwrap_or(defaults.clouds.scroll_rate),
            shadow: sky.clouds.shadow.unwrap_or(defaults.clouds.shadow),
            variance: sky.clouds.variance.unwrap_or(defaults.clouds.variance),
            texture: texture(sky.clouds.texture, DEFAULT_CLOUD_TEXTURE),
        },
        haze: haze_state(&sky.haze, &defaults.haze),
        legacy_haze: sky.legacy_haze.as_ref().map(|haze| haze_state(haze, &defaults.haze)),
        atmosphere: atmosphere_state(&sky.atmosphere, &defaults.atmosphere),
        gamma: sky.gamma.unwrap_or(defaults.gamma),
        glow: sky.glow.unwrap_or(defaults.glow),
        max_y: sky.max_y.unwrap_or(defaults.max_y),
        dome_offset: sky.dome_offset.unwrap_or(defaults.dome_offset),
        dome_radius: sky.dome_radius.unwrap_or(defaults.dome_radius),
        reflection_probe_ambiance: sky.reflection_probe_ambiance.unwrap_or(defaults.reflection_probe_ambiance),
        bloom: texture(sky.bloom, DEFAULT_BLOOM_TEXTURE),
        rainbow: texture(sky.rainbow, DEFAULT_RAINBOW_TEXTURE),
        halo: texture(sky.halo, DEFAULT_HALO_TEXTURE),
    }
}

fn celestial_state(body: &CelestialBody, defaults: &CelestialBodyState) -> CelestialBodyState {
    let rotation = body.rotation.unwrap_or(defaults.rotation);
    CelestialBodyState { rotation, texture: texture(body.texture, defaults.texture.current), scale: body.scale.unwrap_or(defaults.scale) }
}

fn water_state(water: &Water, position: f32) -> WaterState {
    let defaults = water_defaults(position);
    WaterState {
        fog_color: water.fog_color.unwrap_or(defaults.fog_color),
        fog_density: water.fog_density.unwrap_or(defaults.fog_density),
        underwater_fog: water.underwater_fog.unwrap_or(defaults.underwater_fog),
        blur: water.blur.unwrap_or(defaults.blur),
        fresnel_offset: water.fresnel_offset.unwrap_or(defaults.fresnel_offset),
        fresnel_scale: water.fresnel_scale.unwrap_or(defaults.fresnel_scale),
        transparent_texture: texture(water.transparent_texture, DEFAULT_WATER_TRANSPARENT_TEXTURE),
        normal_map: texture(water.normal_map, DEFAULT_WATER_NORMAL_MAP),
        normal_scale: water.normal_scale.unwrap_or(defaults.normal_scale),
        scale_above: water.scale_above.unwrap_or(defaults.scale_above),
        scale_below: water.scale_below.unwrap_or(defaults.scale_below),
        waves: [water.waves[0].unwrap_or(defaults.waves[0]), water.waves[1].unwrap_or(defaults.waves[1])],
    }
}

fn sky_defaults(position: f32) -> SkyState {
    let azimuth = PI * position + 80.0_f32.to_radians();
    let altitude = PI * position;
    let sun_rotation = default_rotation(altitude, azimuth);
    let moon_rotation = default_rotation(altitude + PI * 0.125, azimuth + PI * 0.125);
    SkyState {
        sun_direction: direction(sun_rotation),
        moon_direction: direction(moon_rotation),
        sun: CelestialBodyState {
            rotation: sun_rotation,
            texture: TextureBlend { current: DEFAULT_SUN_TEXTURE, next: None, factor: 0.0 },
            scale: 1.0,
        },
        moon: CelestialBodyState {
            rotation: moon_rotation,
            texture: TextureBlend { current: DEFAULT_MOON_TEXTURE, next: None, factor: 0.0 },
            scale: 1.0,
        },
        sunlight: [0.7342, 0.7815, 0.8999],
        moon_brightness: 0.5,
        star_brightness: 250.0,
        clouds: CloudsState {
            color: [0.4099, 0.4099, 0.4099],
            density: [[1.0, 0.526, 1.0]; 2],
            scale: 0.4199,
            scroll_rate: [0.2, 0.01],
            shadow: 0.2699,
            variance: 0.0,
            texture: TextureBlend { current: DEFAULT_CLOUD_TEXTURE, next: None, factor: 0.0 },
        },
        haze: default_haze(),
        legacy_haze: None,
        atmosphere: AtmosphereState {
            planet_radius: 6360.0,
            bottom_radius: 6360.0,
            top_radius: 6420.0,
            sun_arc: 0.00045,
            rayleigh: vec![DensityLayerState {
                width: 0.0,
                exponential_term: 1.0,
                exponential_scale: -1.0 / 8000.0,
                linear_term: 0.0,
                constant_term: 0.0,
                anisotropy: 0.0,
            }],
            mie: vec![DensityLayerState {
                width: 0.0,
                exponential_term: 1.0,
                exponential_scale: -1.0 / 1200.0,
                linear_term: 0.0,
                constant_term: 0.0,
                anisotropy: 0.8,
            }],
            absorption: vec![
                DensityLayerState {
                    width: 25_000.0,
                    exponential_term: 0.0,
                    exponential_scale: 0.0,
                    linear_term: -1.0 / 25_000.0,
                    constant_term: -2.0 / 3.0,
                    anisotropy: 0.0,
                },
                DensityLayerState {
                    width: 0.0,
                    exponential_term: 0.0,
                    exponential_scale: 0.0,
                    linear_term: -1.0 / 15_000.0,
                    constant_term: 8.0 / 3.0,
                    anisotropy: 0.0,
                },
            ],
            moisture: 0.0,
            droplet_radius: 800.0,
            ice: 0.0,
        },
        gamma: 1.0,
        glow: [5.0, 0.001, -0.4799],
        max_y: 1605.0,
        dome_offset: 0.96,
        dome_radius: 15_000.0,
        reflection_probe_ambiance: 0.0,
        bloom: TextureBlend { current: DEFAULT_BLOOM_TEXTURE, next: None, factor: 0.0 },
        rainbow: TextureBlend { current: DEFAULT_RAINBOW_TEXTURE, next: None, factor: 0.0 },
        halo: TextureBlend { current: DEFAULT_HALO_TEXTURE, next: None, factor: 0.0 },
    }
}

fn default_haze() -> HazeState {
    HazeState {
        ambient: [0.25; 3],
        blue_density: [0.2447, 0.4487, 0.7599],
        blue_horizon: [0.4954, 0.4954, 0.6399],
        density_multiplier: 0.0001,
        distance_multiplier: 0.8,
        density: 0.7,
        horizon: 0.19,
    }
}

fn water_defaults(position: f32) -> WaterState {
    let normal_scale = 2.0 + (position * 0.5 - 0.25);
    WaterState {
        fog_color: [0.0156, 0.149, 0.2509],
        fog_density: 2.0,
        underwater_fog: 0.25,
        blur: 0.04,
        fresnel_offset: 0.5,
        fresnel_scale: 0.3999,
        transparent_texture: TextureBlend { current: DEFAULT_WATER_TRANSPARENT_TEXTURE, next: None, factor: 0.0 },
        normal_map: TextureBlend { current: DEFAULT_WATER_NORMAL_MAP, next: None, factor: 0.0 },
        normal_scale: [normal_scale; 3],
        scale_above: 0.0299,
        scale_below: 0.2,
        waves: [[1.04999, -0.42], [1.10999, -1.16]],
    }
}

fn default_rotation(azimuth: f32, altitude: f32) -> [f32; 4] {
    let direction = Vec3::new(azimuth.cos() * altitude.cos(), azimuth.sin() * altitude.cos(), altitude.sin()).normalize();
    let rotation = Quat::from_rotation_arc(Vec3::X, direction);
    rotation.to_array()
}

fn direction(rotation: [f32; 4]) -> [f32; 3] {
    let rotation = Quat::from_array(rotation).normalize();
    (rotation * Vec3::X).normalize_or_zero().to_array()
}

fn haze_state(haze: &Haze, defaults: &HazeState) -> HazeState {
    HazeState {
        ambient: haze.ambient.unwrap_or(defaults.ambient),
        blue_density: haze.blue_density.unwrap_or(defaults.blue_density),
        blue_horizon: haze.blue_horizon.unwrap_or(defaults.blue_horizon),
        density_multiplier: haze.density_multiplier.unwrap_or(defaults.density_multiplier),
        distance_multiplier: haze.distance_multiplier.unwrap_or(defaults.distance_multiplier),
        density: haze.density.unwrap_or(defaults.density),
        horizon: haze.horizon.unwrap_or(defaults.horizon),
    }
}

fn atmosphere_state(atmosphere: &Atmosphere, defaults: &AtmosphereState) -> AtmosphereState {
    AtmosphereState {
        planet_radius: atmosphere.planet_radius.unwrap_or(defaults.planet_radius),
        bottom_radius: atmosphere.bottom_radius.unwrap_or(defaults.bottom_radius),
        top_radius: atmosphere.top_radius.unwrap_or(defaults.top_radius),
        sun_arc: atmosphere.sun_arc.unwrap_or(defaults.sun_arc),
        rayleigh: density_layers(atmosphere.rayleigh.as_deref(), &defaults.rayleigh),
        mie: density_layers(atmosphere.mie.as_deref(), &defaults.mie),
        absorption: density_layers(atmosphere.absorption.as_deref(), &defaults.absorption),
        moisture: atmosphere.moisture.unwrap_or(defaults.moisture),
        droplet_radius: atmosphere.droplet_radius.unwrap_or(defaults.droplet_radius),
        ice: atmosphere.ice.unwrap_or(defaults.ice),
    }
}

fn density_layers(source: Option<&[DensityLayer]>, defaults: &[DensityLayerState]) -> Vec<DensityLayerState> {
    source.map_or_else(
        || defaults.to_vec(),
        |layers| {
            layers
                .iter()
                .enumerate()
                .map(|(index, layer)| {
                    let default = defaults.get(index).cloned().unwrap_or_default();
                    density_layer(layer, &default)
                })
                .collect()
        },
    )
}

fn density_layer(layer: &DensityLayer, defaults: &DensityLayerState) -> DensityLayerState {
    DensityLayerState {
        width: layer.width.unwrap_or(defaults.width),
        exponential_term: layer.exponential_term.unwrap_or(defaults.exponential_term),
        exponential_scale: layer.exponential_scale.unwrap_or(defaults.exponential_scale),
        linear_term: layer.linear_term.unwrap_or(defaults.linear_term),
        constant_term: layer.constant_term.unwrap_or(defaults.constant_term),
        anisotropy: layer.anisotropy.unwrap_or(defaults.anisotropy),
    }
}

impl Default for DensityLayerState {
    fn default() -> Self {
        Self { width: 0.0, exponential_term: 0.0, exponential_scale: 0.0, linear_term: 0.0, constant_term: 0.0, anisotropy: 0.0 }
    }
}

fn apply_sky_override(state: &mut SkyState, sky: &Sky) {
    if let Some(rotation) = sky.sun.rotation {
        state.sun.rotation = rotation;
        state.sun_direction = direction(rotation);
    }
    apply_texture_override(&mut state.sun.texture, sky.sun.texture);
    if let Some(scale) = sky.sun.scale {
        state.sun.scale = scale;
    }
    if let Some(rotation) = sky.moon.rotation {
        state.moon.rotation = rotation;
        state.moon_direction = direction(rotation);
    }
    apply_texture_override(&mut state.moon.texture, sky.moon.texture);
    if let Some(scale) = sky.moon.scale {
        state.moon.scale = scale;
    }
    if let Some(sunlight) = sky.sunlight {
        state.sunlight = sunlight;
    }
    if let Some(brightness) = sky.moon_brightness {
        state.moon_brightness = brightness;
    }
    if let Some(brightness) = sky.star_brightness {
        state.star_brightness = brightness;
    }
    if let Some(color) = sky.clouds.color {
        state.clouds.color = color;
    }
    for (target, value) in state.clouds.density.iter_mut().zip(sky.clouds.density) {
        if let Some(value) = value {
            *target = value;
        }
    }
    if let Some(scale) = sky.clouds.scale {
        state.clouds.scale = scale;
    }
    if let Some(rate) = sky.clouds.scroll_rate {
        state.clouds.scroll_rate = rate;
    }
    if let Some(shadow) = sky.clouds.shadow {
        state.clouds.shadow = shadow;
    }
    if let Some(variance) = sky.clouds.variance {
        state.clouds.variance = variance;
    }
    apply_texture_override(&mut state.clouds.texture, sky.clouds.texture);
    apply_haze_override(&mut state.haze, &sky.haze);
    if let Some(haze) = &sky.legacy_haze {
        let target = state.legacy_haze.get_or_insert_with(|| state.haze.clone());
        apply_haze_override(target, haze);
    }
    apply_atmosphere_override(&mut state.atmosphere, &sky.atmosphere);
    if let Some(gamma) = sky.gamma {
        state.gamma = gamma;
    }
    if let Some(glow) = sky.glow {
        state.glow = glow;
    }
    if let Some(max_y) = sky.max_y {
        state.max_y = max_y;
    }
    if let Some(offset) = sky.dome_offset {
        state.dome_offset = offset;
    }
    if let Some(radius) = sky.dome_radius {
        state.dome_radius = radius;
    }
    if let Some(ambiance) = sky.reflection_probe_ambiance {
        state.reflection_probe_ambiance = ambiance;
    }
    apply_texture_override(&mut state.bloom, sky.bloom);
    apply_texture_override(&mut state.rainbow, sky.rainbow);
    apply_texture_override(&mut state.halo, sky.halo);
}

fn apply_texture_override(state: &mut TextureBlend, value: TextureRef) {
    match value {
        TextureRef::Absent => {}
        TextureRef::Clear => *state = TextureBlend { current: None, next: None, factor: 0.0 },
        TextureRef::Asset(id) => *state = TextureBlend { current: Some(id), next: None, factor: 0.0 },
    }
}

fn apply_haze_override(state: &mut HazeState, haze: &Haze) {
    if let Some(value) = haze.ambient {
        state.ambient = value;
    }
    if let Some(value) = haze.blue_density {
        state.blue_density = value;
    }
    if let Some(value) = haze.blue_horizon {
        state.blue_horizon = value;
    }
    if let Some(value) = haze.density_multiplier {
        state.density_multiplier = value;
    }
    if let Some(value) = haze.distance_multiplier {
        state.distance_multiplier = value;
    }
    if let Some(value) = haze.density {
        state.density = value;
    }
    if let Some(value) = haze.horizon {
        state.horizon = value;
    }
}

fn apply_atmosphere_override(state: &mut AtmosphereState, atmosphere: &Atmosphere) {
    if let Some(value) = atmosphere.planet_radius {
        state.planet_radius = value;
    }
    if let Some(value) = atmosphere.bottom_radius {
        state.bottom_radius = value;
    }
    if let Some(value) = atmosphere.top_radius {
        state.top_radius = value;
    }
    if let Some(value) = atmosphere.sun_arc {
        state.sun_arc = value;
    }
    if let Some(value) = &atmosphere.rayleigh {
        state.rayleigh = override_density_layers(value, &state.rayleigh);
    }
    if let Some(value) = &atmosphere.mie {
        state.mie = override_density_layers(value, &state.mie);
    }
    if let Some(value) = &atmosphere.absorption {
        state.absorption = override_density_layers(value, &state.absorption);
    }
    if let Some(value) = atmosphere.moisture {
        state.moisture = value;
    }
    if let Some(value) = atmosphere.droplet_radius {
        state.droplet_radius = value;
    }
    if let Some(value) = atmosphere.ice {
        state.ice = value;
    }
}

fn override_density_layers(source: &[DensityLayer], current: &[DensityLayerState]) -> Vec<DensityLayerState> {
    source
        .iter()
        .enumerate()
        .map(|(index, layer)| {
            let defaults = DensityLayerState::default();
            let previous = current.get(index).unwrap_or(&defaults);
            density_layer(layer, previous)
        })
        .collect()
}

fn apply_water_override(state: &mut WaterState, water: &Water) {
    if let Some(value) = water.fog_color {
        state.fog_color = value;
    }
    if let Some(value) = water.fog_density {
        state.fog_density = value;
    }
    if let Some(value) = water.underwater_fog {
        state.underwater_fog = value;
    }
    if let Some(value) = water.blur {
        state.blur = value;
    }
    if let Some(value) = water.fresnel_offset {
        state.fresnel_offset = value;
    }
    if let Some(value) = water.fresnel_scale {
        state.fresnel_scale = value;
    }
    apply_texture_override(&mut state.transparent_texture, water.transparent_texture);
    apply_texture_override(&mut state.normal_map, water.normal_map);
    if let Some(value) = water.normal_scale {
        state.normal_scale = value;
    }
    if let Some(value) = water.scale_above {
        state.scale_above = value;
    }
    if let Some(value) = water.scale_below {
        state.scale_below = value;
    }
    for (target, value) in state.waves.iter_mut().zip(water.waves) {
        if let Some(value) = value {
            *target = value;
        }
    }
}

fn blend_sky(start: &SkyState, end: &SkyState, factor: f32) -> SkyState {
    if factor >= 1.0 {
        return end.clone();
    }
    let mut clouds = CloudsState {
        color: blend_array(start.clouds.color, end.clouds.color, factor),
        density: [
            blend_array(start.clouds.density[0], end.clouds.density[0], factor),
            blend_array(start.clouds.density[1], end.clouds.density[1], factor),
        ],
        scale: lerp(start.clouds.scale, end.clouds.scale, factor),
        scroll_rate: blend_array(start.clouds.scroll_rate, end.clouds.scroll_rate, factor),
        shadow: lerp(start.clouds.shadow, end.clouds.shadow, factor),
        variance: lerp(start.clouds.variance, end.clouds.variance, factor),
        texture: blend_texture(start.clouds.texture, end.clouds.texture, factor),
    };
    let cloud_start = texture_at(start.clouds.texture);
    let cloud_end = texture_at(end.clouds.texture);
    if cloud_start.is_some() && cloud_end.is_none() {
        clouds.shadow = lerp(start.clouds.shadow, 0.0, factor);
        clouds.texture = TextureBlend { current: cloud_start, next: cloud_start, factor };
    } else if cloud_start.is_none() && cloud_end.is_some() {
        clouds.shadow = lerp(0.0, end.clouds.shadow, factor);
        clouds.texture = TextureBlend { current: cloud_end, next: cloud_end, factor };
    }
    let sun = blend_body(&start.sun, &end.sun, factor);
    let moon = blend_body(&start.moon, &end.moon, factor);
    SkyState {
        sun_direction: direction(sun.rotation),
        moon_direction: direction(moon.rotation),
        sun,
        moon,
        sunlight: blend_array(start.sunlight, end.sunlight, factor),
        moon_brightness: lerp(start.moon_brightness, end.moon_brightness, factor),
        star_brightness: lerp(start.star_brightness, end.star_brightness, factor),
        clouds,
        haze: blend_haze(&start.haze, &end.haze, factor),
        legacy_haze: match (&start.legacy_haze, &end.legacy_haze) {
            (Some(start), Some(end)) => Some(blend_haze(start, end, factor)),
            (Some(start), None) => Some(blend_haze(start, &default_haze(), factor)),
            (None, Some(end)) => Some(blend_haze(&default_haze(), end, factor)),
            (None, None) => None,
        },
        atmosphere: blend_atmosphere(&start.atmosphere, &end.atmosphere, factor),
        gamma: lerp(start.gamma, end.gamma, factor),
        glow: blend_array(start.glow, end.glow, factor),
        max_y: lerp(start.max_y, end.max_y, factor),
        dome_offset: lerp(start.dome_offset, end.dome_offset, factor),
        dome_radius: lerp(start.dome_radius, end.dome_radius, factor),
        reflection_probe_ambiance: lerp(start.reflection_probe_ambiance, end.reflection_probe_ambiance, factor),
        bloom: blend_texture(start.bloom, end.bloom, factor),
        rainbow: blend_texture(start.rainbow, end.rainbow, factor),
        halo: blend_texture(start.halo, end.halo, factor),
    }
}

fn blend_body(start: &CelestialBodyState, end: &CelestialBodyState, factor: f32) -> CelestialBodyState {
    let rotation = blend_rotation(start.rotation, end.rotation, factor);
    CelestialBodyState { rotation, texture: blend_texture(start.texture, end.texture, factor), scale: lerp(start.scale, end.scale, factor) }
}

fn blend_rotation(start: [f32; 4], end: [f32; 4], factor: f32) -> [f32; 4] {
    let start = Quat::from_array(start).normalize();
    let end = Quat::from_array(end).normalize();
    start.slerp(end, factor).normalize().to_array()
}

fn blend_haze(start: &HazeState, end: &HazeState, factor: f32) -> HazeState {
    HazeState {
        ambient: blend_array(start.ambient, end.ambient, factor),
        blue_density: blend_array(start.blue_density, end.blue_density, factor),
        blue_horizon: blend_array(start.blue_horizon, end.blue_horizon, factor),
        density_multiplier: lerp(start.density_multiplier, end.density_multiplier, factor),
        distance_multiplier: lerp(start.distance_multiplier, end.distance_multiplier, factor),
        density: lerp(start.density, end.density, factor),
        horizon: lerp(start.horizon, end.horizon, factor),
    }
}

fn blend_atmosphere(start: &AtmosphereState, end: &AtmosphereState, factor: f32) -> AtmosphereState {
    AtmosphereState {
        planet_radius: lerp(start.planet_radius, end.planet_radius, factor),
        bottom_radius: lerp(start.bottom_radius, end.bottom_radius, factor),
        top_radius: lerp(start.top_radius, end.top_radius, factor),
        sun_arc: lerp(start.sun_arc, end.sun_arc, factor),
        rayleigh: blend_layers(&start.rayleigh, &end.rayleigh, factor),
        mie: blend_layers(&start.mie, &end.mie, factor),
        absorption: blend_layers(&start.absorption, &end.absorption, factor),
        moisture: lerp(start.moisture, end.moisture, factor),
        droplet_radius: lerp(start.droplet_radius, end.droplet_radius, factor),
        ice: lerp(start.ice, end.ice, factor),
    }
}

fn blend_layers(start: &[DensityLayerState], end: &[DensityLayerState], factor: f32) -> Vec<DensityLayerState> {
    let default = DensityLayerState::default();
    (0..start.len().max(end.len()))
        .map(|index| {
            let start = start.get(index).unwrap_or(&default);
            let end = end.get(index).unwrap_or(&default);
            DensityLayerState {
                width: lerp(start.width, end.width, factor),
                exponential_term: lerp(start.exponential_term, end.exponential_term, factor),
                exponential_scale: lerp(start.exponential_scale, end.exponential_scale, factor),
                linear_term: lerp(start.linear_term, end.linear_term, factor),
                constant_term: lerp(start.constant_term, end.constant_term, factor),
                anisotropy: lerp(start.anisotropy, end.anisotropy, factor),
            }
        })
        .collect()
}

fn blend_water(start: &WaterState, end: &WaterState, factor: f32) -> WaterState {
    if factor >= 1.0 {
        return end.clone();
    }
    WaterState {
        fog_color: blend_array(start.fog_color, end.fog_color, factor),
        fog_density: lerp(start.fog_density, end.fog_density, factor),
        underwater_fog: lerp(start.underwater_fog, end.underwater_fog, factor),
        blur: lerp(start.blur, end.blur, factor),
        fresnel_offset: lerp(start.fresnel_offset, end.fresnel_offset, factor),
        fresnel_scale: lerp(start.fresnel_scale, end.fresnel_scale, factor),
        transparent_texture: blend_texture(start.transparent_texture, end.transparent_texture, factor),
        normal_map: blend_texture(start.normal_map, end.normal_map, factor),
        normal_scale: blend_array(start.normal_scale, end.normal_scale, factor),
        scale_above: lerp(start.scale_above, end.scale_above, factor),
        scale_below: lerp(start.scale_below, end.scale_below, factor),
        waves: [blend_array(start.waves[0], end.waves[0], factor), blend_array(start.waves[1], end.waves[1], factor)],
    }
}

fn lerp(start: f32, end: f32, factor: f32) -> f32 {
    ((1.0 - f64::from(factor)) * f64::from(start) + f64::from(factor) * f64::from(end)) as f32
}

fn blend_array<const N: usize>(start: [f32; N], end: [f32; N], factor: f32) -> [f32; N] {
    std::array::from_fn(|index| lerp(start[index], end[index], factor))
}

#[cfg(test)]
#[path = "../environment_tests.rs"]
mod tests;
