use math::{DQuat, DVec3};

use std::{
    f64::consts::{FRAC_PI_3, PI, TAU},
    time::{Duration, Instant},
};

use crate::DEFAULT_DRAW_DISTANCE;

#[cfg(test)]
#[path = "camera_tests.rs"]
mod tests;

const FOCUS_TIME: Duration = Duration::from_millis(400);

pub(crate) struct OrbitCamera {
    focus: DVec3,
    offset: DVec3,
    home: DVec3,
    minimum: f64,
    transition: Option<Transition>,
    pub(crate) draw_distance: f32,
    pub(crate) lod_factor: f32,
}

struct Transition {
    start: Instant,
    position: DVec3,
    focus: DVec3,
}

impl OrbitCamera {
    pub(crate) fn moving(&self, now: Instant) -> bool {
        self.transition.as_ref().is_some_and(|transition| now.saturating_duration_since(transition.start) < FOCUS_TIME)
    }
    pub(crate) fn new(position: DVec3) -> Self {
        Self {
            focus: position,
            offset: DVec3::new(-4.0, 0.0, 2.0),
            home: position,
            minimum: 0.5,
            transition: None,
            draw_distance: DEFAULT_DRAW_DISTANCE,
            lod_factor: 1.0,
        }
    }

    pub(crate) fn scene_camera(&self, viewport: [u32; 2], now: Instant) -> Option<scene::Camera> {
        let (position, focus) = self.position_and_focus(now);
        scene::Camera::new(position, (focus - position).as_vec3(), FRAC_PI_3, viewport)
            .and_then(|camera| camera.with_quality(f64::from(self.draw_distance), f64::from(self.lod_factor)))
            .ok()
    }

    fn position_and_focus(&self, now: Instant) -> (DVec3, DVec3) {
        let position = self.focus + self.offset;
        let Some(transition) = &self.transition else {
            return (position, self.focus);
        };
        let time = now.saturating_duration_since(transition.start).as_secs_f64() / FOCUS_TIME.as_secs_f64();
        if time >= 1.0 {
            return (position, self.focus);
        }
        let fraction = time * time * (3.0 - 2.0 * time);
        (transition.position.lerp(position, fraction), transition.focus.lerp(self.focus, fraction))
    }

    pub(crate) fn focus(&mut self, point: DVec3, now: Instant) {
        let (position, focus) = self.position_and_focus(now);
        let offset = position - point;
        if point.is_finite() && offset.length() >= 0.02 {
            self.transition = Some(Transition { start: now, position, focus });
            self.focus = point;
            self.offset = offset;
            self.minimum = 0.02;
        }
    }

    pub(crate) fn reset(&mut self) {
        let (draw_distance, lod_factor) = (self.draw_distance, self.lod_factor);
        *self = Self::new(self.home);
        self.draw_distance = draw_distance;
        self.lod_factor = lod_factor;
    }

    pub(crate) fn drag(&mut self, delta: [f64; 2], width: f64, control: bool, shift: bool) {
        if width <= 0.0 {
            return;
        }
        let [dx, dy] = delta;
        let direction = -self.offset.normalize();
        let left = DVec3::Z.cross(direction).normalize_or(DVec3::Y);
        if control && shift {
            let up = direction.cross(left);
            self.focus += (left * dx + up * dy) * (3.0 * self.offset.length() / width);
        } else {
            self.offset = DQuat::from_rotation_z(-dx * TAU / width) * self.offset;
            if control {
                let angle = self.offset.normalize().z.clamp(-1.0, 1.0).acos();
                let next = (angle - dy * TAU / width).clamp(PI / 180.0, PI * 179.0 / 180.0);
                let left = DVec3::Z.cross(-self.offset).normalize_or(DVec3::Y);
                self.offset = DQuat::from_axis_angle(left, angle - next) * self.offset;
            } else {
                self.zoom(0.99f64.powf(-dy));
            }
        }
    }

    pub(crate) fn zoom(&mut self, fraction: f64) {
        let distance = (self.offset.length() * fraction).clamp(self.minimum, 240.0f64.min(f64::from(self.draw_distance) - 1.0));
        self.offset = self.offset.normalize() * distance;
    }
}

pub(crate) fn pick_world_point(
    instances: &[render::Instance],
    camera: scene::Camera,
    cursor: [f64; 2],
    size: [u32; 2],
) -> Result<Option<DVec3>, ()> {
    let forward = camera.direction().as_dvec3();
    let right = forward.cross(DVec3::Z).normalize_or(DVec3::X);
    let up = right.cross(forward);
    let height = f64::from(size[1]);
    let tangent = (camera.vertical_fov() * 0.5).tan();
    let ray = (forward
        + right * ((2.0 * cursor[0] - f64::from(size[0])) / height * tangent)
        + up * ((height - 2.0 * cursor[1]) / height * tangent))
        .normalize();
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
        let bounds = instance.mesh.mesh().bounds();
        let mut near = 0.0f32;
        let mut far = closest;
        for axis in 0..3 {
            if direction[axis].abs() < 1e-20 {
                if origin[axis] < bounds.min[axis] || origin[axis] > bounds.max[axis] {
                    far = -1.0;
                    break;
                }
            } else {
                let a = (bounds.min[axis] - origin[axis]) / direction[axis];
                let b = (bounds.max[axis] - origin[axis]) / direction[axis];
                near = near.max(a.min(b));
                far = far.min(a.max(b));
            }
        }
        if near > far {
            continue;
        }
        let mesh = instance.mesh.mesh();
        for (indices, double_sided) in instance.faces.iter().flat_map(|face| {
            mesh.indices()[face.indices.start as usize..face.indices.end as usize]
                .as_chunks::<3>()
                .0
                .iter()
                .map(move |indices| (indices, face.double_sided))
        }) {
            if work == 250_000 {
                return Err(());
            }
            work += 1;
            let [a, b, c] = indices.map(|index| mesh.vertices()[index as usize].position);
            let edge = b - a;
            let other = c - a;
            let p = direction.cross(other);
            let determinant = edge.dot(p);
            if (double_sided && determinant.abs() <= 1e-12) || (!double_sided && determinant <= 1e-12) {
                continue;
            }
            let t = origin - a;
            let u = t.dot(p) / determinant;
            if !(0.0..=1.0).contains(&u) {
                continue;
            }
            let q = t.cross(edge);
            let v = direction.dot(q) / determinant;
            if v < 0.0 || u + v > 1.0 {
                continue;
            }
            let distance = other.dot(q) / determinant;
            if distance >= 0.05 && distance < closest {
                closest = distance;
                found = true;
            }
        }
    }
    Ok(found.then(|| camera.position() + ray * f64::from(closest)))
}
