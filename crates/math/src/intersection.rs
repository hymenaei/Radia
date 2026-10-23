use crate::{Bounds, Vec3};

pub fn ray_intersects_bounds(origin: Vec3, direction: Vec3, bounds: Bounds, max_parameter: f32) -> bool {
    if !origin.is_finite()
        || !direction.is_finite()
        || direction == Vec3::ZERO
        || !bounds.min.is_finite()
        || !bounds.max.is_finite()
        || !bounds.min.cmple(bounds.max).all()
        || max_parameter.is_nan()
        || max_parameter < 0.0
    {
        return false;
    }
    let mut near = 0.0f64;
    let mut far = f64::from(max_parameter);
    for axis in 0..3 {
        if direction[axis] == 0.0 {
            if origin[axis] < bounds.min[axis] || origin[axis] > bounds.max[axis] {
                return false;
            }
        } else {
            let origin = f64::from(origin[axis]);
            let direction = f64::from(direction[axis]);
            let a = (f64::from(bounds.min[axis]) - origin) / direction;
            let b = (f64::from(bounds.max[axis]) - origin) / direction;
            near = near.max(a.min(b));
            far = far.min(a.max(b));
        }
    }
    near <= far
}

pub fn ray_triangle_intersection(origin: Vec3, direction: Vec3, triangle: [Vec3; 3], double_sided: bool) -> Option<f32> {
    let origin = origin.as_dvec3();
    let direction = direction.as_dvec3();
    let [a, b, c] = triangle.map(Vec3::as_dvec3);
    let edge = b - a;
    let other = c - a;
    let p = direction.cross(other);
    let determinant = edge.dot(p);
    if !determinant.is_finite() || (double_sided && determinant == 0.0) || (!double_sided && determinant <= 0.0) {
        return None;
    }
    let t = origin - a;
    let u = t.dot(p) / determinant;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = t.cross(edge);
    let v = direction.dot(q) / determinant;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let parameter = other.dot(q) / determinant;
    if parameter < 0.0 {
        return None;
    }
    let parameter = parameter as f32;
    parameter.is_finite().then_some(parameter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_include_tangent_hits() {
        let bounds = Bounds { min: Vec3::splat(-1.0), max: Vec3::ONE };
        for (origin, direction, limit, expected) in [
            (Vec3::new(-2.0, 0.0, 0.0), Vec3::X, 1.0, true),
            (Vec3::new(-2.0, 1.0, 0.0), Vec3::X, 1.0, true),
            (Vec3::new(-2.0, 0.0, 0.0), Vec3::X, 0.5, false),
            (Vec3::new(-2.0, 2.0, 0.0), Vec3::X, 8.0, false),
            (Vec3::new(-2.0, 0.0, 0.0), -Vec3::X, 8.0, false),
            (Vec3::ZERO, Vec3::Z, 0.0, true),
            (Vec3::ZERO, Vec3::ZERO, 8.0, false),
            (Vec3::splat(f32::NAN), Vec3::Z, 8.0, false),
        ] {
            assert_eq!(ray_intersects_bounds(origin, direction, bounds, limit), expected, "{origin:?}, {direction:?}, {limit}");
        }
    }

    #[test]
    fn bounds_preserve_extreme_parameters() {
        let bounds = Bounds { min: Vec3::splat(-1.0), max: Vec3::ONE };
        assert!(ray_intersects_bounds(Vec3::new(-2.0, 0.0, 0.0), Vec3::X * 1e-21, bounds, 1.5e21));
        assert!(ray_intersects_bounds(Vec3::new(2.0, 0.0, 0.0), Vec3::NEG_X * 1e-21, bounds, 1.5e21));
        assert!(!ray_intersects_bounds(Vec3::new(-2.0, 0.0, 0.0), Vec3::X * 1e-21, bounds, 0.5e21));

        let bounds = Bounds { min: Vec3::new(f32::MAX * 0.5, -1.0, -1.0), max: Vec3::new(f32::MAX, 1.0, 1.0) };
        assert!(ray_intersects_bounds(Vec3::new(-f32::MAX, 0.0, 0.0), Vec3::X * f32::MAX, bounds, 2.0));
        assert!(!ray_intersects_bounds(Vec3::new(-f32::MAX, 0.0, 0.0), Vec3::X * f32::MAX, bounds, 1.0));
    }

    #[test]
    fn triangles_preserve_extreme_parameters() {
        let triangle = [Vec3::new(-1.0, -1.0, 5.0), Vec3::new(0.0, 1.0, 5.0), Vec3::new(1.0, -1.0, 5.0)];
        assert_eq!(ray_triangle_intersection(Vec3::ZERO, Vec3::Z * 2.0_f32.powi(-48), triangle, false), Some(5.0 * 2.0_f32.powi(48)));
        assert_eq!(ray_triangle_intersection(Vec3::ZERO, Vec3::Z, triangle.map(|vertex| vertex * 1e20), false), Some(5e20));
        assert_eq!(ray_triangle_intersection(Vec3::ZERO, Vec3::Z, triangle.map(|vertex| vertex * 1e-20), false), Some(5e-20));
        let behind = triangle.map(|vertex| Vec3::new(vertex.x, vertex.y, -f32::from_bits(1)));
        assert_eq!(ray_triangle_intersection(Vec3::ZERO, Vec3::Z * f32::MAX, behind, false), None);
    }

    #[test]
    fn triangles_preserve_ray_parameter() {
        let a = Vec3::new(-1.0, -1.0, 5.0);
        let b = Vec3::new(1.0, -1.0, 5.0);
        let c = Vec3::new(0.0, 1.0, 5.0);
        assert_eq!(ray_triangle_intersection(Vec3::ZERO, Vec3::Z, [a, c, b], false), Some(5.0));
        assert_eq!(ray_triangle_intersection(Vec3::ZERO, Vec3::Z * 2.0, [a, c, b], false), Some(2.5));
        assert_eq!(ray_triangle_intersection(Vec3::ZERO, Vec3::Z, [a, b, c], false), None);
        assert_eq!(ray_triangle_intersection(Vec3::ZERO, Vec3::Z, [a, b, c], true), Some(5.0));
        assert_eq!(ray_triangle_intersection(Vec3::new(0.0, -1.0, 0.0), Vec3::Z, [a, c, b], false), Some(5.0));
        assert_eq!(ray_triangle_intersection(Vec3::ZERO, -Vec3::Z, [a, b, c], true), None);
        assert_eq!(ray_triangle_intersection(Vec3::ZERO, Vec3::X, [a, b, c], true), None);
        assert_eq!(ray_triangle_intersection(Vec3::new(3.0, 0.0, 0.0), Vec3::Z, [a, b, c], true), None);
        assert_eq!(ray_triangle_intersection(Vec3::ZERO, Vec3::Z, [a, a, c], true), None);
    }
}
