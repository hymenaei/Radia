use math::srgb_to_linear;

use crate::environment::Evaluated;

pub(crate) const SKY_DOME_RADIUS: f32 = 15_000.0;
pub(crate) const SKY_DOME_OFFSET: f32 = 0.96;
pub(crate) const SKY_DOME_HEIGHT: f32 = SKY_DOME_RADIUS * SKY_DOME_OFFSET;
pub(crate) const ATMOSPHERE_FLOATS: usize = 48;
const LEGACY_SUNLIGHT_SCALE: f32 = 1.5;
const LEGACY_AMBIENT_SCALE: f32 = 1.5;

pub(crate) fn atmosphere_values(environment: Option<&Evaluated>) -> [f32; ATMOSPHERE_FLOATS] {
    let mut values = [0.0; ATMOSPHERE_FLOATS];
    values[40..44].copy_from_slice(&[SKY_DOME_RADIUS, SKY_DOME_HEIGHT, 0.0, 0.0]);
    let Some(environment) = environment else { return values };
    let haze = &environment.sky.haze;
    let sun_up = environment.sky.sun_direction[2] >= 0.0;
    let moon_up = environment.sky.moon_direction[2] >= 0.0;
    let light_direction = if sun_up { environment.sky.sun_direction } else { environment.sky.moon_direction };
    let mut light_norm = light_direction;
    light_norm[2] = light_norm[2].max(-0.1);
    let glow_factor = if sun_up {
        1.0
    } else if moon_up {
        environment.sky.moon_brightness * 0.25
    } else {
        0.0
    };
    let light_color = environment.sky.sunlight;
    let light_color_length = (light_color[0] * light_color[0] + light_color[1] * light_color[1] + light_color[2] * light_color[2]).sqrt();
    let horizontal_light_squared = light_direction[0] * light_direction[0] + light_direction[1] * light_direction[1];
    let water_specular_scale = if light_color_length > 0.0 { (1.5 + 6.0 * horizontal_light_squared) / light_color_length } else { 0.0 };
    let water_specular = light_color.map(|component| component * water_specular_scale);
    let sky_hdr_scale = sky_hdr_scale(Some(environment));
    let tonemap_mix = if environment.sky.reflection_probe_ambiance.is_some_and(|ambiance| ambiance > 0.0) { 1.0 } else { 0.0 };
    let source = [
        [haze.blue_horizon[0], haze.blue_horizon[1], haze.blue_horizon[2], 0.0],
        [haze.blue_density[0], haze.blue_density[1], haze.blue_density[2], 0.0],
        [haze.horizon, haze.density, haze.density_multiplier, environment.sky.max_y],
        [environment.sky.sunlight[0], environment.sky.sunlight[1], environment.sky.sunlight[2], if sun_up { 1.0 } else { 0.0 }],
        [light_direction[0], light_direction[1], light_direction[2], 0.0],
        [haze.ambient[0], haze.ambient[1], haze.ambient[2], tonemap_mix],
        [environment.sky.glow[0], environment.sky.glow[1], environment.sky.glow[2], environment.sky.clouds.shadow],
        [haze.distance_multiplier, 0.0, sky_hdr_scale, glow_factor],
        [light_norm[0], light_norm[1], light_norm[2], 1.0 / light_norm[2].max(0.3)],
        [water_specular[0], water_specular[1], water_specular[2], 0.0],
    ];
    for (target, value) in values.iter_mut().zip(source.into_iter().flatten()) {
        *target = value;
    }
    values[44..48].copy_from_slice(&[
        legacy_ambient_irradiance(haze.ambient, environment.sky.clouds.shadow),
        LEGACY_SUNLIGHT_SCALE,
        0.0,
        0.0,
    ]);
    values
}

fn legacy_ambient_irradiance(ambient: [f32; 3], cloud_shadow: f32) -> f32 {
    let cloud_shadow = cloud_shadow.clamp(0.0, 1.0);
    let cloud_adjusted = ambient.map(|component| {
        let component = component.max(0.0);
        component + (1.0 - component) * cloud_shadow * 0.5
    });
    let amblit = cloud_adjusted.map(|component| component.max(0.0).powf(0.9) * 0.57);
    let linear = amblit.map(srgb_to_linear);
    (linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722) * LEGACY_AMBIENT_SCALE
}

