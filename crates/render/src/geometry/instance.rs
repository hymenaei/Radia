use math::{Bounds, DVec3, Frustum, Mat4, Quat, Vec3, normal_matrix};

use super::TRANSFORM_BYTES;
use crate::{material::FaceParams, pipeline::INSTANCE_BYTES, scene::Camera};

pub(super) fn model(position: DVec3, rotation: Quat, scale: Vec3, origin: DVec3) -> Mat4 {
    Mat4::from_scale_rotation_translation(scale, rotation, (position - origin).as_vec3())
}

pub(super) fn scene_origin(position: DVec3) -> DVec3 {
    let snap = |coordinate: f64| (coordinate / 256.0).floor() * 256.0;
    DVec3::new(snap(position.x), snap(position.y), snap(position.z))
}

pub(super) fn instance_base(position: DVec3, rotation: Quat, scale: Vec3, origin: DVec3) -> [u8; TRANSFORM_BYTES] {
    let mut bytes = [0; TRANSFORM_BYTES];
    let normal = normal_matrix(rotation, scale);
    let values = model(position, rotation, scale, origin)
        .to_cols_array()
        .into_iter()
        .chain([normal.x_axis, normal.y_axis, normal.z_axis].into_iter().flat_map(|column| column.extend(0.0).to_array()));
    for (dest, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
        *dest = value.to_le_bytes();
    }
    bytes
}

pub(super) fn packed_instance(base: &[u8; TRANSFORM_BYTES], params: &FaceParams, scale: Vec3) -> [u8; INSTANCE_BYTES] {
    let mut bytes = [0; INSTANCE_BYTES];
    bytes[..TRANSFORM_BYTES].copy_from_slice(base);
    let values = params.color.into_iter().chain(params.uv.into_iter().flatten()).chain(params.params).chain(scale.extend(0.0).to_array());
    for (dest, value) in bytes[TRANSFORM_BYTES..].as_chunks_mut::<4>().0.iter_mut().zip(values) {
        *dest = value.to_le_bytes();
    }
    bytes
}

pub(super) fn is_visible(position: DVec3, rotation: Quat, scale: Vec3, bounds: Bounds, camera: Camera, frustum: &Frustum) -> bool {
    let center = (bounds.min.as_dvec3() + bounds.max.as_dvec3()) * 0.5 * scale.as_dvec3();
    let offset = position - camera.position() + (rotation * center.as_vec3()).as_dvec3();
    let radius = ((bounds.max.as_dvec3() - bounds.min.as_dvec3()) * 0.5 * scale.as_dvec3()).length();
    if !offset.is_finite()
        || !radius.is_finite()
        || offset.length() - radius > camera.draw_distance()
        || offset.abs().max_element() + radius > f64::from(f32::MAX) / 16.0
    {
        return false;
    }
    let relative = position - camera.position();
    if !relative.is_finite() || relative.abs().max_element() > f64::from(f32::MAX) || !rotation.is_finite() || !scale.recip().is_finite() {
        return false;
    }
    frustum.intersects_sphere(offset, radius)
}

#[cfg(test)]
mod tests {
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
}
