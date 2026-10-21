use std::{
    error::Error,
    f64::consts::PI,
    fmt::{self, Display, Formatter},
};

use assets::TextureSize;
use math::{DVec3, Vec3};

#[cfg(test)]
#[path = "camera_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Detail {
    Lowest,
    Low,
    Medium,
    High,
}

impl Detail {
    pub(crate) const fn scale(self) -> f32 {
        match self {
            Self::Lowest => 1.0,
            Self::Low => 1.5,
            Self::Medium => 2.5,
            Self::High => 4.0,
        }
    }

    fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Lowest,
            1 => Self::Low,
            2 => Self::Medium,
            _ => Self::High,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Lowest => 0,
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
        }
    }
}

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
    pub(crate) fn texture_size(self, center: DVec3, diameter: f64, previous: Option<TextureSize>) -> TextureSize {
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

    pub(crate) fn importance(self, center: DVec3, radius: f64) -> Option<u32> {
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
            return Some(Detail::from_index(THRESHOLDS.into_iter().take_while(|threshold| diameter >= *threshold).count()));
        };
        let mut index = previous.index();
        while index < 3 && diameter >= THRESHOLDS[index] * 1.2 {
            index += 1;
        }
        while index > 0 && diameter < THRESHOLDS[index - 1] * 0.8 {
            index -= 1;
        }
        Some(Detail::from_index(index))
    }
}
