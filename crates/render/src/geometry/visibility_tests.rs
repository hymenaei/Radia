use math::{Bounds, DVec3, Quat, Vec3};

use super::{VisibilityBands, WorldSphere};
use crate::{geometry::ShadingMode, pipeline::projection, scene::Camera};

const SIZE: [u32; 2] = [1000, 500];

#[test]
fn side_margins_follow_viewport_pixels() {
    let camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, SIZE).expect("fixture camera");
    let bands = VisibilityBands::new(projection(camera, SIZE), SIZE);
    let point = Bounds { min: Vec3::ZERO, max: Vec3::ZERO };
    let at_pixel = |x, y| camera.position() + camera.ray([x, y], SIZE) * 100.0;
    let visible = |x, y, frustum: &math::Frustum| sphere(at_pixel(x, y), point, camera).visible(ShadingMode::Surface, camera, frustum);

    assert!(!visible(1008.0, 250.0, &bands.strict));
    assert!(visible(1008.0, 250.0, &bands.retained));
    assert!(!visible(1049.0, 250.0, &bands.retained));
    assert!(visible(1049.0, 250.0, &bands.preparation));
    assert!(!visible(1065.0, 250.0, &bands.preparation));
    assert!(!visible(500.0, 508.0, &bands.strict));
    assert!(visible(500.0, 508.0, &bands.retained));
}

#[test]
fn side_margins_leave_depth_limits_strict() {
    let camera =
        Camera::new(DVec3::ZERO, Vec3::X, 1.0, SIZE).expect("fixture camera").with_quality(32.0, 1.0).expect("fixture draw distance");
    let bands = VisibilityBands::new(projection(camera, SIZE), SIZE);
    let point = Bounds { min: Vec3::ZERO, max: Vec3::ZERO };
    let test = |position, shading, frustum: &math::Frustum| sphere(position, point, camera).visible(shading, camera, frustum);

    for frustum in [&bands.strict, &bands.retained, &bands.preparation] {
        assert!(!test(camera.position() + camera.direction().as_dvec3() * 0.01, ShadingMode::Surface, frustum));
        assert!(!test(camera.position() + camera.direction().as_dvec3() * 40.0, ShadingMode::Surface, frustum));
    }
    assert!(test(camera.position() + camera.direction().as_dvec3() * 40.0, ShadingMode::Terrain, &bands.preparation));
}

#[test]
fn transformed_negative_scale_bounds_stay_visible() {
    let camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, SIZE).expect("fixture camera");
    let bounds = Bounds { min: Vec3::new(1.8, -0.2, -0.2), max: Vec3::new(2.2, 0.2, 0.2) };
    let bands = VisibilityBands::new(projection(camera, SIZE), SIZE);

    assert!(
        WorldSphere::new(
            DVec3::new(10.0, -7.5, 0.0),
            Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2),
            Vec3::new(-1.0, 1.0, 1.0),
            bounds,
            camera,
        )
        .expect("finite transformed bounds")
        .visible(ShadingMode::Surface, camera, &bands.strict)
    );
}

fn sphere(position: DVec3, bounds: Bounds, camera: Camera) -> WorldSphere {
    WorldSphere::new(position, Quat::IDENTITY, Vec3::ONE, bounds, camera).expect("finite bounds")
}
