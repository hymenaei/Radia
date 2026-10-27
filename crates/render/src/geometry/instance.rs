use math::{DVec3, Frustum, Mat4, Vec3, normal_matrix};

use super::{Instance, ShadingMode, TRANSFORM_BYTES};
use crate::{material::FaceParams, pipeline::INSTANCE_BYTES, scene::Camera};

pub(super) fn model(instance: &Instance, origin: DVec3) -> Mat4 {
    Mat4::from_scale_rotation_translation(instance.scale, instance.rotation, (instance.position - origin).as_vec3())
}

pub(super) fn instance_base(instance: &Instance, origin: DVec3) -> [u8; TRANSFORM_BYTES] {
    let mut bytes = [0; TRANSFORM_BYTES];
    let normal = normal_matrix(instance.rotation, instance.scale);
    let values = model(instance, origin)
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

pub(super) fn is_visible(instance: &Instance, camera: Camera, frustum: &Frustum) -> bool {
    let bounds = instance.mesh.mesh().bounds();
    let center = (bounds.min.as_dvec3() + bounds.max.as_dvec3()) * 0.5 * instance.scale.as_dvec3();
    let offset = instance.position - camera.position() + (instance.rotation * center.as_vec3()).as_dvec3();
    let radius = ((bounds.max.as_dvec3() - bounds.min.as_dvec3()) * 0.5 * instance.scale.as_dvec3()).length();
    let beyond_draw_distance = !matches!(&instance.shading, ShadingMode::RegionWater) && offset.length() - radius > camera.draw_distance();
    if !offset.is_finite()
        || !radius.is_finite()
        || beyond_draw_distance
        || offset.abs().max_element() + radius > f64::from(f32::MAX) / 16.0
    {
        return false;
    }
    let model = model(instance, camera.position());
    if !model.is_finite() || !instance.scale.recip().is_finite() {
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

    #[test]
    fn nonuniform_normals_upload() {
        let (_scene, view, _) = fixtures::scene(255);
        let object = view.objects().next().expect("fixture object");
        let (_, mesh) = object.mesh().expect("fixture mesh");
        let rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let scale = Vec3::new(2.0, 3.0, 4.0);
        let instance = Instance {
            mesh: mesh.clone(),
            position: DVec3::ZERO,
            rotation,
            scale,
            pickable: true,
            faces: Vec::new(),
            shading: ShadingMode::Surface,
            region_water_id: None,
        };
        let bytes = instance_base(&instance, DVec3::ZERO);
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

    #[test]
    fn region_water_visibility_ignores_user_draw_distance() {
        use crate::pipeline::projection;

        let (_scene, view, _) = fixtures::scene(255);
        let object = view.objects().next().expect("fixture object");
        let (_, mesh) = object.mesh().expect("fixture mesh");
        let camera = Camera::new(DVec3::new(240.0, 128.0, 20.0), -Vec3::X, 1.0, [1280, 720]).unwrap().with_quality(32.0, 1.0).unwrap();
        let frustum = Frustum::from_clip_matrix(projection(camera, [1280, 720]));
        let water = Instance {
            mesh: mesh.clone(),
            position: DVec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
            pickable: false,
            faces: Vec::new(),
            shading: ShadingMode::RegionWater,
            region_water_id: Some(1),
        };
        let surface = Instance {
            mesh: mesh.clone(),
            position: DVec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
            pickable: true,
            faces: Vec::new(),
            shading: ShadingMode::Surface,
            region_water_id: None,
        };
        assert!(is_visible(&water, camera, &frustum));
        assert!(!is_visible(&surface, camera, &frustum));
    }
}
