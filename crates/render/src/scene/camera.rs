use std::{
    error::Error,
    f64::consts::PI,
    fmt::{self, Display, Formatter},
};

use assets::TextureSize;
use math::{DVec3, Vec3};

use super::mesh::Detail;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CameraError {
    Position,
    Direction,
    Projection,
    Quality,
}

impl Display for CameraError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Position => "camera position must be finite",
            Self::Direction => "camera direction must be finite and nonzero",
            Self::Projection => "camera field of view must be between zero and pi radians, with nonzero viewport dimensions",
            Self::Quality => "draw distance must be between 32 and 1024 meters and LOD factor between 0.25 and 4",
        })
    }
}

impl Error for CameraError {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    position: DVec3,
    direction: DVec3,
    focal_pixels: f64,
    vertical_fov: f64,
    view_planes: [DVec3; 4],
    draw_distance: f64,
    lod_factor: f64,
}

impl Camera {
    pub(in crate::scene) fn texture_size(self, center: DVec3, diameter: f64, previous: Option<TextureSize>) -> TextureSize {
        let depth = (center - self.position).dot(self.direction).max(diameter * 0.5).max(0.01);
        let pixels = (diameter * self.focal_pixels / depth).clamp(1.0, 2048.0);
        if let Some(previous) = previous {
            let edge = f64::from(previous.edge());
            if pixels <= edge * 1.2 && pixels >= edge * 0.4 {
                return previous;
            }
        }
        TextureSize::new(pixels.ceil() as u32)
    }
    pub fn new(position: DVec3, direction: Vec3, vertical_fov: f64, viewport: [u32; 2]) -> Result<Self, CameraError> {
        if !position.is_finite() {
            return Err(CameraError::Position);
        }
        let direction = direction.as_dvec3().try_normalize().ok_or(CameraError::Direction)?;
        if !vertical_fov.is_finite() || !(0.0..PI).contains(&vertical_fov) || viewport.contains(&0) {
            return Err(CameraError::Projection);
        }
        let focal_pixels = f64::from(viewport[1]) / (2.0 * (vertical_fov * 0.5).tan());
        if !focal_pixels.is_finite() || focal_pixels <= 0.0 {
            return Err(CameraError::Projection);
        }
        let world_up = if direction.z.abs() > 0.999 { DVec3::Y } else { DVec3::Z };
        let right = direction.cross(world_up).normalize();
        let up = right.cross(direction);
        let vertical = (vertical_fov * 0.5).tan();
        let horizontal = vertical * f64::from(viewport[0]) / f64::from(viewport[1]);
        let view_planes =
            [direction * horizontal + right, direction * horizontal - right, direction * vertical + up, direction * vertical - up]
                .map(DVec3::normalize);
        Ok(Self { position, direction, focal_pixels, vertical_fov, view_planes, draw_distance: 512.0, lod_factor: 1.0 })
    }

    pub fn with_quality(mut self, draw_distance: f64, lod_factor: f64) -> Result<Self, CameraError> {
        if !(32.0..=1024.0).contains(&draw_distance) || !(0.25..=4.0).contains(&lod_factor) {
            return Err(CameraError::Quality);
        }
        self.draw_distance = draw_distance;
        self.lod_factor = lod_factor;
        Ok(self)
    }

    pub fn draw_distance(self) -> f64 {
        self.draw_distance
    }

    pub fn position(self) -> DVec3 {
        self.position
    }
    pub fn direction(self) -> Vec3 {
        self.direction.as_vec3()
    }
    pub fn vertical_fov(self) -> f64 {
        self.vertical_fov
    }

    pub(in crate::scene) fn importance(self, center: DVec3, radius: f64) -> Option<u32> {
        self.detail(center, radius, None)?;
        let offset = center - self.position;
        let depth = offset.dot(self.direction);
        if self.view_planes.into_iter().any(|plane| offset.dot(plane) < -radius) {
            return None;
        }
        Some((2.0 * radius * self.focal_pixels / depth.max(radius)).ceil().clamp(1.0, f64::from(u32::MAX)) as u32)
    }

