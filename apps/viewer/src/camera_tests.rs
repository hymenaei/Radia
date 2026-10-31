use std::{
    f64::consts::PI,
    time::{Duration, Instant},
};

use math::{DVec3, Quat, Vec3};
use uuid::Uuid;
use world::WorldPose;

#[cfg(any(target_os = "windows", target_os = "linux"))]
use winit::{event::ElementState, keyboard::Key};

use super::{CameraMode, FOCUS_TIME, FollowCamera, OrbitCamera};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use super::{handle_keyboard_input, should_reset_sitting_orbit};
use crate::agent_movement::SittingCameraInput;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::agent_movement::{HeldControls, KeyBinding};
use crate::session::SitCameraOverride;

#[test]
fn follow_camera_uses_official_rear_view_offsets_in_agent_axes() {
    let pose = WorldPose { position: DVec3::new(100.0, 200.0, 30.0), rotation: math::Quat::IDENTITY };
    let now = Instant::now();
    let mut camera = FollowCamera::default();
    let view = camera.follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 512.0, 1.0, now).expect("follow view");
    let (position, focus) = expected_rear_view(pose);
    assert_eq!(view.position(), position);
    assert!(view.direction().distance((focus - position).as_vec3().normalize()) < 1e-6);

    let pose = WorldPose { rotation: math::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), ..pose };
    let mut camera = FollowCamera::default();
    let view =
        camera.follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 512.0, 1.0, now).expect("rotated follow view");
    let (position, focus) = expected_rear_view(pose);
    assert!((view.position() - position).length() < 1e-6);
    assert!(view.direction().distance((focus - position).as_vec3().normalize()) < 1e-6);
}

fn expected_rear_view(pose: WorldPose) -> (DVec3, DVec3) {
    let focus = pose.position + (pose.rotation * Vec3::new(1.0, 0.0, 1.0)).as_dvec3();
    let position = focus + (pose.rotation * Vec3::new(-4.0, 0.0, -0.25)).as_dvec3();
    (position, focus)
}

#[test]
fn follow_camera_moves_rigidly_with_avatar_translation() {
    let start = Instant::now();
    let pose = WorldPose { position: DVec3::new(10.0, 20.0, 30.0), rotation: math::Quat::IDENTITY };
    let mut camera = FollowCamera::default();
    let first = camera
        .follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 512.0, 1.0, start)
        .expect("initial follow camera");
    let translated = WorldPose { position: pose.position + DVec3::new(100.0, -50.0, 20.0), ..pose };
    let second = camera
        .follow_with_sit_input(translated, None, SittingCameraInput::default(), [1280, 720], 512.0, 1.0, start + Duration::from_millis(10))
        .expect("translated follow camera");

    assert_eq!(second.position() - first.position(), DVec3::new(100.0, -50.0, 20.0));
    assert!(!camera.moving(translated, start + Duration::from_millis(10)));
}

#[test]
fn follow_camera_converges_offset_after_turn() {
    let start = Instant::now();
    let pose = WorldPose { position: DVec3::ZERO, rotation: math::Quat::IDENTITY };
    let mut camera = FollowCamera::default();
    camera.follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 512.0, 1.0, start).expect("initial follow camera");
    let turned = WorldPose { rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), ..pose };
    let halfway = camera
        .follow_with_sit_input(turned, None, SittingCameraInput::default(), [1280, 720], 512.0, 1.0, start + Duration::from_millis(20))
        .expect("turned follow camera");
    let (target, target_focus) = expected_rear_view(turned);
    assert!(halfway.position().distance(target) > 0.01, "camera offset should lag the avatar turn");
    let (_, focus) = camera.current_position_and_focus(turned, start + Duration::from_millis(20));
    assert_eq!(focus, target_focus);

    let converged = camera
        .follow_with_sit_input(turned, None, SittingCameraInput::default(), [1280, 720], 512.0, 1.0, start + Duration::from_secs(1))
        .expect("converged follow camera");
    assert!(converged.position().distance(target) < 0.01);
    assert!(!camera.moving(turned, start + Duration::from_secs(1)));
}

