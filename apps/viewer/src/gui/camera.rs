use std::{
    f64::consts::{FRAC_PI_3, PI, TAU},
    time::{Duration, Instant},
};

use math::{DQuat, DVec3, ray_intersects_bounds, ray_triangle_intersection};
use render::scene;

use crate::DEFAULT_DRAW_DISTANCE;

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

struct Transition {
    start: Instant,
    position: DVec3,
    focus: DVec3,
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
        if !ray_intersects_bounds(origin, direction, instance.mesh.mesh().bounds(), closest) {
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
            let triangle = indices.map(|index| mesh.vertices()[index as usize].position);
            let Some(distance) = ray_triangle_intersection(origin, direction, triangle, double_sided) else { continue };
            if distance >= 0.05 && distance < closest {
                closest = distance;
                found = true;
            }
        }
    }
    Ok(found.then(|| camera.position() + ray * f64::from(closest)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_eases() {
        let now = Instant::now();
        let mut camera = OrbitCamera::new(DVec3::ZERO);
        let before = camera.scene_camera([1280, 720], now).unwrap();
        let point = DVec3::new(10.0, 5.0, 0.0);
        assert!(!camera.moving(now));
        camera.focus(point, now);
        assert!(camera.moving(now));
        let after = camera.scene_camera([1280, 720], now).unwrap();
        assert!(before.direction().distance(after.direction()) < 1e-6, "selecting focus must not snap the view");
        let halfway = now + FOCUS_TIME / 2;
        let (position, focus) = camera.position_and_focus(halfway);
        assert_eq!(focus, point * 0.5);
        assert_eq!(position, before.position());
        let end = camera.scene_camera([1280, 720], now + FOCUS_TIME).unwrap();
        assert!(!camera.moving(now + FOCUS_TIME), "redraws stop at the end of the focus transition");
        assert!(end.direction().distance((point - position).normalize().as_vec3()) < 1e-6);
        assert_eq!(camera.position_and_focus(now + FOCUS_TIME * 2).1, point);

        let visible = camera.scene_camera([1280, 720], halfway).unwrap();
        camera.focus(DVec3::new(20.0, -5.0, 3.0), halfway);
        let interrupted = camera.scene_camera([1280, 720], halfway).unwrap();
        assert_eq!(visible.position(), interrupted.position());
        assert!(visible.direction().distance(interrupted.direction()) < 1e-6);
        camera.drag([40.0, 0.0], 1000.0, true, false);
        assert!(camera.scene_camera([1280, 720], halfway + FOCUS_TIME / 2).unwrap().position().distance(position) > 0.1);
    }

    #[test]
    fn focus_keeps_position() {
        let home = DVec3::new(1e9, 2e9, 50.0);
        let now = Instant::now();
        let mut camera = OrbitCamera::new(home);
        let position = camera.scene_camera([1280, 720], now).unwrap().position();
        camera.focus(home + DVec3::new(10.0, 2.0, 3.0), now);
        for elapsed in [Duration::ZERO, FOCUS_TIME / 2, FOCUS_TIME] {
            assert!(camera.scene_camera([1280, 720], now + elapsed).unwrap().position().distance(position) < 1e-6);
        }
        assert_eq!(camera.focus, home + DVec3::new(10.0, 2.0, 3.0));
    }

    #[test]
    fn orbit_and_pan() {
        let mut camera = OrbitCamera::new(DVec3::ZERO);
        camera.drag([250.0, 0.0], 1000.0, true, false);
        assert!(camera.offset.distance(DVec3::new(0.0, 4.0, 2.0)) < 1e-10);
        assert_eq!(camera.focus, DVec3::ZERO);
        let offset = camera.offset;
        camera.drag([100.0, 0.0], 1000.0, true, true);
        assert_eq!(camera.offset, offset);
        assert!(camera.focus.distance(DVec3::new(20.0f64.sqrt() * 0.3, 0.0, 0.0)) < 1e-10);
        camera.drag([0.0, 1000.0], 1000.0, true, false);
        assert!((camera.offset.normalize().z.acos() - PI / 180.0).abs() < 1e-10);
    }

    #[test]
    fn zoom_limits() {
        let mut camera = OrbitCamera::new(DVec3::ZERO);
        camera.zoom(0.0001);
        assert!((camera.offset.length() - 0.5).abs() < 1e-10);
        camera.focus(DVec3::X, Instant::now());
        camera.zoom(0.0001);
        assert!((camera.offset.length() - 0.02).abs() < 1e-10);
        camera.zoom(1e6);
        assert!((camera.offset.length() - 240.0).abs() < 1e-10);
        camera.draw_distance = 32.0;
        camera.lod_factor = 2.0;
        camera.zoom(1e6);
        assert!((camera.offset.length() - 31.0).abs() < 1e-10);
        camera.reset();
        assert_eq!((camera.draw_distance, camera.lod_factor), (32.0, 2.0));
    }
}
