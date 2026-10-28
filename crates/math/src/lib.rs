mod frustum;
mod intersection;

pub use self::frustum::Frustum;
pub use self::intersection::{ray_intersects_bounds, ray_triangle_intersection};
pub use glam::{
    Affine2, Affine3, DMat2, DMat3, DMat4, DQuat, DVec2, DVec3, DVec4, EulerRot, FloatExt, Mat2, Mat3, Mat4, Quat, Vec2, Vec3, Vec4,
    camera, dcamera,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UvTransform {
    pub offset: [f32; 2],
    pub scale: [f32; 2],
    pub rotation: f32,
}

impl UvTransform {
    pub fn is_finite(self) -> bool {
        self.offset.into_iter().chain(self.scale).all(f32::is_finite) && self.rotation.is_finite()
    }
}

impl Default for UvTransform {
    fn default() -> Self {
        Self { offset: [0.0; 2], scale: [1.0; 2], rotation: 0.0 }
    }
}

pub fn normal_matrix(rotation: Quat, scale: Vec3) -> Mat3 {
    let inverse = scale.recip();
    Mat3::from_quat(rotation) * Mat3::from_diagonal(inverse / inverse.abs().max_element())
}

pub fn flips_winding(scale: Vec3) -> bool {
    !scale.is_negative_bitmask().count_ones().is_multiple_of(2)
}

pub fn srgb8_to_linear(value: u8) -> f32 {
    srgb_to_linear(f32::from(value) / 255.0)
}

pub fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
}

pub fn linear_to_srgb8(value: f32) -> u8 {
    let value = if value <= 0.0031308 { value * 12.92 } else { 1.055 * value.powf(1.0 / 2.4) - 0.055 };
    (value * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_transfer_matches_reference() {
        for (encoded, linear) in [(0, 0.0), (10, 0.00303527), (11, 0.003346536), (128, 0.2158605), (255, 1.0)] {
            assert!((srgb8_to_linear(encoded) - linear).abs() < 1e-7, "sRGB channel {encoded}");
            assert_eq!(linear_to_srgb8(linear), encoded, "linear channel {linear}");
        }
        assert!((srgb_to_linear(0.25) - 0.05087609).abs() < 1e-7);
    }

    #[test]
    fn normal_transform() {
        let rotation = Quat::from_rotation_z(0.3);
        let scale = Vec3::new(-2.0, 3.0, 4.0);
        let normal = normal_matrix(rotation, scale) * Vec3::new(1.0, 1.0, 0.0);
        let tangent = rotation * (scale * Vec3::new(1.0, -1.0, 0.0));
        assert!(normal.dot(tangent).abs() < 1e-5);
        assert!((normal_matrix(Quat::IDENTITY, Vec3::splat(1e30)) * Vec3::X).is_finite());
        assert_eq!(normal_matrix(Quat::IDENTITY, scale) * Vec3::X, -Vec3::X);
    }

    #[test]
    fn reflected_winding() {
        for mask in 0..8 {
            let scale = Vec3::from_array([0, 1, 2].map(|axis| if mask & (1 << axis) == 0 { 2.0 } else { -2.0 }));
            let cross = (scale * Vec3::X).cross(scale * Vec3::Y);
            let outward = normal_matrix(Quat::IDENTITY, scale) * Vec3::Z;
            assert_eq!(flips_winding(scale), cross.dot(outward) < 0.0, "reflection mask {mask}");
            assert_eq!(flips_winding(scale * 1e30), flips_winding(scale), "large finite scales must preserve winding");
        }
    }
}
