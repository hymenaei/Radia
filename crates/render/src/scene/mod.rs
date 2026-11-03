mod asset_demand;
mod budget;
mod camera;
mod dependencies;
mod environment;
mod environment_textures;
mod mesh;
mod mesh_cache;
mod model;
pub(crate) mod motion;
mod planning;
mod prim;
mod reconcile;
mod region_environment;
mod sculpt;
mod surface;
pub(crate) mod texture_animation;

use uuid::Uuid;
use world::WorldPose;

pub use asset_demand::{AssetChange, AssetRequest, AssetTicket};
pub use budget::{Budget, MEMORY_BYTES};
pub use camera::{Camera, CameraError};
pub use environment::{
    EnvironmentChoice, EnvironmentPreset, EnvironmentProperty, EnvironmentPush, EnvironmentPushError, ExperienceProperty,
    ExperienceSetting, LocalChoice, LocalEnvironment, ViewerEnvironment,
};
pub(crate) use environment::{PropertyChange, PropertyKind, PropertyValue};
pub use mesh::{Detail, DrawnMeshKey, MeshBuildError, MeshId, SharedMesh};
pub use mesh_cache::SculptMeshKey;
pub use mesh_cache::{MeshKey, PreparedMesh};
pub use model::{
    ApplyError, Batch, Geometry, HiddenReason, Object, Patch, Placement, RecoveryReason, RegionData, SceneView, Status, ViewVersion,
};
pub use motion::MotionCircuit;
pub use planning::{MeshCompletion, MeshJob};
pub use reconcile::{
    MAX_BATCH_CHANGES, MAX_OBJECTS, MAX_REGIONS, MAX_RETAINED_BYTES, ReadFailure, ReadJob, ReadQuery, ReadReply, ReconcileStep, Reconciler,
};
pub use region_environment::RegionEnvironment;
pub use sculpt::tessellate_sculpt;
pub use surface::{Surface, SurfaceKind};
pub use utils::ExperienceId;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgentPose {
    agent_id: Uuid,
    pose: WorldPose,
}

impl AgentPose {
    pub fn new(agent_id: Uuid, pose: WorldPose) -> Self {
        Self { agent_id, pose }
    }

    pub(crate) fn agent_id(self) -> Uuid {
        self.agent_id
    }

    pub(crate) fn pose(self) -> WorldPose {
        self.pose
    }
}
