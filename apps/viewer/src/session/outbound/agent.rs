use math::{DVec3, Quat, Vec3};
use protocol::messages::{AgentMovementComplete, AgentUpdate, SetAlwaysRun, agent::agent_update::AgentData};
use render::scene::Camera;
use uuid::Uuid;

use crate::{DEFAULT_DRAW_DISTANCE, agent_movement::MovementObservation};

pub(crate) fn movement_camera(
    agent_id: Uuid,
    session_id: Uuid,
    camera: Camera,
    origin: DVec3,
    movement: MovementObservation,
) -> Option<AgentUpdate> {
    update(
        agent_id,
        session_id,
        (camera.position() - origin).as_vec3(),
        camera.direction(),
        camera.draw_distance() as f32,
        movement.body_rotation,
        movement.control_flags,
    )
}

pub(crate) fn always_run(agent_id: Uuid, session_id: Uuid, always_run: bool) -> SetAlwaysRun {
    SetAlwaysRun { agent_id, session_id, always_run }
}

pub(crate) fn arrival(agent_id: Uuid, session_id: Uuid, movement: &AgentMovementComplete) -> Option<AgentUpdate> {
    if movement.agent_data.agent_id != agent_id || movement.agent_data.session_id != session_id {
        return None;
    }
    update(agent_id, session_id, movement.data.position, movement.data.look_at, DEFAULT_DRAW_DISTANCE, Quat::IDENTITY, 0)
}

fn update(
    agent_id: Uuid,
    session_id: Uuid,
    position: Vec3,
    direction: Vec3,
    far: f32,
    body_rotation: Quat,
    control_flags: u32,
) -> Option<AgentUpdate> {
    if !position.is_finite() {
        return None;
    }
    let direction = direction.try_normalize()?;
    let left = Vec3::Z.cross(direction).try_normalize().unwrap_or(Vec3::Y);
    Some(AgentUpdate {
        agent_data: AgentData {
            agent_id,
            session_id,
            body_rotation,
            head_rotation: body_rotation,
            state: 0,
            camera_center: position,
            camera_at_axis: direction,
            camera_left_axis: left,
            camera_up_axis: direction.cross(left),
            far,
            control_flags,
            flags: 0,
        },
    })
}

#[cfg(test)]
mod tests {
    use protocol::lludp::{Message, Packet};

    use super::*;
    fn idle_movement() -> MovementObservation {
        MovementObservation { control_flags: 0, body_rotation: Quat::IDENTITY, always_run: false, stop_sequence: 0 }
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn camera_distance_reaches_simulator() {
        use std::time::Instant;

        let view = crate::camera::OrbitCamera::new(DVec3::ZERO).scene_camera([1280, 720], Instant::now()).unwrap();
        assert_eq!(
            movement_camera(Uuid::nil(), Uuid::nil(), view, DVec3::ZERO, idle_movement()).unwrap().agent_data.far,
            view.draw_distance() as f32,
            "simulator interest must match the actual default scene draw distance"
        );
    }

    #[test]
    fn arrival_uses_observed_pose() {
        let bytes = test_support::parse_hex_fixture(test_support::lludp::vectors::AGENT_MOVEMENT_COMPLETE).unwrap();
        let packet = Packet::decode(&bytes).unwrap();
        let Message::AgentMovementComplete(movement) = packet.message() else { panic!("movement completion") };
        let (agent, session) = (movement.agent_data.agent_id, movement.agent_data.session_id);
        let result = arrival(agent, session, movement).unwrap().agent_data;
        assert_eq!(result.camera_center, movement.data.position);
        assert_eq!(result.camera_at_axis, movement.data.look_at.normalize());
        assert_eq!(result.far, 512.0);
        assert!(arrival(Uuid::nil(), session, movement).is_none());
        assert!(arrival(agent, Uuid::nil(), movement).is_none());
        let mut invalid = movement.clone();
        invalid.data.position = Vec3::NAN;
        assert!(arrival(agent, session, &invalid).is_none());
        invalid.data.position = Vec3::ZERO;
        invalid.data.look_at = Vec3::ZERO;
        assert!(arrival(agent, session, &invalid).is_none());
    }

    #[test]
    fn camera_uses_region_coordinates() {
        let origin = DVec3::new(1e12, 2e12, 0.0);
        let view =
            Camera::new(origin + DVec3::new(200.25, -6.5, 80.0), -Vec3::X, 1.0, [720, 720]).unwrap().with_quality(64.0, 1.0).unwrap();
        let first = movement_camera(Uuid::nil(), Uuid::nil(), view, origin, idle_movement()).unwrap().agent_data;
        assert_eq!(first.camera_center, Vec3::new(200.25, -6.5, 80.0));
        assert_eq!(first.camera_at_axis, -Vec3::X);
        assert_eq!(first.camera_left_axis, -Vec3::Y);
        assert_eq!(first.camera_up_axis, Vec3::Z);
        assert_eq!(first.far, 64.0);
        let extended = view.with_quality(1024.0, 1.0).unwrap();
        assert_eq!(movement_camera(Uuid::nil(), Uuid::nil(), extended, origin, idle_movement()).unwrap().agent_data.far, 1024.0);
        let crossed = movement_camera(Uuid::nil(), Uuid::nil(), view, origin + DVec3::X * 256.0, idle_movement()).unwrap().agent_data;
        assert_eq!(crossed.camera_center, Vec3::new(-55.75, -6.5, 80.0));
        for direction in [Vec3::Z, -Vec3::Z] {
            let view = Camera::new(origin, direction, 1.0, [720, 720]).unwrap();
            let data = movement_camera(Uuid::nil(), Uuid::nil(), view, origin, idle_movement()).unwrap().agent_data;
            assert_eq!(data.camera_at_axis.dot(data.camera_left_axis), 0.0);
            assert_eq!(data.camera_at_axis.cross(data.camera_left_axis), data.camera_up_axis);
            assert_eq!(data.camera_up_axis.length(), 1.0);
        }
        assert!(movement_camera(Uuid::nil(), Uuid::nil(), view, DVec3::NAN, idle_movement()).is_none());
    }

    #[test]
    fn default_update_matches_fixture() {
        let agent_id = Uuid::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("fixture agent UUID");
        let session_id = Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("fixture session UUID");
        let view = Camera::new(DVec3::new(128.0, 128.0, 30.0), Vec3::X, 1.0, [720, 720]).unwrap().with_quality(128.0, 1.0).unwrap();
        let update = movement_camera(agent_id, session_id, view, DVec3::ZERO, idle_movement()).unwrap();
        let packet = Packet::new(3, update.into()).with_reliable().encode().expect("encode default update");
        let expected =
            test_support::parse_hex_fixture(test_support::lludp::vectors::AGENT_UPDATE_INITIAL).expect("valid AgentUpdate fixture");
        assert_eq!(packet, expected);
    }
}