#[test]
fn follow_camera_snaps_large_offset_changes() {
    let start = Instant::now();
    let pose = WorldPose { position: DVec3::ZERO, rotation: math::Quat::IDENTITY };
    let mut camera = FollowCamera::default();
    camera.follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 512.0, 1.0, start).expect("initial follow camera");
    camera.zoom(25.0, 512.0);
    let (_, focus) = expected_rear_view(pose);
    let rear_offset = Vec3::new(-4.0, 0.0, -0.25);
    let offset = rear_offset.normalize().as_dvec3() * (f64::from(rear_offset.length()) * 25.0);
    let target = focus + offset;

    let snapped = camera
        .follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 512.0, 1.0, start + Duration::from_millis(10))
        .expect("snapped follow camera");
    assert!(snapped.position().distance(target) < 1e-5);
    assert!(!camera.moving(pose, start + Duration::from_millis(10)));
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[test]
fn picked_focus_detaches_and_escape_returns_to_follow() {
    let mut mode = CameraMode::Follow;
    mode.detach_for_focus();
    assert_eq!(mode, CameraMode::Free);

    mode.return_to_follow();
    assert_eq!(mode, CameraMode::Follow);
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[test]
fn uncaptured_movement_returns_to_follow() {
    let mut mode = CameraMode::Free;
    let mut controls = HeldControls::default();

    assert_eq!(
        handle_keyboard_input(&mut controls, &mut mode, &Key::Character("w".into()), ElementState::Pressed, false, false, false),
        Some(true)
    );
    assert_eq!(mode, CameraMode::Follow);
    assert_eq!(controls.observation(false, math::Quat::IDENTITY).control_flags & 0x0000_0001, 0x0000_0001);
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[test]
fn egui_captured_movement_does_not_return_to_follow() {
    let mut mode = CameraMode::Free;
    let mut controls = HeldControls::default();

    assert_eq!(
        handle_keyboard_input(&mut controls, &mut mode, &Key::Character("w".into()), ElementState::Pressed, false, true, false),
        None
    );
    assert_eq!(mode, CameraMode::Free);
    assert_eq!(controls.observation(false, math::Quat::IDENTITY).control_flags, 0);
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[test]
fn flight_run_and_stop_keys_return_to_follow() {
    use winit::keyboard::NamedKey;

    for key in [Key::Character("f".into()), Key::Character("r".into()), Key::Named(NamedKey::Space)] {
        let mut mode = CameraMode::Free;
        let mut controls = HeldControls::default();

        let binding = KeyBinding::from_key(&key).expect("mapped camera-return key");
        assert_eq!(handle_keyboard_input(&mut controls, &mut mode, &key, ElementState::Pressed, false, false, false), Some(true));
        std::assert_matches!(binding, KeyBinding::ToggleFlying | KeyBinding::ToggleRun | KeyBinding::Stop);
        assert_eq!(mode, CameraMode::Follow);
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[test]
fn movement_key_release_does_not_change_camera_mode() {
    use crate::agent_movement::MovementKey;

    let mut mode = CameraMode::Free;
    let mut controls = HeldControls::default();
    controls.set_pressed(MovementKey::Forward, true);

    assert_eq!(
        handle_keyboard_input(&mut controls, &mut mode, &Key::Character("w".into()), ElementState::Released, false, false, false),
        Some(true)
    );
    assert_eq!(mode, CameraMode::Free);
}

#[test]
fn detaching_from_follow_preserves_the_live_view() {
    let pose = WorldPose { position: DVec3::new(1280.0, -256.0, 45.0), rotation: math::Quat::from_rotation_z(0.7) };
    let now = Instant::now();
    let follow = FollowCamera::default()
        .follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 512.0, 1.5, now)
        .expect("valid follow camera");
    let (_, focus) = expected_rear_view(pose);
    let free =
        OrbitCamera::from_follow_view(follow.position(), focus, 512.0, 1.5).scene_camera([1280, 720], now).expect("valid free camera");

    assert!(free.position().distance(follow.position()) < 1e-9);
    assert!(free.direction().distance(follow.direction()) < 1e-6);
    assert_eq!(free.draw_distance(), follow.draw_distance());
}

#[test]
fn sit_camera_overrides_follow_the_world_resolved_seat_pose() {
    let pose = WorldPose { position: DVec3::new(100.0, 200.0, 30.0), rotation: math::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2) };
    let eye = Vec3::new(2.0, 0.0, 1.0);
    let at = Vec3::new(0.0, 0.0, 1.0);
    let mut follow = FollowCamera::default();
    let now = Instant::now();
    let sit_camera = SitCameraOverride { eye_offset: eye, at_offset: at };
    let seat_id = Uuid::from_u128(1);
    follow
        .follow_with_sit_input(pose, Some((seat_id, Some(sit_camera), pose)), SittingCameraInput::default(), [1280, 720], 128.0, 1.0, now)
        .expect("entering sit camera");
    let camera = follow
        .follow_with_sit_input(
            pose,
            Some((seat_id, Some(sit_camera), pose)),
            SittingCameraInput::default(),
            [1280, 720],
            128.0,
            1.0,
            now + FOCUS_TIME + Duration::from_secs(1),
        )
        .expect("settled sit camera");

    assert!((camera.position() - (pose.position + DVec3::new(0.0, 2.0, 1.0))).length() < 1e-6);
    assert!(camera.direction().distance(Vec3::new(0.0, -1.0, 0.0)) < 1e-6);
}

#[test]
fn seated_camera_without_offsets_uses_follow_view() {
    let start = Instant::now();
    let pose = WorldPose { position: DVec3::new(40.0, 50.0, 60.0), rotation: Quat::from_rotation_z(0.4) };
    let seat_id = Uuid::from_u128(12);
    let mut camera = FollowCamera::default();
    camera
        .follow_with_sit_input(pose, Some((seat_id, None, pose)), SittingCameraInput::default(), [1280, 720], 128.0, 1.0, start)
        .expect("seated default camera transition");
    let seated = camera
        .follow_with_sit_input(
            pose,
            Some((seat_id, None, pose)),
            SittingCameraInput::default(),
            [1280, 720],
            128.0,
            1.0,
            start + FOCUS_TIME,
        )
        .expect("settled seated default camera");
    let (position, focus) = expected_rear_view(pose);
    assert!(seated.position().distance(position) < 1e-6);
    assert!(seated.direction().distance((focus - position).as_vec3().normalize()) < 1e-6);
}

#[test]
fn offset_camera_tracks_moving_rotated_seat() {
    let start = Instant::now();
    let first = WorldPose { position: DVec3::new(40.0, 50.0, 60.0), rotation: Quat::IDENTITY };
    let second = WorldPose { position: DVec3::new(44.0, 52.0, 61.0), rotation: Quat::from_rotation_z(0.8) };
    let seat_id = Uuid::from_u128(13);
    let camera_override = SitCameraOverride { eye_offset: Vec3::new(2.0, -1.0, 0.5), at_offset: Vec3::new(0.5, 0.0, 1.0) };
    let mut camera = FollowCamera::default();
    camera
        .follow_with_sit_input(
            first,
            Some((seat_id, Some(camera_override), first)),
            SittingCameraInput::default(),
            [1280, 720],
            128.0,
            1.0,
            start,
        )
        .expect("initial seat camera");
    let moving = camera
        .follow_with_sit_input(
            second,
            Some((seat_id, Some(camera_override), second)),
            SittingCameraInput::default(),
            [1280, 720],
            128.0,
            1.0,
            start + FOCUS_TIME + Duration::from_secs(1),
        )
        .expect("camera after seat movement");
    let expected_position = second.position + (second.rotation * camera_override.eye_offset).as_dvec3();
    let expected_focus = second.position + (second.rotation * camera_override.at_offset).as_dvec3();
    assert!(moving.position().distance(expected_position) < 1e-6);
    assert!(moving.direction().distance((expected_focus - expected_position).as_vec3().normalize()) < 1e-6);
}

#[test]
fn sit_camera_entry_animates_and_stand_uses_follow_smoothing() {
    let start = Instant::now();
    let pose = WorldPose { position: DVec3::new(100.0, 200.0, 30.0), rotation: math::Quat::IDENTITY };
    let sit_pose = WorldPose { position: pose.position + DVec3::new(2.0, 0.0, 1.0), ..pose };
    let sit_camera = SitCameraOverride { eye_offset: Vec3::new(2.0, 0.0, 1.0), at_offset: Vec3::new(0.0, 0.0, 1.0) };
    let seat_id = Uuid::from_u128(1);
    let transition_start = start + Duration::from_millis(1);
    let mut camera = FollowCamera::default();
    let standing =
        camera.follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 128.0, 1.0, start).expect("standing camera");
    let entering = camera
        .follow_with_sit_input(
            sit_pose,
            Some((seat_id, Some(sit_camera), sit_pose)),
            SittingCameraInput::default(),
            [1280, 720],
            128.0,
            1.0,
            transition_start,
        )
        .expect("camera while entering the seat");
    assert_eq!(entering.position(), standing.position());
    assert_eq!(entering.direction(), standing.direction());

    let seated_at = transition_start + FOCUS_TIME;
    let seated = camera
        .follow_with_sit_input(
            sit_pose,
            Some((seat_id, Some(sit_camera), sit_pose)),
            SittingCameraInput::default(),
            [1280, 720],
            128.0,
            1.0,
            seated_at,
        )
        .expect("seated camera");
    let leaving = camera
        .follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 128.0, 1.0, seated_at + Duration::from_millis(1))
        .expect("camera while leaving the seat");
    assert_ne!(leaving.position(), seated.position());
    assert!(camera.moving(pose, seated_at + Duration::from_millis(1)));

    let standing_again = camera
        .follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 128.0, 1.0, seated_at + Duration::from_millis(200))
        .expect("standing camera after follow smoothing");
    let expected = FollowCamera::default()
        .follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 128.0, 1.0, seated_at + Duration::from_millis(200))
        .expect("default follow camera");
    assert!(standing_again.position().distance(expected.position()) < 0.1);
    assert!(standing_again.direction().distance(expected.direction()) < 0.02);
    assert!(!camera.moving(pose, seated_at + Duration::from_millis(200)));
}

