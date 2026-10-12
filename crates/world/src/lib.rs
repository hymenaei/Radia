//! Renderer-independent state for one simulator session.
//!
//! The world owns entity identity, region lifetimes, and source-ordered updates. It does not
//! depend on the network or rendering crates. A caller supplies explicit limits when creating it.

mod model;
mod state;
mod update;

pub use model::{
    Appearance, AttachmentPoint, CoordinateFrame, Entity, Identity, Kind, LinearMotion, LocalAddress, LocalTransform, MotionPatch, Parent,
    ParentUpdate, Pose, PoseError, PoseState, Properties, PropertiesPatch, Region, RegionHandle, RegionInfo, Shape, SitTarget, Transform,
    UnresolvedReason, WorldPose, WorldTransform, WorldTransformState,
};
pub use state::{Object, World};
pub use update::{
    AgentHandoff, Applied, ApplyError, ApplyOutcome, DeferredReason, FieldGroup, FieldRejection, FieldRejectionReason, ObjectObservation,
    ObjectPatch, OrderEvidence, Recovery, Rejections, UpdateAuthority, UpdateSource, WorldLimits, WorldRevision,
};
