use base::UUID;

use math::{Quat, Vec3};

/// The viewer's pose, camera, and control state sent to the simulator.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentUpdate {
    pub agent_data: AgentData,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentData {
    pub agent_id: UUID,
    pub session_id: UUID,
    pub body_rotation: Quat,
    pub head_rotation: Quat,
    pub state: u8,
    pub camera_center: Vec3,
    pub camera_at_axis: Vec3,
    pub camera_left_axis: Vec3,
    pub camera_up_axis: Vec3,
    pub far: f32,
    pub control_flags: u32,
    pub flags: u8,
}