#[test]
fn switching_seats_with_same_camera_offsets_transitions() {
    let start = Instant::now();
    let first_pose = WorldPose { position: DVec3::new(100.0, 200.0, 30.0), rotation: math::Quat::IDENTITY };
    let second_pose = WorldPose { position: DVec3::new(180.0, 40.0, 22.0), rotation: math::Quat::IDENTITY };
    let first_seat = Uuid::from_u128(1);
    let second_seat = Uuid::from_u128(2);
    let sit_camera = SitCameraOverride { eye_offset: Vec3::new(2.0, 0.0, 1.0), at_offset: Vec3::new(0.0, 0.0, 1.0) };
    let mut camera = FollowCamera::default();
    camera
        .follow_with_sit_input(
            first_pose,
            Some((first_seat, Some(sit_camera), first_pose)),
            SittingCameraInput::default(),
            [1280, 720],
            128.0,
            1.0,
            start,
        )
        .expect("first seat camera");
    let seated = camera
        .follow_with_sit_input(
            first_pose,
            Some((first_seat, Some(sit_camera), first_pose)),
            SittingCameraInput::default(),
            [1280, 720],
            128.0,
            1.0,
            start + FOCUS_TIME + Duration::from_secs(1),
        )
        .expect("settled first seat camera");
    let switch_at = start + FOCUS_TIME + Duration::from_secs(1) + Duration::from_millis(1);
    let switching = camera
        .follow_with_sit_input(
            second_pose,
            Some((second_seat, Some(sit_camera), second_pose)),
            SittingCameraInput::default(),
            [1280, 720],
            128.0,
            1.0,
            switch_at,
        )
        .expect("camera at seat switch");
    assert_eq!(switching.position(), seated.position());
    assert_eq!(switching.direction(), seated.direction());

    let settled = camera
        .follow_with_sit_input(
            second_pose,
            Some((second_seat, Some(sit_camera), second_pose)),
            SittingCameraInput::default(),
            [1280, 720],
            128.0,
            1.0,
            switch_at + FOCUS_TIME,
        )
        .expect("settled second seat camera");
    assert!((settled.position() - (second_pose.position + DVec3::new(2.0, 0.0, 1.0))).length() < 1e-6);
    assert!(settled.direction().distance(Vec3::new(-1.0, 0.0, 0.0)) < 1e-6);
}

