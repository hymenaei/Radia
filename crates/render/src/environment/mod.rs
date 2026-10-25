mod runtime;
mod transition;

use std::f32::consts::PI;

use assets::{CelestialBody, DayCycle, EnvironmentDescription, EnvironmentSettings, Haze, Keyframe, Sky, TextureRef, Water};
use math::{Quat, Vec3};
use utils::AssetId;
use uuid::Uuid;

pub use runtime::ActiveEnvironment;
pub use transition::Transition;

const DEFAULT_DAY_LENGTH: f64 = 14_400.0;
const MIN_DAY_LENGTH: f64 = 14_400.0;
const MAX_DAY_LENGTH: f64 = 604_800.0;
const DEFAULT_DAY_OFFSET: f64 = 57_600.0;
const MAX_DAY_OFFSET: f64 = 86_400.0;

pub(crate) const DEFAULT_SUN_TEXTURE: Option<AssetId> = None;
pub(crate) const DEFAULT_MOON_TEXTURE: Option<AssetId> = Some(AssetId::new(Uuid::from_u128(0xd07f6eedb96a47cdb51d400ad4a1c428)));
pub(crate) const DEFAULT_CLOUD_TEXTURE: Option<AssetId> = Some(AssetId::new(Uuid::from_u128(0x1dc1368fe8fef02da08d9d9f11c1af6b)));
pub(crate) const DEFAULT_WATER_NORMAL_MAP: Option<AssetId> = Some(AssetId::new(Uuid::from_u128(0x822ded499a6cf61ccb896df54f42cdf4)));

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
    pub gamma: f32,
    pub glow: [f32; 3],
    pub max_y: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WaterState {
    pub fog_color: [f32; 3],
    pub fog_density: f32,
    pub underwater_fog: f32,
    pub blur: f32,
    pub fresnel_offset: f32,
    pub fresnel_scale: f32,
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
    let (sky, sky_track, sky_cycles) = sky_source
        .and_then(|source| sample_sky(source, day_position, altitude, schedule.altitudes))
        .unwrap_or_else(|| (sky_defaults(0.0), None, false));
    let (water, water_cycles) =
        water_source.and_then(|source| sample_water(source, day_position)).unwrap_or_else(|| (water_defaults(0.0), false));
    let mut sky = sky;
    let mut water = water;
    if let Some(override_sky) = sources.sky_override {
        overlay(&mut sky, override_sky);
    }
    if let Some(override_water) = sources.water_override {
        overlay_water(&mut water, override_water);
    }
    Evaluated { sky, water, sky_track, day_position, cycle_length: (sky_cycles || water_cycles).then_some(schedule.day_length) }
}

fn day_position(schedule: Schedule, utc_seconds: f64) -> f64 {
    let utc_seconds = if utc_seconds.is_finite() { utc_seconds } else { 0.0 };
    let position = (utc_seconds + schedule.day_offset).rem_euclid(schedule.day_length) / schedule.day_length;
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
            Some((sample_sky_track(track, position as f32)?, Some(track_number), track.len() > 1))
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
        EnvironmentSettings::DayCycle(day) => Some((sample_water_track(day.water(), position as f32)?, day.water().len() > 1)),
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

fn sky_state(sky: &Sky, position: f32) -> SkyState {
    let mut state = sky_defaults(position);
    overlay(&mut state, sky);
    state
}

fn overlay(state: &mut SkyState, sky: &Sky) {
    overlay_body(&mut state.sun, &sky.sun);
    overlay_body(&mut state.moon, &sky.moon);
    if let Some(value) = sky.sunlight {
        state.sunlight = value;
    }
    if let Some(value) = sky.moon_brightness {
        state.moon_brightness = value;
    }
    if let Some(value) = sky.star_brightness {
        state.star_brightness = value;
    }
    if let Some(value) = sky.clouds.color {
        state.clouds.color = value;
    }
    for (target, value) in state.clouds.density.iter_mut().zip(sky.clouds.density) {
        if let Some(value) = value {
            *target = value;
        }
    }
    if let Some(value) = sky.clouds.scale {
        state.clouds.scale = value;
    }
    if let Some(value) = sky.clouds.scroll_rate {
        state.clouds.scroll_rate = value;
    }
    if let Some(value) = sky.clouds.shadow {
        state.clouds.shadow = value;
    }
    if let Some(value) = sky.clouds.variance {
        state.clouds.variance = value;
    }
    overlay_texture(&mut state.clouds.texture, sky.clouds.texture);
    overlay_haze(&mut state.haze, &sky.haze);
    if let Some(value) = sky.gamma {
        state.gamma = value;
    }
    if let Some(value) = sky.glow {
        state.glow = value;
    }
    if let Some(value) = sky.max_y {
        state.max_y = value;
    }
    state.sun_direction = direction(state.sun.rotation);
    state.moon_direction = direction(state.moon.rotation);
}

fn overlay_body(state: &mut CelestialBodyState, body: &CelestialBody) {
    if let Some(value) = body.rotation {
        state.rotation = value;
    }
    overlay_texture(&mut state.texture, body.texture);
    if let Some(value) = body.scale {
        state.scale = value;
    }
}

fn overlay_haze(state: &mut HazeState, haze: &Haze) {
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

fn overlay_water(state: &mut WaterState, water: &Water) {
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
    overlay_texture(&mut state.normal_map, water.normal_map);
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

fn overlay_texture(state: &mut TextureBlend, value: TextureRef) {
    match value {
        TextureRef::Absent => {}
        TextureRef::Clear => *state = TextureBlend { current: None, next: None, factor: 0.0 },
        TextureRef::Asset(id) => *state = TextureBlend { current: Some(id), next: None, factor: 0.0 },
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
        gamma: 1.0,
        glow: [5.0, 0.001, -0.4799],
        max_y: 1605.0,
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

fn water_state(water: &Water, position: f32) -> WaterState {
    let mut state = water_defaults(position);
    overlay_water(&mut state, water);
    state
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

fn texture_at(value: TextureBlend) -> Option<AssetId> {
    if value.factor < 0.5 { value.current } else { value.next.or(value.current) }
}

fn blend_texture(start: TextureBlend, end: TextureBlend, factor: f32) -> TextureBlend {
    let current = texture_at(start);
    let next = texture_at(end);
    if current == next { TextureBlend { current, next: None, factor: 0.0 } } else { TextureBlend { current, next, factor } }
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
        gamma: lerp(start.gamma, end.gamma, factor),
        glow: blend_array(start.glow, end.glow, factor),
        max_y: lerp(start.max_y, end.max_y, factor),
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
mod tests;
