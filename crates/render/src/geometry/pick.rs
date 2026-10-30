use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    ops::Range,
};

use math::{DVec3, Quat, Vec3, ray_intersects_bounds, ray_triangle_intersection};
use uuid::Uuid;
use world::Kind;

use super::{GeometryRenderer, ShadingMode, items::CachedItem};
use crate::scene::{Camera, SharedMesh, SurfaceKind};

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickHit {
    pub target: PickTarget,
    pub position: DVec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PickTarget {
    Object(PickObject),
    Terrain,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickObject {
    pub id: Uuid,
    pub kind: PickObjectKind,
    pub position: DVec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PickObjectKind {
    Prim,
    Avatar,
    Unknown(u8),
}

impl GeometryRenderer {
    pub(crate) fn pick(&self, camera: Camera, ray: DVec3) -> Result<Option<PickHit>, PickError> {
        let mut query = PickQuery::new(camera, ray);
        if !ray.is_finite() {
            return Ok(None);
        }
        for id in &self.cache.visible {
            let Some(item) = self.cache.items.get(*id) else { continue };
            if !item.pickable {
                continue;
            }
            let Some(target) = pick_target(item) else { continue };
            let faces = item.faces.iter().filter(|face| face.slot.is_some()).map(|face| (&face.indices, face.params.double_sided));
            query.visit_item(&item.mesh, item.position, item.rotation, item.scale, target, item.shading, faces)?;
        }
        Ok(query.result())
    }
}

fn pick_target(item: &CachedItem) -> Option<PickTarget> {
    if let Some(object) = &item.source {
        let summary = object.detail().summary;
        let kind = match summary.kind {
            Kind::Object => PickObjectKind::Prim,
            Kind::Avatar => PickObjectKind::Avatar,
            Kind::Unknown(kind) => PickObjectKind::Unknown(kind),
        };
        return Some(PickTarget::Object(PickObject {
            id: summary.id,
            kind,
            position: item.position,
            rotation: item.rotation,
            scale: item.scale,
        }));
    }

    match item.surface_kind? {
        SurfaceKind::Terrain(_) => Some(PickTarget::Terrain),
        SurfaceKind::Water => None,
    }
}

struct PickQuery {
    camera: Camera,
    ray: DVec3,
    closest: f32,
    hit: Option<PickHit>,
    work: usize,
}

impl PickQuery {
    fn new(camera: Camera, ray: DVec3) -> Self {
        Self { camera, ray, closest: f32::INFINITY, hit: None, work: 0 }
    }

    fn visit_item<'a>(
        &mut self,
        shared: &SharedMesh,
        position: DVec3,
        rotation: Quat,
        scale: Vec3,
        target: PickTarget,
        shading: ShadingMode,
        faces: impl Iterator<Item = (&'a Range<u32>, bool)>,
    ) -> Result<(), PickError> {
        let mesh = shared.mesh();
        let max_distance = if shading == ShadingMode::Studio { self.camera.draw_distance() as f32 } else { f32::INFINITY };
        let limit = self.closest.min(max_distance);
        let inverse = rotation.conjugate();
        let origin = inverse * (self.camera.position() - position).as_vec3() / scale;
        let direction = inverse * self.ray.as_vec3() / scale;
        if !origin.is_finite() || !direction.is_finite() || !ray_intersects_bounds(origin, direction, mesh.bounds(), limit) {
            return Ok(());
        }

        for (range, double_sided) in faces {
            let Ok(start) = usize::try_from(range.start) else { continue };
            let Ok(end) = usize::try_from(range.end) else { continue };
            let Some(indices) = mesh.indices().get(start..end) else { continue };
            for triangle_indices in indices.as_chunks::<3>().0 {
                if self.work == MAX_PICK_TRIANGLES {
                    return Err(PickError::TriangleLimitExceeded);
                }
                self.work += 1;
                let Some(first) = triangle_indices.first().and_then(|index| mesh.vertices().get(*index as usize)) else { continue };
                let Some(second) = triangle_indices.get(1).and_then(|index| mesh.vertices().get(*index as usize)) else { continue };
                let Some(third) = triangle_indices.get(2).and_then(|index| mesh.vertices().get(*index as usize)) else { continue };
                let triangle = [first.position, second.position, third.position];
                let Some(distance) = ray_triangle_intersection(origin, direction, triangle, double_sided) else { continue };
                if distance < 0.05 || distance >= self.closest || distance >= max_distance {
                    continue;
                }
                self.closest = distance;
                self.hit = Some(PickHit { target, position: self.camera.position() + self.ray * f64::from(distance) });
            }
        }
        Ok(())
    }

    fn result(self) -> Option<PickHit> {
        self.hit
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use math::{DVec3, Quat, Vec3};
    use uuid::Uuid;

    use super::{MAX_PICK_TRIANGLES, PickError, PickObject, PickObjectKind, PickQuery, PickTarget};
    use crate::{
        geometry::{ShadingMode, fixtures},
        scene::{Camera, SharedMesh},
    };

    type TestFace = (Range<u32>, bool);
    type TestItem = (SharedMesh, DVec3, Quat, Vec3, PickTarget, ShadingMode, Vec<TestFace>);

    #[test]
    fn backface_needs_double_sided() {
        let mesh = shared_mesh();
        let (triangle, indices) = first_triangle(&mesh);
        let (front_camera, center, front_ray) = camera_for_triangle(triangle);
        assert_hit_near(
            &[object_item(mesh.clone(), DVec3::ZERO, 1, PickObjectKind::Prim, ShadingMode::Studio, vec![(indices.clone(), false)])],
            front_camera,
            front_ray,
            center,
        );

        let back_camera = Camera::new(center + front_ray * 5.0, -front_ray.as_vec3(), 1.0, [128, 128]).unwrap();
        let back_ray = back_camera.direction().as_dvec3();
        assert_eq!(
            pick_items(
                &[object_item(mesh.clone(), DVec3::ZERO, 1, PickObjectKind::Prim, ShadingMode::Studio, vec![(indices.clone(), false)])],
                back_camera,
                back_ray
            ),
            Ok(None)
        );
        assert_hit_near(
            &[object_item(mesh, DVec3::ZERO, 1, PickObjectKind::Prim, ShadingMode::Studio, vec![(indices, true)])],
            back_camera,
            back_ray,
            center,
        );
    }

    #[test]
    fn closest_hit_keeps_identity() {
        let mesh = shared_mesh();
        let (triangle, indices) = first_triangle(&mesh);
        let (camera, center, ray) = camera_for_triangle(triangle);
        let position_at = |distance| camera.position() + ray * distance - center;
        let first_position = position_at(1.0);
        let items = [
            object_item(mesh.clone(), first_position, 7, PickObjectKind::Prim, ShadingMode::Studio, vec![(indices.clone(), false)]),
            object_item(mesh, position_at(2.0), 8, PickObjectKind::Avatar, ShadingMode::Studio, vec![(indices, false)]),
        ];
        let hit = pick_items(&items, camera, ray).unwrap().unwrap();
        assert!(hit.position.distance(camera.position() + ray) < 1e-4);
        let PickTarget::Object(object) = hit.target else { panic!("nearest item is an object") };
        assert_eq!(object.id, Uuid::from_u128(7));
        assert_eq!(object.kind, PickObjectKind::Prim);
        let avatar = pick_items(&items[1..], camera, ray).unwrap().unwrap();
        let PickTarget::Object(object) = avatar.target else { panic!("avatar is an object") };
        assert_eq!(object.id, Uuid::from_u128(8));
        assert_eq!(object.kind, PickObjectKind::Avatar);
    }

    #[test]
    fn min_distance_skips_near_hit() {
        let mesh = shared_mesh();
        let (triangle, indices) = first_triangle(&mesh);
        let (camera, center, ray) = camera_for_triangle(triangle);
        let position_at = |distance| camera.position() + ray * distance - center;
        let items = [
            object_item(mesh.clone(), position_at(0.01), 1, PickObjectKind::Prim, ShadingMode::Studio, vec![(indices.clone(), false)]),
            object_item(mesh, position_at(0.1), 2, PickObjectKind::Prim, ShadingMode::Studio, vec![(indices, false)]),
        ];
        assert_hit_near(&items, camera, ray, camera.position() + ray * 0.1);
    }

    #[test]
    fn terrain_picks_past_draw_distance_but_objects_do_not() {
        let mesh = shared_mesh();
        let (triangle, indices) = first_triangle(&mesh);
        let (camera, center, ray) = camera_for_triangle(triangle);
        let camera = camera.with_quality(32.0, 1.0).unwrap();
        let position = camera.position() + ray * 40.0 - center;
        let object = [object_item(mesh.clone(), position, 1, PickObjectKind::Prim, ShadingMode::Studio, vec![(indices.clone(), true)])];
        let terrain = [item(mesh, position, Quat::IDENTITY, Vec3::ONE, PickTarget::Terrain, ShadingMode::Terrain, vec![(indices, true)])];
        assert_eq!(pick_items(&object, camera, ray), Ok(None));
        let hit = pick_items(&terrain, camera, ray).unwrap().unwrap();
        assert_eq!(hit.target, PickTarget::Terrain);
        assert!(hit.position.distance(camera.position() + ray * 40.0) < 1e-4);
    }

    #[test]
    fn work_limit_follows_bounds() {
        let mesh = shared_mesh();
        let (triangle, indices) = first_triangle(&mesh);
        let (camera, _, ray) = camera_for_triangle(triangle);
        let repeated = (0..=MAX_PICK_TRIANGLES).map(|_| (&indices, false));
        let mut query = PickQuery::new(camera, ray);
        query
            .visit_item(&mesh, DVec3::new(0.0, 100.0, 0.0), Quat::IDENTITY, Vec3::ONE, PickTarget::Terrain, ShadingMode::Studio, repeated)
            .unwrap();
        assert_eq!(query.result(), None);
        let repeated = (0..=MAX_PICK_TRIANGLES).map(|_| (&indices, false));
        let mut query = PickQuery::new(camera, ray);
        assert_eq!(
            query.visit_item(&mesh, DVec3::ZERO, Quat::IDENTITY, Vec3::ONE, PickTarget::Terrain, ShadingMode::Studio, repeated),
            Err(PickError::TriangleLimitExceeded)
        );
    }

    fn pick_items(items: &[TestItem], camera: Camera, ray: DVec3) -> Result<Option<super::PickHit>, PickError> {
        let mut query = PickQuery::new(camera, ray);
        for (mesh, position, rotation, scale, target, shading, faces) in items {
            query.visit_item(
                mesh,
                *position,
                *rotation,
                *scale,
                *target,
                *shading,
                faces.iter().map(|(range, double_sided)| (range, *double_sided)),
            )?;
        }
        Ok(query.result())
    }

    fn shared_mesh() -> SharedMesh {
        let (_scene, view, _) = fixtures::scene(255);
        let (_, mesh) = view.objects().next().expect("fixture object").mesh().expect("fixture mesh");
        mesh.clone()
    }

    fn first_triangle(mesh: &SharedMesh) -> ([Vec3; 3], Range<u32>) {
        let face = mesh.mesh().faces().first().expect("fixture face");
        let indices = face.indices.start..face.indices.start + 3;
        let triangle_indices = mesh.mesh().indices().get(indices.start as usize..indices.end as usize).expect("fixture indices");
        let triangle = [
            mesh.mesh().vertices().get(triangle_indices[0] as usize).expect("fixture vertex").position,
            mesh.mesh().vertices().get(triangle_indices[1] as usize).expect("fixture vertex").position,
            mesh.mesh().vertices().get(triangle_indices[2] as usize).expect("fixture vertex").position,
        ];
        (triangle, indices)
    }

    fn camera_for_triangle(triangle: [Vec3; 3]) -> (Camera, DVec3, DVec3) {
        let [a, b, c] = triangle.map(Vec3::as_dvec3);
        let center = (a + b + c) / 3.0;
        let normal = (b - a).cross(c - a).normalize();
        let direction = -normal;
        let camera = Camera::new(center - direction * 5.0, direction.as_vec3(), 1.0, [128, 128]).unwrap();
        (camera, center, camera.direction().as_dvec3())
    }

    fn assert_hit_near(items: &[TestItem], camera: Camera, ray: DVec3, expected: DVec3) {
        let picked = pick_items(items, camera, ray).expect("fixture stays within the pick work limit");
        let point = picked.expect("expected a ray hit").position;
        assert!(point.distance(expected) < 1e-4, "picked {point:?}, expected {expected:?}");
    }

    fn object_item(
        mesh: SharedMesh,
        position: DVec3,
        id: u128,
        kind: PickObjectKind,
        shading: ShadingMode,
        faces: Vec<TestFace>,
    ) -> TestItem {
        item(
            mesh,
            position,
            Quat::IDENTITY,
            Vec3::ONE,
            PickTarget::Object(PickObject { id: Uuid::from_u128(id), kind, position, rotation: Quat::IDENTITY, scale: Vec3::ONE }),
            shading,
            faces,
        )
    }

    fn item(
        mesh: SharedMesh,
        position: DVec3,
        rotation: Quat,
        scale: Vec3,
        target: PickTarget,
        shading: ShadingMode,
        faces: Vec<TestFace>,
    ) -> TestItem {
        (mesh, position, rotation, scale, target, shading, faces)
    }
}