#[test]
fn returning_from_free_camera_preserves_live_view_when_sitting() {
    let start = Instant::now();
    let pose = WorldPose { position: DVec3::new(100.0, 200.0, 30.0), rotation: math::Quat::IDENTITY };
    let sit_pose = WorldPose { position: pose.position + DVec3::new(2.0, 0.0, 1.0), ..pose };
    let sit_camera = SitCameraOverride { eye_offset: Vec3::new(2.0, 0.0, 1.0), at_offset: Vec3::new(0.0, 0.0, 1.0) };
    let seat_id = Uuid::from_u128(1);
    let mut camera = FollowCamera::default();
    camera.follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 128.0, 1.0, start).expect("standing camera");

    let free_position = pose.position + DVec3::new(-8.0, 2.0, 4.0);
    let free_focus = pose.position + DVec3::new(1.0, 1.0, 1.0);
    let returned_at = start + Duration::from_millis(1);
    camera.transition_from(free_position, free_focus, returned_at);
    let returned = camera
        .follow_with_sit_input(
            pose,
            Some((seat_id, Some(sit_camera), sit_pose)),
            SittingCameraInput::default(),
            [1280, 720],
            128.0,
            1.0,
            returned_at + Duration::from_millis(1),
        )
        .expect("follow camera after returning from free mode");

    assert_eq!(returned.position(), free_position);
    assert!(returned.direction().distance((free_focus - free_position).as_vec3().normalize()) < 1e-6);
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[test]
fn ground_sitting_keys_orbit_the_camera() {
    let start = Instant::now();
    let pose = WorldPose { position: DVec3::new(20.0, 30.0, 40.0), rotation: Quat::IDENTITY };
    let mut controls = HeldControls::default();
    for key in ["a", "w", "e"] {
        controls.keyboard_event(&Key::Character(key.into()), ElementState::Pressed, false, false, true).expect("sitting key");
    }
    let actions = controls.sitting_actions(
        false,
        false,
        false,
        false,
        crate::agent_movement::ScriptControlGrabs::default(),
        start + Duration::from_secs(1),
    );
    assert_eq!(actions.control_flags, 0);
    assert_eq!(actions.camera.around, -1.0);
    assert_eq!(actions.camera.over, 1.0);
    assert_eq!(actions.camera.distance, 1.0);

    let mut camera = FollowCamera::default();
    let before = camera
        .follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 128.0, 1.0, start)
        .expect("ground sit camera before input");
    let after = camera
        .follow_with_sit_input(pose, None, actions.camera, [1280, 720], 128.0, 1.0, start + Duration::from_secs(1))
        .expect("ground sit camera after input");
    assert!(after.position().distance(before.position()) > 1.0);
    assert!(after.direction().distance(before.direction()) > 0.1);
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[test]
fn object_sitting_without_offsets_orbits_the_camera() {
    let start = Instant::now();
    let pose = WorldPose { position: DVec3::new(20.0, 30.0, 40.0), rotation: Quat::IDENTITY };
    let seat_id = Uuid::from_u128(10);
    let mut controls = HeldControls::default();
    for key in ["d", "s", "c"] {
        controls.keyboard_event(&Key::Character(key.into()), ElementState::Pressed, false, false, true).expect("sitting key");
    }
    let actions = controls.sitting_actions(
        false,
        false,
        false,
        false,
        crate::agent_movement::ScriptControlGrabs::default(),
        start + Duration::from_secs(1),
    );
    assert_eq!(actions.control_flags, 0);
    assert_eq!(actions.camera.around, 1.0);
    assert_eq!(actions.camera.over, -1.0);
    assert_eq!(actions.camera.distance, -1.0);

    let mut camera = FollowCamera::default();
    camera
        .follow_with_sit_input(pose, Some((seat_id, None, pose)), SittingCameraInput::default(), [1280, 720], 128.0, 1.0, start)
        .expect("object sit camera before input");
    let before = camera
        .follow_with_sit_input(
            pose,
            Some((seat_id, None, pose)),
            SittingCameraInput::default(),
            [1280, 720],
            128.0,
            1.0,
            start + FOCUS_TIME,
        )
        .expect("object sit camera after entry transition");
    let after = camera
        .follow_with_sit_input(
            pose,
            Some((seat_id, None, pose)),
            actions.camera,
            [1280, 720],
            128.0,
            1.0,
            start + FOCUS_TIME + Duration::from_secs(1),
        )
        .expect("object sit camera after input");
    assert!(after.position().distance(before.position()) > 1.0);
    assert!(after.direction().distance(before.direction()) > 0.1);
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[test]
fn object_orbit_resets_on_forward() {
    let now = Instant::now();
    let rotation = Quat::from_rotation_z(0.7);
    let pose = WorldPose { position: DVec3::new(20.0, 30.0, 40.0), rotation };
    let seat_id = Uuid::from_u128(12);
    let mut camera = FollowCamera::default();
    camera
        .follow_with_sit_input(pose, Some((seat_id, None, pose)), SittingCameraInput::default(), [1280, 720], 128.0, 1.0, now)
        .expect("object sit camera");
    camera
        .follow_with_sit_input(
            pose,
            Some((seat_id, None, pose)),
            SittingCameraInput { around: 1.0, ..Default::default() },
            [1280, 720],
            128.0,
            1.0,
            now + Duration::from_secs(1),
        )
        .expect("object sit orbit");
    camera
        .follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 128.0, 1.0, now + Duration::from_secs(2))
        .expect("standing camera");

    assert_forward_camera_matches_facing(&mut camera, pose, now + Duration::from_secs(3));
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[test]
fn ground_orbit_resets_on_forward() {
    let now = Instant::now();
    let rotation = Quat::from_rotation_z(-0.4);
    let pose = WorldPose { position: DVec3::new(50.0, -10.0, 25.0), rotation };
    let mut camera = FollowCamera::default();
    camera.follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 128.0, 1.0, now).expect("ground sit camera");
    camera
        .follow_with_sit_input(
            pose,
            None,
            SittingCameraInput { around: 1.0, ..Default::default() },
            [1280, 720],
            128.0,
            1.0,
            now + Duration::from_secs(1),
        )
        .expect("ground sit orbit");
    camera
        .follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 128.0, 1.0, now + Duration::from_secs(2))
        .expect("standing camera");

    assert_forward_camera_matches_facing(&mut camera, pose, now + Duration::from_secs(3));
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn assert_forward_camera_matches_facing(camera: &mut FollowCamera, pose: WorldPose, now: Instant) {
    let mut controls = HeldControls::default();
    let mut mode = CameraMode::Follow;
    let forward_key = Key::Character("w".into());
    assert_eq!(handle_keyboard_input(&mut controls, &mut mode, &forward_key, ElementState::Pressed, false, false, true), Some(true));
    assert!(should_reset_sitting_orbit(&forward_key, ElementState::Pressed, false, false, false, false, false, false));
    let mut body_rotation = crate::agent_movement::BodyRotation::default();
    body_rotation.update(Some((1, pose.rotation)), 0.0, now);
    assert!(camera.reset_after_sitting(pose, now), "the first walking key resets the seated camera");
    body_rotation.reset_axes(pose.rotation);
    let movement = controls.observation(false, body_rotation.current());
    let standing_pose = WorldPose { rotation: body_rotation.current(), ..pose };
    let view = camera
        .follow_with_sit_input(standing_pose, None, SittingCameraInput::default(), [1280, 720], 128.0, 1.0, now + FOCUS_TIME)
        .expect("camera after forward press");
    let facing = body_rotation.current() * Vec3::X;
    assert_eq!(movement.control_flags, 0x0000_0401);
    assert!((movement.body_rotation * Vec3::X).distance(facing) < 1e-6, "movement follows the standing body rotation");
    let camera_facing = Vec3::new(view.direction().x, view.direction().y, 0.0).normalize();
    assert!(camera_facing.distance(facing) < 0.01, "the camera follows the standing body rotation");
    let (expected_position, _) = expected_rear_view(standing_pose);
    assert!(view.position().distance(expected_position) < 0.1, "the camera returns to the rear-view position");
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[test]
fn only_viewer_movement_actions_reset_the_sitting_orbit() {
    let eligible = |key: &str, shift: bool| {
        should_reset_sitting_orbit(&Key::Character(key.into()), ElementState::Pressed, false, false, false, false, false, shift)
    };

    assert!(eligible("w", false));
    assert!(eligible("s", false));
    assert!(!eligible("a", false));
    assert!(!eligible("d", false));
    assert!(eligible("a", true));
    assert!(eligible("d", true));
    assert!(!eligible("e", false));
    assert!(!should_reset_sitting_orbit(&Key::Character("w".into()), ElementState::Pressed, false, false, true, false, false, false));
    assert!(!should_reset_sitting_orbit(&Key::Character("w".into()), ElementState::Pressed, false, false, false, true, false, false));
    assert!(!should_reset_sitting_orbit(&Key::Character("w".into()), ElementState::Pressed, true, false, false, false, false, false));
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[test]
fn object_sit_camera_forwards_sitting_keys() {
    let now = Instant::now();
    let pose = WorldPose { position: DVec3::new(20.0, 30.0, 40.0), rotation: Quat::IDENTITY };
    let seat_id = Uuid::from_u128(11);
    let sit_camera = SitCameraOverride { eye_offset: Vec3::new(2.0, 0.0, 1.0), at_offset: Vec3::new(0.0, 0.0, 1.0) };
    let mut controls = HeldControls::default();
    for key in ["a", "w", "e"] {
        controls.keyboard_event(&Key::Character(key.into()), ElementState::Pressed, false, false, true).expect("sitting key");
    }
    let actions = controls.sitting_actions(
        false,
        false,
        false,
        true,
        crate::agent_movement::ScriptControlGrabs::default(),
        now + Duration::from_secs(1),
    );
    assert_eq!(actions.camera, SittingCameraInput::default());
    assert_eq!(actions.control_flags, 0x0000_1511);

    let mut camera = FollowCamera::default();
    camera
        .follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 128.0, 1.0, now)
        .expect("standing camera before sit");
    camera
        .follow_with_sit_input(
            pose,
            Some((seat_id, Some(sit_camera), pose)),
            SittingCameraInput::default(),
            [1280, 720],
            128.0,
            1.0,
            now + Duration::from_millis(1),
        )
        .expect("sit camera transition");
    let camera = camera
        .follow_with_sit_input(
            pose,
            Some((seat_id, Some(sit_camera), pose)),
            actions.camera,
            [1280, 720],
            128.0,
            1.0,
            now + FOCUS_TIME + Duration::from_secs(1),
        )
        .expect("sit camera with control input");
    assert_eq!(camera.position(), pose.position + DVec3::new(2.0, 0.0, 1.0));
    assert_eq!(camera.direction(), Vec3::new(-1.0, 0.0, 0.0));
}

#[test]
fn sitting_orbit_rate_matches_viewer_camera_rate() {
    let start = Instant::now();
    let pose = WorldPose { position: DVec3::ZERO, rotation: Quat::IDENTITY };
    let input = SittingCameraInput { around: 1.0, ..SittingCameraInput::default() };
    let mut camera = FollowCamera::default();
    camera.follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 128.0, 1.0, start).expect("initial camera");
    let orbit = camera
        .follow_with_sit_input(pose, None, input, [1280, 720], 128.0, 1.0, start + Duration::from_secs(1))
        .expect("camera after orbit");
    assert!((orbit.position() - DVec3::new(1.0, -4.0, 0.75)).length() < 0.001);
}

