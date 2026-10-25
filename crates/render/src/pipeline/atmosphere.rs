use crate::environment::Evaluated;

pub(crate) const ATMOSPHERE_FLOATS: usize = 32;

pub(crate) fn atmosphere_values(environment: Option<&Evaluated>) -> [f32; ATMOSPHERE_FLOATS] {
    let mut values = [0.0; ATMOSPHERE_FLOATS];
    let Some(environment) = environment else { return values };
    let haze = &environment.sky.haze;
    let values_source = [
        [haze.blue_horizon[0], haze.blue_horizon[1], haze.blue_horizon[2], 0.0],
        [haze.blue_density[0], haze.blue_density[1], haze.blue_density[2], 0.0],
        [haze.horizon, haze.density, haze.density_multiplier, environment.sky.max_y],
        [environment.sky.sunlight[0], environment.sky.sunlight[1], environment.sky.sunlight[2], 0.0],
        [environment.sky.sun_direction[0], environment.sky.sun_direction[1], environment.sky.sun_direction[2], 0.0],
        [haze.ambient[0], haze.ambient[1], haze.ambient[2], environment.sky.gamma],
        [environment.sky.glow[0], environment.sky.glow[1], environment.sky.glow[2], environment.sky.clouds.shadow],
        [haze.distance_multiplier, 0.0, 0.0, 0.0],
    ];
    for (target, value) in values.iter_mut().zip(values_source.into_iter().flatten()) {
        *target = value;
    }
    values
}

#[cfg(test)]
mod tests {
    fn smoothstep(value: f64, edge0: f64, edge1: f64) -> f64 {
        let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    fn display(radiance: f64, gamma: f64) -> f64 {
        radiance.max(0.0).powf(1.0 / gamma.max(0.05))
    }

    fn sky(vertical: f64, horizon: f64, upper: f64, lower: f64, gamma: f64) -> f64 {
        let radiance = if vertical >= 0.0 {
            horizon + (upper - horizon) * smoothstep(vertical, 0.0, 0.72)
        } else {
            horizon + (lower - horizon) * smoothstep(-vertical, 0.0, 0.72)
        };
        display(radiance, gamma)
    }

    fn water(distance: f64, fog: f64, horizon: f64, start: f64, draw_distance: f64, gamma: f64) -> f64 {
        display(fog + (horizon - fog) * smoothstep(distance, start, draw_distance), gamma)
    }

    #[test]
    fn sky_and_water_share_a_value_and_slope_continuous_horizon() {
        let horizon = 0.34;
        let upper = 0.91;
        let lower = 0.07;
        let gamma = 2.2;
        let horizon_color = sky(0.0, horizon, upper, lower, gamma);
        assert_eq!(horizon_color, water(100.0, 0.12, horizon, 84.0, 100.0, gamma));
        for epsilon in [0.0001, 0.00005] {
            let above_slope = (sky(epsilon, horizon, upper, lower, gamma) - horizon_color) / epsilon;
            let below_slope = (horizon_color - sky(-epsilon, horizon, upper, lower, gamma)) / epsilon;
            assert!(above_slope.abs() < 0.001);
            assert!(below_slope.abs() < 0.001);
        }
    }
}
