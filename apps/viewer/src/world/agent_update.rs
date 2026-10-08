//! Agent update message construction helpers.

use base::UUID;
use protocol::{
    Quaternion, Vector3,
    messages::{AgentUpdate, agent_update::AgentData},
};

pub fn idle(agent_id: UUID, session_id: UUID) -> AgentUpdate {
    AgentUpdate {
        agent_data: AgentData {
            agent_id,
            session_id,
            body_rotation: Quaternion::default(),
            head_rotation: Quaternion::default(),
            state: 0,
            camera_center: Vector3 { x: 128.0, y: 128.0, z: 30.0 },
            camera_at_axis: Vector3 { x: 1.0, y: 0.0, z: 0.0 },
            camera_left_axis: Vector3 { x: 0.0, y: 1.0, z: 0.0 },
            camera_up_axis: Vector3 { x: 0.0, y: 0.0, z: 1.0 },
            far: 128.0,
            control_flags: 0,
            flags: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use protocol::udp::Packet;

    use super::*;

    #[test]
    fn default_update_matches_fixture() {
        let agent_id = UUID::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("fixture agent UUID");
        let session_id = UUID::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("fixture session UUID");
        let packet = Packet::new(3, idle(agent_id, session_id).into()).with_reliable().encode().expect("encode default update");
        let expected =
            test_support::parse_hex_fixture(test_support::udp::vectors::AGENT_UPDATE_INITIAL).expect("valid AgentUpdate fixture");
        assert_eq!(packet, expected);
    }
}
