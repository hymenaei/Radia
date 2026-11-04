use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    ops::Range,
};

use math::{DVec3, Quat, Vec3, ray_intersects_bounds, ray_triangle_intersection};

use super::items::ItemId;
use super::{GeometryRenderer, ShadingMode};
use crate::scene::{Camera, Object, SharedMesh};

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
        let mut query = PickQuery::new(camera, ray);
        if !ray.is_finite() {
            self.log_pick(&query, PickOutcome::InvalidRay);
            return Ok(None);
        }
        for id in &self.cache.visible {
            let Some(item) = self.cache.items.get(*id) else { continue };
            if !item.pickable {
                continue;
            }
            let faces =
                item.faces.iter().filter(|face| face.slot.is_some()).map(|face| (&face.indices, face.params.double_sided, face.order.3));
            if let Err(error) =
                query.visit_item(item.id, &item.mesh, item.position, item.rotation, item.scale, item.invert_normals, item.shading, faces)
            {
                self.log_pick(&query, PickOutcome::TriangleLimitExceeded);
                return Err(error);
            }
        }
        let outcome = if query.hit.is_some() { PickOutcome::Hit } else { PickOutcome::Miss };
        let result = query.result();
        self.log_pick(&query, outcome);
        Ok(result)
    }

    fn log_pick(&self, query: &PickQuery, outcome: PickOutcome) {
        let Some(hit) = query.hit.as_ref().filter(|_| outcome == PickOutcome::Hit) else {
            tracing::info!(
                outcome = ?outcome,
                triangles_tested = query.work,
                visible_items = self.cache.visible.len(),
                "geometry pick"
            );
            return;
        };
        let Some(item) = self.cache.index(hit.item_id).and_then(|index| self.cache.items.get(index)) else {
            tracing::info!(
                outcome = ?PickOutcome::HitItemUnavailable,
                triangles_tested = query.work,
                visible_items = self.cache.visible.len(),
                "geometry pick"
            );
            return;
        };

        let mesh = item.mesh.mesh();
        let source = item.source.as_ref();
        let replacement_source = item.replacement.as_ref().and_then(|replacement| replacement.item.source.as_ref());
        let item_source_mesh = source.and_then(Object::mesh);
        let published_source = replacement_source.or(source);
        let published_mesh = published_source.and_then(Object::mesh);
        let published_source_kind = if replacement_source.is_some() {
            "replacement"
        } else if source.is_some() {
            "cached_item"
        } else {
            "none"
        };
        let resident = self.meshes.get(&item.mesh.id());
        let visible_sharing_count = self
            .cache
            .visible
            .iter()
            .filter(|id| self.cache.items.get(**id).is_some_and(|visible| visible.mesh.id() == item.mesh.id()))
            .count();
        tracing::info!(
            outcome = ?outcome,
            object_uuid = ?source.map(|source| source.detail().summary.id),
            entity = ?item.entity,
            region = ?item.region,
            local_id = ?source.map(|source| source.detail().summary.address.local_id()),
            raw_shape = ?published_source.and_then(|source| source.detail().shape),
            item_source_raw_shape = ?source.and_then(|source| source.detail().shape),
            item_source_mesh_key = ?item_source_mesh.map(|(key, _)| key),
            item_source_mesh_id = ?item_source_mesh.map(|(_, mesh)| mesh.id()),
            published_source = published_source_kind,
            published_mesh_key = ?published_mesh.map(|(key, _)| key),
            published_mesh_id = ?published_mesh.map(|(_, mesh)| mesh.id()),
            item_mesh_id = ?item.mesh.id(),
            item_mesh_bounds = ?mesh.bounds(),
            item_cpu_vertex_count = mesh.vertices().len(),
            item_cpu_index_count = mesh.indices().len(),
            item_cpu_face_count = mesh.faces().len(),
            item_cpu_nonempty_face_count = mesh.faces().iter().filter(|face| !face.indices.is_empty()).count(),
            cached_face_count = item.faces.len(),
            submitted_face_count = item.faces.iter().filter(|face| face.slot.is_some()).count(),
            hit_face_range = ?hit.face_range,
            hit_material_slot = hit.material_slot,
            resident = resident.is_some(),
            resident_vertex_range = ?resident.map(|mesh| mesh.allocation.vertices()),
            resident_index_range = ?resident.map(|mesh| mesh.allocation.indices()),
            visible_sharing_count,
            triangles_tested = query.work,
            "geometry pick"
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PickOutcome {
    Hit,
    Miss,
    InvalidRay,
    TriangleLimitExceeded,
    HitItemUnavailable,
}

#[derive(Debug, Eq, PartialEq)]
struct PickHit {
    item_id: ItemId,
    face_range: Range<u32>,
    material_slot: u8,
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
        item_id: ItemId,
        shared: &SharedMesh,
        position: DVec3,
        rotation: Quat,
        scale: Vec3,
        invert_normals: bool,
        shading: ShadingMode,
        faces: impl Iterator<Item = (&'a Range<u32>, bool, u8)>,
    ) -> Result<(), PickError> {
        let mesh = shared.mesh();
        let max_distance = if shading == ShadingMode::Surface { self.camera.draw_distance() as f32 } else { f32::INFINITY };
        let limit = self.closest.min(max_distance);
        let inverse = rotation.conjugate();
        let origin = inverse * (self.camera.position() - position).as_vec3() / scale;
        let direction = inverse * self.ray.as_vec3() / scale;
        if !origin.is_finite() || !direction.is_finite() || !ray_intersects_bounds(origin, direction, mesh.bounds(), limit) {
            return Ok(());
        }

        for (range, double_sided, material_slot) in faces {
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
                let triangle = if invert_normals {
                    [first.position, third.position, second.position]
                } else {
                    [first.position, second.position, third.position]
                };
                let Some(distance) = ray_triangle_intersection(origin, direction, triangle, double_sided) else { continue };
                if distance < 0.05 || distance >= self.closest || distance >= max_distance {
                    continue;
                }
                self.closest = distance;
                self.hit = Some(PickHit { item_id, face_range: range.clone(), material_slot });
            }
        }
        Ok(())
    }

    fn result(&self) -> Option<DVec3> {
        self.hit.as_ref().map(|_| self.camera.position() + self.ray * f64::from(self.closest))
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use math::{DVec3, Quat, Vec3};

    use super::{MAX_PICK_TRIANGLES, PickError, PickQuery};
    use crate::{
        geometry::{ShadingMode, fixtures},
        scene::{Camera, SharedMesh},
    };

    type TestFace = (Range<u32>, bool, u8);
    type TestItem = (SharedMesh, DVec3, ShadingMode, Vec<TestFace>);

    #[test]
    fn backface_needs_double_sided() {
        let mesh = shared_mesh();
        let (triangle, indices) = first_triangle(&mesh);
        let (front_camera, center, front_ray) = camera_for_triangle(triangle);
        assert_hit_near(
            &[(mesh.clone(), DVec3::ZERO, ShadingMode::Surface, vec![(indices.clone(), false, 0)])],
            front_camera,
            front_ray,
            center,
        );

        let back_camera = Camera::new(center + front_ray * 5.0, -front_ray.as_vec3(), 1.0, [128, 128]).unwrap();
        let back_ray = back_camera.direction().as_dvec3();
        assert_eq!(
            pick_items(&[(mesh.clone(), DVec3::ZERO, ShadingMode::Surface, vec![(indices.clone(), false, 0)])], back_camera, back_ray),
            Ok(None)
        );
        assert_hit_near(&[(mesh, DVec3::ZERO, ShadingMode::Surface, vec![(indices, true, 0)])], back_camera, back_ray, center);
    }

    #[test]
    fn closest_hit_wins() {
        let mesh = shared_mesh();
        let (triangle, indices) = first_triangle(&mesh);
        let (camera, center, ray) = camera_for_triangle(triangle);
        let position_at = |distance| camera.position() + ray * distance - center;
        let items = [
            (mesh.clone(), position_at(1.0), ShadingMode::Surface, vec![(indices.clone(), false, 0)]),
            (mesh, position_at(2.0), ShadingMode::Surface, vec![(indices, false, 0)]),
        ];
        assert_hit_near(&items, camera, ray, camera.position() + ray);
    }

    #[test]
    fn closest_hit_keeps_face_metadata() {
        let mesh = shared_mesh();
        let (triangle, indices) = first_triangle(&mesh);
        let (camera, center, ray) = camera_for_triangle(triangle);
        let position_at = |distance| camera.position() + ray * distance - center;
        let farther = (2, 10, 0);
        let nearer = (2, 20, 0);
        let mut query = PickQuery::new(camera, ray);
        query
            .visit_item(
                farther,
                &mesh,
                position_at(2.0),
                Quat::IDENTITY,
                Vec3::ONE,
                false,
                ShadingMode::Surface,
                std::iter::once((&indices, false, 3)),
            )
            .unwrap();
        query
            .visit_item(
                nearer,
                &mesh,
                position_at(1.0),
                Quat::IDENTITY,
                Vec3::ONE,
                false,
                ShadingMode::Surface,
                std::iter::once((&indices, false, 7)),
            )
            .unwrap();

        let hit = query.hit.as_ref().expect("expected a ray hit");
        assert_eq!(hit.item_id, nearer);
        assert_eq!(hit.face_range, indices);
        assert_eq!(hit.material_slot, 7);
        let point = query.result().expect("expected a picked point");
        assert!(point.distance(camera.position() + ray) < 1e-4, "picked {point:?} from {hit:?}");
    }

    #[test]
    fn min_distance_skips_near_hit() {
        let mesh = shared_mesh();
        let (triangle, indices) = first_triangle(&mesh);
        let (camera, center, ray) = camera_for_triangle(triangle);
        let position_at = |distance| camera.position() + ray * distance - center;
        let items = [
            (mesh.clone(), position_at(0.01), ShadingMode::Surface, vec![(indices.clone(), false, 0)]),
            (mesh, position_at(0.1), ShadingMode::Surface, vec![(indices, false, 0)]),
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
        let object = [(mesh.clone(), position, ShadingMode::Surface, vec![(indices.clone(), true, 0)])];
        let terrain = [(mesh, position, ShadingMode::Terrain, vec![(indices, true, 0)])];
        assert_eq!(pick_items(&object, camera, ray), Ok(None));
        assert_hit_near(&terrain, camera, ray, camera.position() + ray * 40.0);
    }

    #[test]
    fn work_limit_follows_bounds() {
        let mesh = shared_mesh();
        let (triangle, indices) = first_triangle(&mesh);
        let (camera, _, ray) = camera_for_triangle(triangle);
        let repeated = (0..=MAX_PICK_TRIANGLES).map(|_| (&indices, false, 0));
        let mut query = PickQuery::new(camera, ray);
        query
            .visit_item((2, 1, 0), &mesh, DVec3::new(0.0, 100.0, 0.0), Quat::IDENTITY, Vec3::ONE, false, ShadingMode::Surface, repeated)
            .unwrap();
        assert_eq!(query.result(), None);
        let repeated = (0..=MAX_PICK_TRIANGLES).map(|_| (&indices, false, 0));
        let mut query = PickQuery::new(camera, ray);
        assert_eq!(
            query.visit_item((2, 1, 0), &mesh, DVec3::ZERO, Quat::IDENTITY, Vec3::ONE, false, ShadingMode::Surface, repeated),
            Err(PickError::TriangleLimitExceeded)
        );
    }

    fn pick_items(items: &[TestItem], camera: Camera, ray: DVec3) -> Result<Option<DVec3>, PickError> {
        let mut query = PickQuery::new(camera, ray);
        for (index, (mesh, position, shading, faces)) in items.iter().enumerate() {
            query.visit_item(
                (2, index as u128, 0),
                mesh,
                *position,
                Quat::IDENTITY,
                Vec3::ONE,
                false,
                *shading,
                faces.iter().map(|(range, double_sided, slot)| (range, *double_sided, *slot)),
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
        let point = picked.expect("expected a ray hit");
        assert!(point.distance(expected) < 1e-4, "picked {point:?}, expected {expected:?}");
    }
}
