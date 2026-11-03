use std::f64::consts::TAU;

use math::DVec3;

use crate::environment::Evaluated;

pub(crate) const DEFAULT_WATER_COLOR: [f32; 3] = [0.0156, 0.149, 0.2509];
pub(crate) const DEFAULT_WATER_FRESNEL: [f32; 4] = [0.5, 0.3999, 0.0, 0.0];
pub(crate) const DEFAULT_WATER_NORMAL_SCALE: f32 = 1.75;
pub(crate) const DEFAULT_WATER_BLUR: f32 = 0.04;
pub(crate) const DEFAULT_WATER_WAVES: [f32; 4] = [1.04999, -0.42, 1.10999, -1.16];

const WATER_UNIFORM_FLOATS: usize = 36;

#[derive(Clone, Copy)]
struct WaterUvOrigins {
    sweep: [f32; 2],
    big: [f32; 2],
    first: [f32; 2],
    second: [f32; 2],
    lattice: [f32; 2],
}

pub(crate) fn water_surface_values(
    environment: Option<&Evaluated>,
    camera_position: DVec3,
    camera_relative_position: DVec3,
    water_height: Option<f64>,
    time: [f32; 4],
    normal_blend: f32,
    normal_present: bool,
) -> [f32; WATER_UNIFORM_FLOATS] {
    let water = environment.map(|environment| &environment.water);
    let fog_color = water.map_or(DEFAULT_WATER_COLOR, |water| water.fog_color);
    let density = water.map_or(2.0, |water| water.fog_density);
    let underwater_modifier = water.map_or(0.25, |water| water.underwater_fog);
    let underwater = water_height.is_some_and(|height| camera_position.z <= height);
    let fog_density = modified_water_fog_density(density, underwater_modifier, underwater);
    let fog_ks = water_fog_ks(environment);
    let plane_w = water_height.map_or(0.0, |height| (camera_position.z - height) as f32);
    let normal_scale = water.map_or([DEFAULT_WATER_NORMAL_SCALE; 3], |water| water.normal_scale);
    let blur = water.map_or(DEFAULT_WATER_BLUR, |water| water.blur).max(0.0) * 2.0;
    let fresnel = water.map_or(DEFAULT_WATER_FRESNEL, |water| [water.fresnel_offset, water.fresnel_scale, 0.0, 0.0]);
    let waves = water.map_or(DEFAULT_WATER_WAVES, |water| [water.waves[0][0], water.waves[0][1], water.waves[1][0], water.waves[1][1]]);
    let origin = water_uv_origin(camera_position);
    let mut values = [0.0; WATER_UNIFORM_FLOATS];
    values[..4].copy_from_slice(&[fog_color[0], fog_color[1], fog_color[2], if water_height.is_some() { 1.0 } else { 0.0 }]);
    values[4..8].copy_from_slice(&[fog_density, fog_ks, plane_w, if underwater { 1.0 } else { 0.0 }]);
    values[8..12].copy_from_slice(&[normal_scale[0], normal_scale[1], normal_scale[2], blur]);
    values[12..16].copy_from_slice(&[fresnel[0], fresnel[1], normal_blend, if normal_present { 1.0 } else { 0.0 }]);
    values[16..20].copy_from_slice(&waves);
    values[20..24].copy_from_slice(&[time[0], origin.sweep[0], origin.sweep[1], 0.0]);
    values[24..28].copy_from_slice(&[origin.big[0], origin.big[1], origin.first[0], origin.first[1]]);
    values[28..32].copy_from_slice(&[origin.second[0], origin.second[1], origin.lattice[0], origin.lattice[1]]);
    values[32..36].copy_from_slice(&[
        camera_relative_position.x as f32,
        camera_relative_position.y as f32,
        camera_relative_position.z as f32,
        0.0,
    ]);
    values
}

pub(crate) fn modified_water_fog_density(density: f32, modifier: f32, underwater: bool) -> f32 {
    if !underwater || modifier <= 0.0 {
        return density.max(0.0);
    }
    let modifier = modifier.clamp(0.0, 10.0);
    let density = if density < 0.0 && modifier != modifier.round() { 1.0 } else { density };
    density.powf(modifier).clamp(0.0, f32::MAX)
}

pub(crate) fn water_fog_values(environment: Option<&Evaluated>, camera_position: DVec3, water_height: Option<f64>) -> [f32; 8] {
    water_surface_values(environment, camera_position, DVec3::ZERO, water_height, [0.0; 4], 0.0, false)[..8]
        .try_into()
        .expect("water fog uses two four-component values")
}