    pub fn detail(self, center: DVec3, radius: f64, previous: Option<Detail>) -> Option<Detail> {
        if !center.is_finite() || !radius.is_finite() || radius <= 0.0 {
            return None;
        }
        let offset = center - self.position;
        if !offset.is_finite() || offset.length() - radius > self.draw_distance {
            return None;
        }
        let depth = offset.dot(self.direction);
        if !depth.is_finite() {
            return None;
        }
        if depth + radius <= 0.0 {
            return None;
        }
        if depth <= radius {
            return Some(Detail::High);
        }
        let diameter = (radius / (depth - radius)) * self.focal_pixels * 2.0 * self.lod_factor;
        const THRESHOLDS: [f64; 3] = [24.0, 96.0, 384.0];
        let Some(previous) = previous else {
            return Some(detail_from_index(THRESHOLDS.into_iter().take_while(|threshold| diameter >= *threshold).count()));
        };
        let mut index = detail_index(previous);
        while index < 3 && diameter >= THRESHOLDS[index] * 1.2 {
            index += 1;
        }
        while index > 0 && diameter < THRESHOLDS[index - 1] * 0.8 {
            index -= 1;
        }
        Some(detail_from_index(index))
    }
}

fn detail_from_index(index: usize) -> Detail {
    match index {
        0 => Detail::Lowest,
        1 => Detail::Low,
        2 => Detail::Medium,
        _ => Detail::High,
    }
}

