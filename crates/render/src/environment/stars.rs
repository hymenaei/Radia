pub(crate) const STAR_COUNT: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Star {
    pub(crate) direction_intensity: [f32; 4],
    pub(crate) color: [f32; 4],
}

pub(crate) fn stars() -> [Star; STAR_COUNT] {
    let mut seed = 0x5eed_51a7_d15c_a11eu64;
    std::array::from_fn(|_| {
        let x = random(&mut seed) - 0.5;
        let y = random(&mut seed) - 0.5;
        let z = random(&mut seed) * 0.5;
        let length = (x * x + y * y + z * z).sqrt();
        let direction = if length > 0.000_001 { [x / length, y / length, z / length] } else { [0.0, 0.0, 1.0] };
        let intensity = (random(&mut seed).powi(2) + 0.1).min(1.0);
        let red = 0.75 + random(&mut seed) * 0.25;
        let blue = 0.75 + random(&mut seed) * 0.25;
        let size = 16.0 + random(&mut seed) * 20.0;
        Star { direction_intensity: [direction[0], direction[1], direction[2], intensity], color: [red, 1.0, blue, size] }
    })
}

pub(crate) fn star_brightness(brightness: f32) -> f32 {
    let alpha = brightness / 500.0;
    if alpha < 0.001 {
        return 0.0;
    }
    alpha
}

fn random(seed: &mut u64) -> f32 {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    ((*seed >> 40) as u32) as f32 / 16_777_216.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_field_is_deterministic_and_matches_the_legacy_bounds() {
        let first = stars();
        let second = stars();
        assert_eq!(first, second);
        assert_eq!(first.len(), 1000);
        for star in first {
            let [x, y, z, intensity] = star.direction_intensity;
            let length = (x * x + y * y + z * z).sqrt();
            assert!((length - 1.0).abs() < 0.000_01);
            assert!(z >= 0.0);
            assert!((0.1..=1.0).contains(&intensity));
            assert!((0.75..=1.0).contains(&star.color[0]));
            assert_eq!(star.color[1], 1.0);
            assert!((0.75..=1.0).contains(&star.color[2]));
            assert!((16.0..=36.0).contains(&star.color[3]));
        }
    }

    #[test]
    fn star_brightness_uses_legacy_setting_scale_without_sun_fade() {
        assert_eq!(star_brightness(0.49), 0.0);
        assert_eq!(star_brightness(250.0), 0.5);
        assert_eq!(star_brightness(500.0), 1.0);
    }
}
