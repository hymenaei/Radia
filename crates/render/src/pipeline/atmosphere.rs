use crate::environment::Evaluated;

pub(crate) const ATMOSPHERE_FLOATS: usize = 32;

pub(crate) fn atmosphere_values(environment: Option<&Evaluated>, altitude: f32) -> [f32; ATMOSPHERE_FLOATS] {
    let mut values = [0.0; ATMOSPHERE_FLOATS];
    let Some(environment) = environment else { return values };
    let haze = &environment.sky.haze;
    let sun_up = environment.sky.sun_direction[2] >= 0.0;
    let moon_up = environment.sky.moon_direction[2] >= 0.0;
    let light_direction = if sun_up {
        environment.sky.sun_direction
    } else if moon_up {
        environment.sky.moon_direction
    } else {
        [0.0, 0.0, -1.0]
    };
    let glow_factor = if sun_up {
        1.0
    } else if moon_up {
        environment.sky.moon_brightness * 0.25
    } else {
        0.0
    };
    let values_source = [
        [haze.blue_horizon[0], haze.blue_horizon[1], haze.blue_horizon[2], 0.0],
        [haze.blue_density[0], haze.blue_density[1], haze.blue_density[2], 0.0],
        [haze.horizon, haze.density, haze.density_multiplier, environment.sky.max_y],
        [environment.sky.sunlight[0], environment.sky.sunlight[1], environment.sky.sunlight[2], if sun_up { 1.0 } else { 0.0 }],
        [light_direction[0], light_direction[1], light_direction[2], 0.0],
        [haze.ambient[0], haze.ambient[1], haze.ambient[2], 1.0],
        [environment.sky.glow[0], environment.sky.glow[1], environment.sky.glow[2], environment.sky.clouds.shadow],
        [haze.distance_multiplier, altitude, environment.sky.gamma, glow_factor],
    ];
    for (target, value) in values.iter_mut().zip(values_source.into_iter().flatten()) {
        *target = value;
    }
    values
}

#[cfg(test)]
fn horizon_color(direction: [f64; 3], atmosphere: &[f32; ATMOSPHERE_FLOATS]) -> [f64; 3] {
    if atmosphere[23] <= 0.0 {
        return [0.024; 3];
    }
    let max_y = f64::from(atmosphere[11]).max(0.0);
    let altitude = f64::from(atmosphere[29]).clamp(-32000.0, max_y);
    let mut relative_position = normalize([direction[0], direction[2], direction[1]]).map(|component| component * 15000.0);
    relative_position[1] += 50.0 - altitude;
    let ray = normalize(relative_position);
    let light_world = normalize([f64::from(atmosphere[16]), f64::from(atmosphere[18]), f64::from(atmosphere[17])]);
    let light = light_world;
    let up = ray[1];
    let below = 1.0 - smoothstep(-0.05, 0.05, up);
    let rel_pos_len = (max_y * (1.0 - below) + 32000.0 * below) / (up * up + 0.000001).sqrt();
    let haze_density = f64::from(atmosphere[9]);
    let density_multiplier = f64::from(atmosphere[10]);
    let blue_density = [atmosphere[4], atmosphere[5], atmosphere[6]].map(f64::from);
    let light_atten = blue_density.map(|density| (density + haze_density * 0.25) * density_multiplier * max_y);
    let combined_density = blue_density.map(|density| density.abs() + haze_density.abs());
    let blue_weight = std::array::from_fn::<_, 3, _>(|index| blue_density[index] / combined_density[index].max(0.000001));
    let haze_weight = combined_density.map(|density| haze_density / density.max(0.000001));
    let sunlight_factor = if atmosphere[15] >= 0.5 { 1.0 } else { 0.7 };
    let sunlight = [12, 13, 14].map(|index| f64::from(atmosphere[index]).max(0.0) * sunlight_factor);
    let positive_up = 0.5 * (up + (up * up + 0.000001).sqrt());
    let off_axis = 1.0 / (positive_up + light[1]).max(0.000001);
    let attenuated_sunlight: [f64; 3] = std::array::from_fn(|index| sunlight[index] * (-light_atten[index] * off_axis).exp());
    let transparency = combined_density.map(|density| (-density * rel_pos_len * density_multiplier).exp());
    let dot = ray.iter().zip(light).map(|(a, b)| a * b).sum::<f64>();
    let glow_angle = ((1.0 - dot).max(0.001) * f64::from(atmosphere[24])).max(0.0);
    let glow_factor = f64::from(atmosphere[31]);
    let glow_term = if glow_factor < 1.0 { 0.0 } else { glow_factor * (glow_angle.powf(f64::from(atmosphere[26])) + 0.25) };
    let blue_horizon = [0, 1, 2].map(|index| f64::from(atmosphere[index]));
    let haze_horizon = f64::from(atmosphere[8]);
    let ambient = [20, 21, 22].map(|index| f64::from(atmosphere[index]));
    let cloud_shadow = f64::from(atmosphere[27]);
    let above_cloud: [f64; 3] = std::array::from_fn(|index| {
        let blue = blue_horizon[index] * blue_weight[index] * (attenuated_sunlight[index] + ambient[index]);
        let haze = haze_horizon * haze_weight[index] * (attenuated_sunlight[index] * glow_term + ambient[index]);
        (blue + haze) * (1.0 - transparency[index])
    });
    let ambient_below: [f64; 3] = std::array::from_fn(|index| ambient[index] + (1.0 - ambient[index]).max(0.0) * cloud_shadow * 0.5);
    let sunlight_below = attenuated_sunlight.map(|value| value * (1.0 - cloud_shadow).max(0.0));
    let below_cloud: [f64; 3] = std::array::from_fn(|index| {
        let blue = blue_horizon[index] * blue_weight[index] * (sunlight_below[index] + ambient_below[index]);
        let haze = haze_horizon * haze_weight[index] * (sunlight_below[index] * glow_term + ambient_below[index]);
        blue + haze
    });
    std::array::from_fn(|index| {
        ((above_cloud[index] + (below_cloud[index] - above_cloud[index]) * (1.0 - transparency[index].sqrt())) * 2.0).clamp(0.0, 5.0)
    })
}