#[test]
fn sitting_under_orbit_clamps_to_viewer_limit() {
    let start = Instant::now();
    let pose = WorldPose { position: DVec3::ZERO, rotation: Quat::IDENTITY };
    let input = SittingCameraInput { over: -1.0, ..SittingCameraInput::default() };
    let mut camera = FollowCamera::default();
    camera.follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 128.0, 1.0, start).expect("initial camera");
    let result = camera
        .follow_with_sit_input(pose, None, input, [1280, 720], 128.0, 1.0, start + Duration::from_secs(1))
        .expect("camera after under orbit");
    let focus = pose.position + DVec3::new(1.0, 0.0, 1.0);
    let offset = (result.position() - focus).normalize();
    assert!((offset.dot(DVec3::Z) - (179.0 * PI / 180.0).cos()).abs() < 1e-6);
}

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
    let halfway_view = camera.scene_camera([1280, 720], halfway).unwrap();
    assert_eq!(halfway_view.position(), before.position());
    assert!(halfway_view.direction().distance((point * 0.5 - before.position()).normalize().as_vec3()) < 1e-6);
    let end = camera.scene_camera([1280, 720], now + FOCUS_TIME).unwrap();
    assert!(!camera.moving(now + FOCUS_TIME), "redraws stop at the end of the focus transition");
    assert!(end.direction().distance((point - end.position()).normalize().as_vec3()) < 1e-6);
    assert_eq!(camera.current_position_and_focus(now + FOCUS_TIME * 2).1, point);

    let visible = camera.scene_camera([1280, 720], halfway).unwrap();
    camera.focus(DVec3::new(20.0, -5.0, 3.0), halfway);
    let interrupted = camera.scene_camera([1280, 720], halfway).unwrap();
    assert_eq!(visible.position(), interrupted.position());
    assert!(visible.direction().distance(interrupted.direction()) < 1e-6);
    camera.drag([40.0, 0.0], 1000.0, true, false);
    assert!(camera.scene_camera([1280, 720], halfway + FOCUS_TIME / 2).unwrap().position().distance(before.position()) > 0.1);
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
    assert_eq!(camera.current_position_and_focus(now + FOCUS_TIME).1, home + DVec3::new(10.0, 2.0, 3.0));
}