pub(crate) fn sky_hdr_scale(environment: Option<&Evaluated>) -> f32 {
    let Some(environment) = environment else { return 1.0 };
    if environment.sky.reflection_probe_ambiance.is_some_and(|ambiance| ambiance > 0.0) { environment.sky.gamma.sqrt() * 2.0 } else { 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::{Schedule, Sources, evaluate};

    #[test]
    fn default_sky_uniforms_match_windlight_defaults() {
        let sources = Sources {
            base: None,
            schedule: Schedule::default(),
            experience_day: None,
            experience_sky: None,
            experience_water: None,
            sky_override: None,
            water_override: None,
            local: None,
        };
        let environment = evaluate(&sources, 0.0, 0.0);
        let values = atmosphere_values(Some(&environment));
        assert_eq!(&values[0..3], &[0.4954, 0.4954, 0.6399]);
        assert_eq!(&values[4..7], &[0.2447, 0.4487, 0.7599]);
        assert_eq!(&values[8..12], &[0.19, 0.7, 0.0001, 1605.0]);
        assert_eq!(&values[20..24], &[0.25, 0.25, 0.25, 0.0]);
        assert_eq!(&values[24..28], &[5.0, 0.001, -0.4799, 0.2699]);
        assert_eq!(&values[28..32], &[0.8, 0.0, 1.0, 1.0]);
        assert_eq!(&values[32..36], &[0.17364812, 0.0, 0.9848078, 1.0154265]);
        assert_eq!(&values[36..40], &[0.8816109, 0.9384076, 1.0805798, 0.0]);
        assert_eq!(&values[40..44], &[15_000.0, 14_400.0, 0.0, 0.0]);
        assert!((values[44] - 0.06071076).abs() < 1e-6);
        assert_eq!(&values[45..48], &[1.5, 0.0, 0.0]);
    }

    #[test]
    fn legacy_ambient_uses_cloud_adjustment_and_linear_luminance() {
        assert!((legacy_ambient_irradiance([0.25; 3], 0.0) - 0.03434571).abs() < 1e-6);
        assert!((legacy_ambient_irradiance([0.25; 3], 0.5) - 0.08944999).abs() < 1e-6);
        assert_eq!(legacy_ambient_irradiance([0.25; 3], 2.0), legacy_ambient_irradiance([0.25; 3], 1.0));
    }

    #[test]
    fn legacy_surface_light_uses_the_moon_when_both_bodies_are_below_horizon() {
        let sources = Sources {
            base: None,
            schedule: Schedule::default(),
            experience_day: None,
            experience_sky: None,
            experience_water: None,
            sky_override: None,
            water_override: None,
            local: None,
        };
        let mut environment = evaluate(&sources, 0.0, 0.0);
        environment.sky.sun_direction = [0.0, 0.0, -1.0];
        environment.sky.moon_direction = [0.6, 0.0, -0.8];
        environment.sky.sunlight = [0.2, 0.3, 0.4];

        let values = atmosphere_values(Some(&environment));

        assert_eq!(&values[12..16], &[0.2, 0.3, 0.4, 0.0]);
        assert_eq!(&values[16..20], &[0.6, 0.0, -0.8, 0.0]);
    }

    #[test]
    fn legacy_skies_disable_tonemapping_and_probe_skies_enable_it() {
        let sources = Sources {
            base: None,
            schedule: Schedule::default(),
            experience_day: None,
            experience_sky: None,
            experience_water: None,
            sky_override: None,
            water_override: None,
            local: None,
        };
        let legacy = evaluate(&sources, 0.0, 0.0);
        assert_eq!(atmosphere_values(Some(&legacy))[23], 0.0);
        let mut hdr = legacy;
        hdr.sky.reflection_probe_ambiance = Some(0.5);
        assert_eq!(atmosphere_values(Some(&hdr))[23], 1.0);
        assert_eq!(atmosphere_values(Some(&hdr))[30], 2.0);
    }
}
