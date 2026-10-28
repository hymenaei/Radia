use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};

use math::{DVec3, ray_intersects_bounds, ray_triangle_intersection};

use super::{GeometryRenderer, Instance};
use crate::scene::Camera;

const MAX_PICK_TRIANGLES: usize = 250_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PickError {
    TriangleLimitExceeded,
}

impl Display for PickError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("pick exceeded its triangle work limit")
    }
}

impl Error for PickError {}

impl GeometryRenderer {
    pub(crate) fn pick(&self, camera: Camera, ray: DVec3) -> Result<Option<DVec3>, PickError> {
        pick_instances(&self.visible[..self.visible_count], camera, ray)
    }
}

fn pick_instances(instances: &[Instance], camera: Camera, ray: DVec3) -> Result<Option<DVec3>, PickError> {
    if !ray.is_finite() {
        return Ok(None);
    }

    let mut closest = camera.draw_distance() as f32;
    let mut found = false;
    let mut work = 0;
    for instance in instances {
        if !instance.pickable {
            continue;
        }

        let inverse = instance.rotation.conjugate();
        let origin = inverse * (camera.position() - instance.position).as_vec3() / instance.scale;
        let direction = inverse * ray.as_vec3() / instance.scale;
        if !origin.is_finite() || !direction.is_finite() {
            continue;
        }
        let mesh = instance.mesh.mesh();
        if !ray_intersects_bounds(origin, direction, mesh.bounds(), closest) {
            continue;
        }

        for face in &instance.faces {
            let indices = &mesh.indices()[face.indices.start as usize..face.indices.end as usize];
            for triangle_indices in indices.as_chunks::<3>().0 {
                if work == MAX_PICK_TRIANGLES {
                    return Err(PickError::TriangleLimitExceeded);
                }
                work += 1;
                let triangle = triangle_indices.map(|index| mesh.vertices()[index as usize].position);
                let Some(distance) = ray_triangle_intersection(origin, direction, triangle, face.double_sided) else {
                    continue;
                };
                if distance >= 0.05 && distance < closest {
                    closest = distance;
                    found = true;
                }
            }
        }
    }

    Ok(found.then(|| camera.position() + ray * f64::from(closest)))
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use math::{DVec3, Quat, Vec3};

    use super::{MAX_PICK_TRIANGLES, PickError, pick_instances};
    use crate::{
        geometry::{Instance, PickFace, fixtures},
        scene::{Camera, SharedMesh},
    };

    #[test]
    fn backface_needs_double_sided() {
        let mesh = shared_mesh();
        let (triangle, indices) = first_triangle(&mesh);
        let (front_camera, center, front_ray) = camera_for_triangle(triangle);
        let single_sided = instance(mesh.clone(), DVec3::ZERO, indices.clone(), false);
        assert_hit_near(&[single_sided], front_camera, front_ray, center);

        let back_camera = Camera::new(center + front_ray * 5.0, -front_ray.as_vec3(), 1.0, [128, 128]).unwrap();
        let back_ray = back_camera.direction().as_dvec3();
        assert_eq!(pick_instances(&[instance(mesh.clone(), DVec3::ZERO, indices.clone(), false)], back_camera, back_ray), Ok(None));
        assert_hit_near(&[instance(mesh, DVec3::ZERO, indices, true)], back_camera, back_ray, center);
    }

    #[test]
    fn closest_hit_wins() {
        let mesh = shared_mesh();
        let (triangle, indices) = first_triangle(&mesh);
        let (camera, center, ray) = camera_for_triangle(triangle);
        let position_at = |distance| camera.position() + ray * distance - center;
        let instances =
            [instance(mesh.clone(), position_at(2.0), indices.clone(), false), instance(mesh, position_at(1.0), indices, false)];

        let expected = camera.position() + ray;
        assert_hit_near(&instances, camera, ray, expected);
    }

    #[test]
    fn min_distance_skips_near_hit() {
        let mesh = shared_mesh();
        let (triangle, indices) = first_triangle(&mesh);
        let (camera, center, ray) = camera_for_triangle(triangle);
        let position_at = |distance| camera.position() + ray * distance - center;
        let instances =
            [instance(mesh.clone(), position_at(0.01), indices.clone(), false), instance(mesh, position_at(0.1), indices, false)];

        assert_hit_near(&instances, camera, ray, camera.position() + ray * 0.1);
    }

    #[test]
    fn work_limit_follows_bounds() {
        let mesh = shared_mesh();
        let (triangle, indices) = first_triangle(&mesh);
        let (camera, _, ray) = camera_for_triangle(triangle);
        let mut instance = instance(mesh, DVec3::new(0.0, 100.0, 0.0), indices.clone(), false);
        instance.faces = (0..=MAX_PICK_TRIANGLES).map(|_| PickFace { indices: indices.clone(), double_sided: false }).collect();

        assert_eq!(pick_instances(std::slice::from_ref(&instance), camera, ray), Ok(None));
        instance.position = DVec3::ZERO;
        assert_eq!(pick_instances(std::slice::from_ref(&instance), camera, ray), Err(PickError::TriangleLimitExceeded));
    }

    fn shared_mesh() -> SharedMesh {
        let (_scene, view, _) = fixtures::scene(255);
        let (_, mesh) = view.objects().next().expect("fixture object").mesh().expect("fixture mesh");
        mesh.clone()
    }

    fn first_triangle(mesh: &SharedMesh) -> ([Vec3; 3], Range<u32>) {
        let face = mesh.mesh().faces().first().expect("fixture face");
        let indices = face.indices.start..face.indices.start + 3;
        let triangle_indices = mesh.mesh().indices()[indices.start as usize..indices.end as usize].as_chunks::<3>().0[0];
        let triangle = triangle_indices.map(|index| mesh.mesh().vertices()[index as usize].position);
        (triangle, indices)
    }

    fn instance(mesh: SharedMesh, position: DVec3, indices: Range<u32>, double_sided: bool) -> Instance {
        Instance {
            mesh,
            position,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
            pickable: true,
            faces: vec![PickFace { indices, double_sided }],
        }
    }

    fn camera_for_triangle(triangle: [Vec3; 3]) -> (Camera, DVec3, DVec3) {
        let [a, b, c] = triangle.map(Vec3::as_dvec3);
        let center = (a + b + c) / 3.0;
        let normal = (b - a).cross(c - a).normalize();
        let direction = -normal;
        let camera = Camera::new(center - direction * 5.0, direction.as_vec3(), 1.0, [128, 128]).unwrap();
        (camera, center, camera.direction().as_dvec3())
    }

    fn assert_hit_near(instances: &[Instance], camera: Camera, ray: DVec3, expected: DVec3) {
        let picked = pick_instances(instances, camera, ray).expect("fixture stays within the pick work limit");
        let point = picked.expect("expected a ray hit");
        assert!(point.distance(expected) < 1e-4, "picked {point:?}, expected {expected:?}");
    }
}
