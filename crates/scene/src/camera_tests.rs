use super::*;

#[test]
fn detail_hysteresis() {
    let camera = Camera::new(DVec3::ZERO, Vec3::X, PI / 2.0, [1000, 1000]).unwrap();
    let center = DVec3::new(100.0, 0.0, 0.0);
    assert_eq!(camera.detail(center, 1.0, None), Some(Detail::Lowest));
    assert_eq!(camera.detail(center, 5.0, None), Some(Detail::Low));
    assert_eq!(camera.detail(center, 20.0, None), Some(Detail::Medium));
    assert_eq!(camera.detail(center, 40.0, None), Some(Detail::High));
    assert_eq!(camera.detail(center, 2.6, Some(Detail::Lowest)), Some(Detail::Lowest));
    assert_eq!(camera.detail(center, 2.6, Some(Detail::Low)), Some(Detail::Low));
    assert_eq!(camera.detail(center, 1.8, Some(Detail::Low)), Some(Detail::Lowest));
    assert_eq!(camera.detail(center, 3.0, Some(Detail::Lowest)), Some(Detail::Low));
    assert_eq!(camera.detail(DVec3::X, 2.0, None), Some(Detail::High));
    assert_eq!(camera.detail(-DVec3::X * 10.0, 2.0, None), None);
    let translated = Camera::new(DVec3::splat(1e12), Vec3::X, PI / 2.0, [1000, 1000]).unwrap();
    assert_eq!(translated.detail(DVec3::splat(1e12) + center, 20.0, None), Some(Detail::Medium));
}

#[test]
fn camera_input_limits() {
    assert_eq!(Camera::new(DVec3::NAN, Vec3::X, 1.0, [1000, 1000]), Err(CameraError::Position));
    for direction in [Vec3::ZERO, Vec3::NAN, Vec3::INFINITY] {
        assert_eq!(Camera::new(DVec3::ZERO, direction, 1.0, [1000, 1000]), Err(CameraError::Direction));
    }
    for fov in [0.0, -1.0, PI, f64::NAN, f64::INFINITY] {
        assert_eq!(Camera::new(DVec3::ZERO, Vec3::X, fov, [1000, 1000]), Err(CameraError::Projection));
    }
    assert_eq!(Camera::new(DVec3::ZERO, Vec3::X, 1.0, [0, 0]), Err(CameraError::Projection));
    assert_eq!(Camera::new(DVec3::ZERO, Vec3::X, 1.0, [0, 1000]), Err(CameraError::Projection));
    assert_eq!(Camera::new(DVec3::ZERO, Vec3::X, 1.0, [1000, 0]), Err(CameraError::Projection));
    let camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [1000, 1000]).unwrap();
    for distance in [32.0, 512.0, 1024.0] {
        assert_eq!(camera.with_quality(distance, 1.0).unwrap().draw_distance(), distance);
    }
    assert_eq!(camera.detail(DVec3::X * 900.0, 1.0, None), None);
    assert_eq!(camera.with_quality(1024.0, 1.0).unwrap().detail(DVec3::X * 900.0, 1.0, None), Some(Detail::Lowest));
    for distance in [0.0, 31.0, 1024.01, f64::NAN, f64::INFINITY] {
        assert_eq!(camera.with_quality(distance, 1.0), Err(CameraError::Quality));
    }
    for factor in [0.0, 0.24, 4.01, f64::NAN, f64::INFINITY] {
        assert_eq!(camera.with_quality(512.0, factor), Err(CameraError::Quality));
    }
    for radius in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(camera.detail(DVec3::X, radius, None), None);
    }
    assert_eq!(camera.detail(DVec3::NAN, 1.0, None), None);
}

#[test]
fn texture_detail_tracks_projection() {
    let camera = Camera::new(DVec3::ZERO, Vec3::X, PI / 2.0, [1000, 1000]).unwrap();
    assert_eq!(camera.texture_size(DVec3::X * 10.0, 2.0, None).edge(), 128);
    assert_eq!(camera.texture_size(DVec3::X * 100.0, 2.0, None).edge(), 32);
    assert_eq!(camera.texture_size(DVec3::X, 2.0, None).edge(), 1024);
    let previous = Some(TextureSize::new(128));
    assert_eq!(camera.texture_size(DVec3::X * 7.0, 2.0, previous).edge(), 128);
    assert_eq!(camera.texture_size(DVec3::X * 6.0, 2.0, previous).edge(), 256);
    assert_eq!(camera.texture_size(DVec3::X * 25.0, 2.0, previous).edge(), 64);
    assert_eq!(camera.texture_size(DVec3::X * 100.0, 2.0, Some(TextureSize::new(64))).edge(), 32);
    let tall = Camera::new(DVec3::ZERO, Vec3::X, PI / 2.0, [2000, 2000]).unwrap();
    assert_eq!(tall.texture_size(DVec3::X * 10.0, 2.0, None).edge(), 256);
}

#[test]
fn importance_tracks_view_bounds() {
    let square = Camera::new(DVec3::ZERO, Vec3::X, PI / 2.0, [1000, 1000]).unwrap();
    let wide = Camera::new(DVec3::ZERO, Vec3::X, PI / 2.0, [2000, 1000]).unwrap();
    assert!(square.importance(DVec3::new(10.0, 9.0, 0.0), 1.0).is_some());
    assert!(square.importance(DVec3::new(10.0, 11.0, 0.0), 1.0).is_some(), "edge intersections remain visible");
    assert_eq!(square.importance(DVec3::new(10.0, 12.0, 0.0), 1.0), None);
    assert!(wide.importance(DVec3::new(10.0, 18.0, 0.0), 1.0).is_some());
    assert_eq!(wide.importance(DVec3::new(10.0, 0.0, 12.0), 1.0), None);
    assert_eq!(wide.importance(-DVec3::X * 10.0, 1.0), None);
    assert_eq!(wide.importance(DVec3::X * 600.0, 1.0), None);
    assert!(square.importance(DVec3::X * 10.0, 1.0) > square.importance(DVec3::X * 100.0, 1.0));
    let vertical = Camera::new(DVec3::ZERO, Vec3::Z, PI / 2.0, [1000, 1000]).unwrap();
    assert!(vertical.importance(DVec3::Z * 10.0, 1.0).is_some());
    assert_eq!(vertical.importance(DVec3::new(12.0, 0.0, 10.0), 1.0), None);
}