fn water_uv_origin(position: DVec3) -> WaterUvOrigins {
    let phase = |coordinate: f64, frequency: f64| (coordinate * frequency).rem_euclid(1.0) as f32;
    WaterUvOrigins {
        sweep: [(position.x.rem_euclid(TAU / 0.08)) as f32, (position.y.rem_euclid(TAU / 0.02)) as f32],
        big: [phase(position.x, 0.04), phase(position.y, 0.04)],
        first: [phase(position.x, 0.45), phase(position.y, 0.9)],
        second: [phase(position.x, 0.1), phase(position.y, 0.2)],
        lattice: [position.x.rem_euclid(256.0) as f32, position.y.rem_euclid(256.0) as f32],
    }
}

fn water_fog_ks(environment: Option<&Evaluated>) -> f32 {
    let Some(environment) = environment else { return 1.0 / 0.3 };
    let sun_up = environment.sky.sun_direction[2] >= 0.0;
    let moon_up = environment.sky.moon_direction[2] >= 0.0;
    let light_direction = if sun_up {
        environment.sky.sun_direction
    } else if moon_up {
        environment.sky.moon_direction
    } else {
        [0.0, 0.0, -1.0]
    };
    1.0 / light_direction[0].max(0.3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::{Schedule, Sources, evaluate};

    #[test]
    fn water_uniforms_match_rules() {
        let environment = evaluate(
            &Sources {
                base: None,
                schedule: Schedule::default(),
                experience_day: None,
                experience_sky: None,
                experience_water: None,
                sky_override: None,
                water_override: None,
                local: None,
            },
            0.0,
            0.0,
        );
        let above = water_surface_values(
            Some(&environment),
            DVec3::new(25.0, 10.0, 20.0),
            DVec3::new(5.0, 2.0, 3.0),
            Some(10.0),
            [5.0; 4],
            0.25,
            true,
        );
        assert_eq!(&above[4..8], &[2.0, 1.0 / environment.sky.sun_direction[0].max(0.3), 10.0, 0.0]);
        assert_eq!(&above[8..16], &[1.75, 1.75, 1.75, 0.08, 0.5, 0.3999, 0.25, 1.0]);
        assert_eq!(&above[16..24], &[1.04999, -0.42, 1.10999, -1.16, 5.0, 25.0, 10.0, 0.0]);
        assert_eq!(&above[24..32], &[0.0, 0.4, 0.25, 0.0, 0.5, 0.0, 25.0, 10.0]);
        assert_eq!(&above[32..36], &[5.0, 2.0, 3.0, 0.0]);
        let below = water_surface_values(
            Some(&environment),
            DVec3::new(25.0, 10.0, 0.0),
            DVec3::new(5.0, 2.0, -10.0),
            Some(10.0),
            [0.0; 4],
            0.0,
            false,
        );
        assert!((below[4] - 2.0_f32.powf(0.25)).abs() < 0.000001);
        assert_eq!(below[6], -10.0);
        assert_eq!(below[7], 1.0);
        assert_eq!(modified_water_fog_density(2.0, 0.25, false), 2.0);
        assert_eq!(modified_water_fog_density(2.0, 0.5, true), 2.0_f32.sqrt());
        assert_eq!(modified_water_fog_density(2.0, 2.0, true), 4.0);
        assert_eq!(modified_water_fog_density(-2.0, 0.5, true), 1.0);
        assert_eq!(modified_water_fog_density(-2.0, 1.5, true), 1.0);
        assert_eq!(modified_water_fog_density(-2.0, 2.0, true), 4.0);
        assert_eq!(modified_water_fog_density(2.0, 11.0, true), 1024.0);
        assert_eq!(modified_water_fog_density(2.0, -1.0, true), 2.0);
        assert_eq!(modified_water_fog_density(f32::MAX, 2.0, true), f32::MAX);
        assert_eq!(modified_water_fog_density(-f32::MAX, 3.0, true), 0.0);
        assert_eq!(modified_water_fog_density(-f32::MAX, 3.0, false), 0.0);
        assert_eq!(modified_water_fog_density(-2.0, 0.0, true), 0.0);
    }

    #[test]
    fn camera_plane_equality_is_underwater() {
        let environment = evaluate(
            &Sources {
                base: None,
                schedule: Schedule::default(),
                experience_day: None,
                experience_sky: None,
                experience_water: None,
                sky_override: None,
                water_override: None,
                local: None,
            },
            0.0,
            0.0,
        );
        let values =
            water_surface_values(Some(&environment), DVec3::new(1.0e12, -2.0e12, 20.0), DVec3::ZERO, Some(20.0), [0.0; 4], 0.0, false);
        assert_eq!(values[4], 2.0_f32.powf(0.25));
        assert_eq!(values[6], 0.0);
        assert_eq!(values[7], 1.0);
        assert!(values[20..30].iter().all(|value| value.is_finite()));
    }

    #[test]
    fn water_fog_exponent_uses_the_converted_light_z_axis() {
        let mut environment = evaluate(
            &Sources {
                base: None,
                schedule: Schedule::default(),
                experience_day: None,
                experience_sky: None,
                experience_water: None,
                sky_override: None,
                water_override: None,
                local: None,
            },
            0.0,
            0.0,
        );
        environment.sky.sun_direction = [0.0, 0.0, 1.0];
        environment.sky.moon_direction = [0.0, 0.0, -1.0];
        assert!((water_fog_ks(Some(&environment)) - 1.0 / 0.3).abs() < 0.000001);

        environment.sky.sun_direction = [0.6, 0.2, 0.7];
        assert!((water_fog_ks(Some(&environment)) - 1.0 / 0.6).abs() < 0.000001);

        environment.sky.sun_direction = [0.1, 0.2, -0.4];
        environment.sky.moon_direction = [0.6, 0.2, 0.7];
        assert!((water_fog_ks(Some(&environment)) - 1.0 / 0.6).abs() < 0.000001);

        environment.sky.moon_direction = [-0.6, 0.2, -0.7];
        assert!((water_fog_ks(Some(&environment)) - 1.0 / 0.3).abs() < 0.000001);
    }

    #[test]
    fn water_wave_lattice_phase_keeps_global_region_alignment() {
        let values = water_surface_values(None, DVec3::new(1.0e12 + 128.0, -2.0e12 + 128.0, 0.0), DVec3::ZERO, None, [0.0; 4], 0.0, false);
        assert_eq!(values[30], (1.0e12_f64 + 128.0).rem_euclid(256.0) as f32);
        assert_eq!(values[31], (-2.0e12_f64 + 128.0).rem_euclid(256.0) as f32);
    }

    #[test]
    fn firestorm_region_quad_interpolates_the_swept_center() {
        let sweep = |x: f64, y: f64| x + ((x * 0.08).cos() + (y * 0.02).sin()) * 6.0;
        let source_center = (sweep(0.0, 0.0) + sweep(256.0, 256.0)) * 0.5;
        let per_fragment = sweep(128.0, 128.0);
        assert!((source_center - 128.066951411123).abs() < 0.0000001);
        assert!((source_center - per_fragment - 0.885059550074).abs() < 0.0000001);
    }

    #[test]
    fn firestorm_water_fog_vectors_cover_a_grazing_high_altitude_ray() {
        let intersection = 20.0_f32 / 0.01;
        let original_endpoint = 2048.0 - intersection;
        let extended_endpoint = intersection + 2048.0 - intersection;
        assert_eq!(intersection, 2000.0);
        assert_eq!(original_endpoint, 48.0);
        assert_eq!(extended_endpoint, 2048.0);
        assert!((firestorm_fog_vector(original_endpoint)[0] - 0.9067385).abs() < 0.000001);
        assert!((firestorm_fog_vector(original_endpoint)[1] - 0.1437816).abs() < 0.000001);
        assert!((firestorm_fog_vector(extended_endpoint)[0] - 0.990_324).abs() < 0.000001);
        assert!(firestorm_fog_vector(extended_endpoint)[1] < 0.000001);
    }

    fn firestorm_fog_vector(length_in_water: f32) -> [f32; 2] {
        let density = 2.0_f32;
        let light_exponent = 1.0 / 0.3;
        let plane_slope = 0.01_f32;
        let t1 = -density * 0.98_f32.powf(light_exponent * 0.0);
        let t2 = density + light_exponent * plane_slope;
        let t3 = 0.98_f32.powf(t2 * length_in_water) - 1.0;
        [(t1 / t2 * t3).min(1.0).powf(1.0 / 1.7), 0.98_f32.powf(length_in_water * density)]
    }
}