fn detail_index(detail: Detail) -> usize {
    match detail {
        Detail::Lowest => 0,
        Detail::Low => 1,
        Detail::Medium => 2,
        Detail::High => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_hysteresis() {
        let camera = Camera::new(DVec3::ZERO, Vec3::X, PI / 2.0, [1000, 1000]).unwrap();
        let center = DVec3::new(100.0, 0.0, 0.0);
        assert_eq!(camera.detail(center, 1.0, None), Some(Detail::Lowest));
        assert_eq!(camera.detail(center, 5.0, None), Some(Detail::Low));
        assert_eq!(camera.detail(center, 20.0, None), Some(Detail::Medium));
        assert_eq!(camera.detail(center, 40.0, None), Some(Detail::High));
        assert_eq!(camera.detail(center, 2.6, Some(Detail::Lowest)), Some(Detail::Lowest));
        assert_eq!(camera.detail(center, 2.6, Some(Detail::Low)), Some(Detail::Low));
        assert_eq!(camera.detail(center, 1.8, Some(Detail::Low)), Some(Detail::Lowest));
        assert_eq!(camera.detail(center, 3.0, Some(Detail::Lowest)), Some(Detail::Low));
        assert_eq!(camera.detail(DVec3::X, 2.0, None), Some(Detail::High));
        assert_eq!(camera.detail(-DVec3::X * 10.0, 2.0, None), None);
        let translated = Camera::new(DVec3::splat(1e12), Vec3::X, PI / 2.0, [1000, 1000]).unwrap();
        assert_eq!(translated.detail(DVec3::splat(1e12) + center, 20.0, None), Some(Detail::Medium));
    }

    #[test]
    fn camera_input_limits() {
        assert_eq!(Camera::new(DVec3::NAN, Vec3::X, 1.0, [1000, 1000]), Err(CameraError::Position));
        for direction in [Vec3::ZERO, Vec3::NAN, Vec3::INFINITY] {
            assert_eq!(Camera::new(DVec3::ZERO, direction, 1.0, [1000, 1000]), Err(CameraError::Direction));
        }
        for fov in [0.0, -1.0, PI, f64::NAN, f64::INFINITY] {
            assert_eq!(Camera::new(DVec3::ZERO, Vec3::X, fov, [1000, 1000]), Err(CameraError::Projection));
        }
        assert_eq!(Camera::new(DVec3::ZERO, Vec3::X, 1.0, [0, 0]), Err(CameraError::Projection));
        assert_eq!(Camera::new(DVec3::ZERO, Vec3::X, 1.0, [0, 1000]), Err(CameraError::Projection));
        assert_eq!(Camera::new(DVec3::ZERO, Vec3::X, 1.0, [1000, 0]), Err(CameraError::Projection));
        let camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [1000, 1000]).unwrap();
        for distance in [32.0, 512.0, 1024.0] {
            assert_eq!(camera.with_quality(distance, 1.0).unwrap().draw_distance(), distance);
        }
        assert_eq!(camera.detail(DVec3::X * 900.0, 1.0, None), None);
        assert_eq!(camera.with_quality(1024.0, 1.0).unwrap().detail(DVec3::X * 900.0, 1.0, None), Some(Detail::Lowest));
        for distance in [0.0, 31.0, 1024.01, f64::NAN, f64::INFINITY] {
            assert_eq!(camera.with_quality(distance, 1.0), Err(CameraError::Quality));
        }
        for factor in [0.0, 0.24, 4.01, f64::NAN, f64::INFINITY] {
            assert_eq!(camera.with_quality(512.0, factor), Err(CameraError::Quality));
        }
        for radius in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(camera.detail(DVec3::X, radius, None), None);
        }
        assert_eq!(camera.detail(DVec3::NAN, 1.0, None), None);
    }

    #[test]
    fn texture_detail_tracks_projection() {
        let camera = Camera::new(DVec3::ZERO, Vec3::X, PI / 2.0, [1000, 1000]).unwrap();
        assert_eq!(camera.texture_size(DVec3::X * 10.0, 2.0, None).edge(), 128);
        assert_eq!(camera.texture_size(DVec3::X * 100.0, 2.0, None).edge(), 32);
        assert_eq!(camera.texture_size(DVec3::X, 2.0, None).edge(), 1024);
        let previous = Some(TextureSize::new(128));
        assert_eq!(camera.texture_size(DVec3::X * 7.0, 2.0, previous).edge(), 128);
        assert_eq!(camera.texture_size(DVec3::X * 6.0, 2.0, previous).edge(), 256);
        assert_eq!(camera.texture_size(DVec3::X * 25.0, 2.0, previous).edge(), 64);
        assert_eq!(camera.texture_size(DVec3::X * 100.0, 2.0, Some(TextureSize::new(64))).edge(), 32);
        let tall = Camera::new(DVec3::ZERO, Vec3::X, PI / 2.0, [2000, 2000]).unwrap();
        assert_eq!(tall.texture_size(DVec3::X * 10.0, 2.0, None).edge(), 256);
    }

    #[test]
    fn importance_tracks_view_bounds() {
        let square = Camera::new(DVec3::ZERO, Vec3::X, PI / 2.0, [1000, 1000]).unwrap();
        let wide = Camera::new(DVec3::ZERO, Vec3::X, PI / 2.0, [2000, 1000]).unwrap();
        assert!(square.importance(DVec3::new(10.0, 9.0, 0.0), 1.0).is_some());
        assert!(square.importance(DVec3::new(10.0, 11.0, 0.0), 1.0).is_some(), "edge intersections remain visible");
        assert_eq!(square.importance(DVec3::new(10.0, 12.0, 0.0), 1.0), None);
        assert!(wide.importance(DVec3::new(10.0, 18.0, 0.0), 1.0).is_some());
        assert_eq!(wide.importance(DVec3::new(10.0, 0.0, 12.0), 1.0), None);
        assert_eq!(wide.importance(-DVec3::X * 10.0, 1.0), None);
        assert_eq!(wide.importance(DVec3::X * 600.0, 1.0), None);
        assert!(square.importance(DVec3::X * 10.0, 1.0) > square.importance(DVec3::X * 100.0, 1.0));
        let vertical = Camera::new(DVec3::ZERO, Vec3::Z, PI / 2.0, [1000, 1000]).unwrap();
        assert!(vertical.importance(DVec3::Z * 10.0, 1.0).is_some());
        assert_eq!(vertical.importance(DVec3::new(12.0, 0.0, 10.0), 1.0), None);
    }
}