#[cfg(test)]
fn normalize(value: [f64; 3]) -> [f64; 3] {
    let length = value.iter().map(|component| component * component).sum::<f64>().sqrt();
    if length > 0.000001 { value.map(|component| component / length) } else { [1.0, 0.0, 0.0] }
}

#[cfg(test)]
fn smoothstep(edge0: f64, edge1: f64, value: f64) -> f64 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::{Schedule, Sources, evaluate};

    fn default_atmosphere(altitude: f32) -> [f32; ATMOSPHERE_FLOATS] {
        let sources = Sources {
            base: None,
            schedule: Schedule::default(),
            experience_day: None,
            experience_sky: None,
            experience_water: None,
            sky_override: None,
            water_override: None,
        };
        atmosphere_values(Some(&evaluate(&sources, 0.0, 0.0)), altitude)
    }

    #[test]
    fn default_sky_matches_windlight_values_at_zenith_and_horizon() {
        let atmosphere = default_atmosphere(0.0);
        let zenith = horizon_color([0.0, 0.0, 1.0], &atmosphere);
        let horizon = horizon_color([1.0, 0.0, 0.0], &atmosphere);
        let expected_zenith = [0.2004713, 0.2440249, 0.3701902];
        let expected_horizon = [0.4242817, 0.5072095, 0.7518440];
        for (actual, expected) in zenith.into_iter().zip(expected_zenith) {
            assert!((actual - expected).abs() < 0.0001, "zenith {zenith:?}");
        }
        for (actual, expected) in horizon.into_iter().zip(expected_horizon) {
            assert!((actual - expected).abs() < 0.0001, "horizon {horizon:?}");
        }
    }

    #[test]
    fn horizon_color_is_continuous_through_water_and_sky_directions() {
        let atmosphere = default_atmosphere(24.0);
        let horizon = horizon_color([1.0, 0.0, 0.0], &atmosphere);
        let epsilon: f64 = 0.0001;
        let above = horizon_color([(1.0 - epsilon * epsilon).sqrt(), 0.0, epsilon], &atmosphere);
        let below = horizon_color([(1.0 - epsilon * epsilon).sqrt(), 0.0, -epsilon], &atmosphere);
        let above_slope = std::array::from_fn::<_, 3, _>(|index| (above[index] - horizon[index]) / epsilon);
        let below_slope = std::array::from_fn::<_, 3, _>(|index| (horizon[index] - below[index]) / epsilon);
        for index in 0..3 {
            assert!((above[index] - horizon[index]).abs() < 0.0001);
            assert!((below[index] - horizon[index]).abs() < 0.0001);
            assert!((above_slope[index] - below_slope[index]).abs() < 0.01);
        }
    }
}
