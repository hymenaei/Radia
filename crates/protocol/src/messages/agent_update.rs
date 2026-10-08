use base::UUID;

use crate::{Quaternion, Vector3};

/// The viewer's pose, camera, and control state sent to the simulator.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentUpdate {
    pub agent_data: AgentData,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentData {
    pub agent_id: UUID,
    pub session_id: UUID,
    pub body_rotation: Quaternion,
    pub head_rotation: Quaternion,
    pub state: u8,
    /// Region-local camera center.
    pub camera_center: Vector3,
    pub camera_at_axis: Vector3,
    pub camera_left_axis: Vector3,
    pub camera_up_axis: Vector3,
    /// Far clipping distance in meters.
    pub far: f32,
    pub control_flags: u32,
    pub flags: u8,
}
