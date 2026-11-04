use math::{DVec3, Mat4, Quat, Vec3, normal_matrix};

use super::TRANSFORM_BYTES;
use crate::{material::FaceParams, pipeline::INSTANCE_BYTES};

pub(super) const PARAMETER_BYTES: usize = INSTANCE_BYTES - TRANSFORM_BYTES;

pub(super) fn model(position: DVec3, rotation: Quat, scale: Vec3, origin: DVec3) -> Mat4 {
    Mat4::from_scale_rotation_translation(scale, rotation, (position - origin).as_vec3())
}

pub(super) fn scene_origin(position: DVec3) -> DVec3 {
    let snap = |coordinate: f64| (coordinate / 256.0).floor() * 256.0;
    DVec3::new(snap(position.x), snap(position.y), snap(position.z))
}

pub(super) fn instance_base(position: DVec3, rotation: Quat, scale: Vec3, invert_normals: bool, origin: DVec3) -> [u8; TRANSFORM_BYTES] {
    let mut bytes = [0; TRANSFORM_BYTES];
    let normal = normal_matrix(rotation, scale) * if invert_normals { -1.0 } else { 1.0 };
    let values = model(position, rotation, scale, origin)
        .to_cols_array()
        .into_iter()
        .chain([normal.x_axis, normal.y_axis, normal.z_axis].into_iter().flat_map(|column| column.extend(0.0).to_array()));
    for (dest, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
        *dest = value.to_le_bytes();
    }
    bytes
}

#[cfg(test)]
pub(super) fn packed_instance(base: &[u8; TRANSFORM_BYTES], params: &FaceParams, scale: Vec3) -> [u8; INSTANCE_BYTES] {
    let mut bytes = [0; INSTANCE_BYTES];
    bytes[..TRANSFORM_BYTES].copy_from_slice(base);
    bytes[TRANSFORM_BYTES..].copy_from_slice(&parameter_part(params, scale));
    bytes
}

pub(super) fn parameter_part(params: &FaceParams, scale: Vec3) -> [u8; PARAMETER_BYTES] {
    let mut bytes = [0; PARAMETER_BYTES];
    let values = params.color.into_iter().chain(params.uv.into_iter().flatten()).chain(params.params).chain(scale.extend(0.0).to_array());
    for (dest, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
        *dest = value.to_le_bytes();
    }
    bytes
}

#[cfg(test)]
mod tests {
    use math::Mat3;

    use super::*;

    #[test]
    fn instance_attributes_match_shader() {
        let base = [0xa5; TRANSFORM_BYTES];
        let mut params = FaceParams::new(None);
        params.color = [0.125, 0.25, 0.5, 0.75];
        params.uv = [[2.0, 3.0, 4.0, 0.0], [5.0, 6.0, 7.0, 0.0]];
        params.params = [1.0, 2.0, 0.7, 1.0];
        let bytes = packed_instance(&base, &params, Vec3::new(8.0, 9.0, 10.0));
        assert_eq!(&bytes[..112], &base, "model and normal attributes retain their offsets");
        let attributes: Vec<_> = bytes[112..].as_chunks::<4>().0.iter().map(|word| f32::from_le_bytes(*word)).collect();
        assert_eq!(
            attributes,
            [0.125, 0.25, 0.5, 0.75, 2.0, 3.0, 4.0, 0.0, 5.0, 6.0, 7.0, 0.0, 1.0, 2.0, 0.7, 1.0, 8.0, 9.0, 10.0, 0.0],
            "surface shader locations 10 through 14 consume five vec4 attributes"
        );
        assert_eq!(bytes.len(), 192, "GPU instance stride");
    }
    #[test]
    fn nonuniform_normals_upload() {
        let rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let scale = Vec3::new(2.0, 3.0, 4.0);
        let bytes = instance_base(DVec3::ZERO, rotation, scale, false, DVec3::ZERO);
        let column = |index: usize| {
            let start = 64 + index * 16;
            Vec3::new(
                f32::from_le_bytes(bytes[start..start + 4].try_into().expect("normal x")),
                f32::from_le_bytes(bytes[start + 4..start + 8].try_into().expect("normal y")),
                f32::from_le_bytes(bytes[start + 8..start + 12].try_into().expect("normal z")),
            )
        };
        let normal = Mat3::from_cols(column(0), column(1), column(2));
        assert!(normal.x_axis.distance(Vec3::Y) < 1e-6);
        assert!(normal.y_axis.distance(Vec3::new(-2.0 / 3.0, 0.0, 0.0)) < 1e-6);
        assert!(normal.z_axis.distance(Vec3::Z * 0.5) < 1e-6);
        let local_normal = Vec3::new(1.0, 1.0, 0.0).normalize();
        let local_tangent = Vec3::new(1.0, -1.0, 0.0);
        let world_normal = normal * local_normal;
        let world_tangent = rotation * (scale * local_tangent);
        assert!(world_normal.dot(world_tangent).abs() < 1e-5);
    }
}
