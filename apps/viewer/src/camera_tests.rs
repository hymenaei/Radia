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
