use crate::{DVec3, DVec4, Mat4};

#[derive(Clone, Copy, Debug)]
pub struct Frustum {
    planes: [(DVec4, f64); 6],
}

impl Frustum {
    pub fn from_clip_matrix(matrix: Mat4) -> Self {
        let rows = matrix.as_dmat4().transpose();
        Self {
            planes: [
                rows.w_axis + rows.x_axis,
                rows.w_axis - rows.x_axis,
                rows.w_axis + rows.y_axis,
                rows.w_axis - rows.y_axis,
                rows.z_axis,
                rows.w_axis - rows.z_axis,
            ]
            .map(|plane| (plane, plane.truncate().length())),
        }
    }

    pub fn intersects_sphere(&self, center: DVec3, radius: f64) -> bool {
        center.is_finite()
            && radius.is_finite()
            && radius >= 0.0
            && self.planes.iter().all(|(plane, length)| plane.truncate().dot(center) + plane.w >= -radius * length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Vec3,
        camera::rh::{proj::directx::perspective, view::look_to_mat4},
    };

    #[test]
    fn points_match_clip_space() {
        for (near, far) in [(0.05, 256.0), (256.0, 0.05)] {
            let matrix = perspective(1.0, 1.0, near, far) * look_to_mat4(Vec3::ZERO, Vec3::X, Vec3::Z);
            let frustum = Frustum::from_clip_matrix(matrix);
            for point in [
                Vec3::new(10.0, 0.0, 0.0),
                Vec3::new(-10.0, 0.0, 0.0),
                Vec3::new(10.0, 100.0, 0.0),
                Vec3::new(0.01, 0.0, 0.0),
                Vec3::new(10.0, 0.0, 100.0),
                Vec3::new(300.0, 0.0, 0.0),
            ] {
                let clip = matrix * point.extend(1.0);
                let expected =
                    clip.x >= -clip.w && clip.x <= clip.w && clip.y >= -clip.w && clip.y <= clip.w && clip.z >= 0.0 && clip.z <= clip.w;
                assert_eq!(frustum.intersects_sphere(point.as_dvec3(), 0.0), expected, "{point:?}, near {near}, far {far}");
            }
        }
    }

    #[test]
    fn spheres_touch_each_plane() {
        let frustum = Frustum::from_clip_matrix(Mat4::IDENTITY);
        for center in [
            DVec3::new(-1.5, 0.0, 0.5),
            DVec3::new(1.5, 0.0, 0.5),
            DVec3::new(0.0, -1.5, 0.5),
            DVec3::new(0.0, 1.5, 0.5),
            DVec3::new(0.0, 0.0, -0.5),
            DVec3::new(0.0, 0.0, 1.5),
        ] {
            assert!(frustum.intersects_sphere(center, 0.5), "touching sphere at {center:?}");
            assert!(!frustum.intersects_sphere(center, 0.49), "separated sphere at {center:?}");
        }
        assert!(!frustum.intersects_sphere(DVec3::NAN, 0.5));
        assert!(!frustum.intersects_sphere(DVec3::ZERO, f64::NAN));
        assert!(!frustum.intersects_sphere(DVec3::ZERO, -1.0));
    }
}