#[test]
fn orbit_and_pan() {
    let mut camera = OrbitCamera::new(DVec3::ZERO);
    camera.drag([250.0, 0.0], 1000.0, true, false);
    let orbit = camera.scene_camera([1280, 720], Instant::now()).unwrap();
    assert!(orbit.position().distance(DVec3::new(0.0, 4.0, 2.0)) < 1e-10);
    assert!(orbit.direction().distance(DVec3::new(0.0, -4.0, -2.0).normalize().as_vec3()) < 1e-10);
    camera.drag([100.0, 0.0], 1000.0, true, true);
    let panned = camera.scene_camera([1280, 720], Instant::now()).unwrap();
    assert!(panned.position().distance(DVec3::new(20.0f64.sqrt() * 0.3, 4.0, 2.0)) < 1e-10);
    assert!(panned.direction().distance(orbit.direction()) < 1e-10);
    camera.drag([0.0, 1000.0], 1000.0, true, false);
    let clamped = camera.scene_camera([1280, 720], Instant::now()).unwrap();
    assert!(((-clamped.direction().z).acos() - std::f32::consts::PI / 180.0).abs() < 1e-6);
}

#[test]
fn zoom_limits() {
    let now = Instant::now();
    let mut camera = OrbitCamera::new(DVec3::ZERO);
    camera.zoom(0.0001);
    let distance = |camera: &OrbitCamera| {
        let (position, focus) = camera.current_position_and_focus(now + FOCUS_TIME);
        position.distance(focus)
    };
    assert!((distance(&camera) - 0.5).abs() < 1e-10);
    camera.focus(DVec3::X, now);
    camera.zoom(0.0001);
    assert!((distance(&camera) - 0.02).abs() < 1e-10);
    camera.zoom(1e6);
    assert!((distance(&camera) - 240.0).abs() < 1e-10);
    let mut limited = OrbitCamera::from_follow_view(DVec3::ZERO, DVec3::X, 32.0, 2.0);
    limited.zoom(1e6);
    let (position, focus) = limited.current_position_and_focus(now);
    assert!((position.distance(focus) - 31.0).abs() < 1e-10);
}
