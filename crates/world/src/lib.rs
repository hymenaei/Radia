//! Renderer-independent state for one simulator session.
//!
//! The world owns entity identity, region lifetimes, and source-ordered updates. It does not
//! depend on the network or rendering crates. A caller supplies explicit limits when creating it.
//!
//! Each live entity has private spatial state, content, and ordering components with one shared
//! lifetime. World plans dependent changes before committing them and maintains identity,
//! relationship, and query indexes together. Consumers cannot mutate the ECS directly.
//! [`WorldChange`] records describe committed domain changes rather than individual component
//! writes. Bounded pages and read barriers support consumers that retain their own derived state.
//!
//! Simulator state changes through typed methods such as [`World::observe_object`],
//! [`World::patch_object`], and [`World::retire_region`]. Callers supply decoded values and source
//! evidence; World owns acceptance, dependent changes, revisions, and recovery intents. An
//! [`Applied`] result can include both accepted changes and rejected field groups. An [`ApplyError`]
//! rejects the operation without changing World. An unchanged result can still advance internal
//! ordering evidence, so consumers should use the returned revision to detect observable changes.
//!
//! Prediction and local simulation are separate responsibilities. Their state needs an explicit
//! ownership and reconciliation contract before it can affect authoritative simulator state.
//!
//! Use [`World::object`] for a point read and [`World::objects`] with [`Objects::iter`] for a scan
//! that borrows component storage once. [`Shape::Prim`] carries lossless [`PrimShape`] parameters;
//! geometry consumers must constrain observed parameters before generating meshes.

mod appearance;
mod avatar;
mod identity;
mod media;
mod object;
mod shape;
mod transform;
mod update;
mod world;

pub use appearance::{
    Appearance, FaceAppearance, GLTF_TEXTURE_INFO_COUNT, GltfAlphaMode, GltfMaterialOverride, GltfMaterialOverrides,
    GltfMaterialOverridesError, GltfTextureTransform, MAX_RENDER_MATERIAL_BINDINGS, MAX_SURFACE_FACES, RenderMaterials,
    RenderMaterialsError, SurfaceAppearance, SurfaceAppearanceError,
};
pub use avatar::{
    AvatarAppearance, AvatarAppearanceError, AvatarAppearanceUpdate, AvatarBakeSlot, AvatarTextureSlot, AvatarTextureSlots,
    AvatarVisualParamIndex, MAX_AVATAR_TEXTURE_SLOTS, MAX_AVATAR_VISUAL_PARAMS,
};
pub use identity::{AssetId, Entity, LocalAddress, ObjectBindingTarget, Region, RegionHandle, RegionInfo};
pub use media::{
    MAX_MEDIA_DIMENSION, MAX_MEDIA_URL_BYTES, MAX_MEDIA_WHITELIST_BYTES, MAX_MEDIA_WHITELIST_ENTRIES, MediaControls, MediaEntry,
    ObjectMedia, ObjectMediaError, ObjectMediaUpdate,
};
pub use object::{Kind, Parent, ParentUpdate, Properties, PropertiesPatch, PropertiesProvenance, PropertyProvenance};
pub use shape::{PrimShape, SculptFlags, SculptType, Shape};
pub use transform::{
    AttachmentPoint, CoordinateFrame, LinearMotion, LocalTransform, MotionPatch, Pose, PoseError, PoseState, SitTarget, Transform,
    UnresolvedReason, WorldPose, WorldTransform, WorldTransformState,
};
pub use update::{
    AgentHandoff, Applied, ApplyError, ApplyOutcome, DeferredReason, FieldGroup, FieldRejection, FieldRejectionReason, FieldUpdate,
    ObjectObservation, ObjectPatch, OrderEvidence, Recovery, RegionApplied, RegionOutcome, Rejections, UpdateAuthority, UpdateSource,
    WorldLimits, WorldRevision,
};
pub use world::{
    MAX_OBJECT_DETAIL_BATCH_BYTES, MAX_OBJECT_DETAIL_BATCH_ITEMS, MAX_OBJECT_DETAIL_BYTES, MAX_OBJECT_PAGE_ITEMS,
    MAX_QUERY_STATE_MAINTENANCE_WORK, MAX_RECENT_OBJECT_BYTES, MAX_RECENT_OBJECTS, MAX_REGION_PAGE_BYTES, MAX_REGION_PAGE_ITEMS,
    MAX_WORLD_CHANGE_PAGE_ITEMS, MAX_WORLD_QUERY_RESULTS, MAX_WORLD_QUERY_WORK, Object, ObjectDetail, ObjectDetailBatch,
    ObjectDetailResult, ObjectListItem, ObjectListItemRead, ObjectListPage, ObjectPage, ObjectPageCursor, ObjectSummary, ObjectSummaryRead,
    Objects, QueryCompleteness, QueryIncompleteReason, QueryScope, QueryStateMaintenance, QueryUnresolved, RECENT_OBJECT_TTL, RecentObject,
    RegionPage, RegionPageCursor, RegionSummary, RegionSummaryRead, ValueFilter, World, WorldChange, WorldChangeClasses, WorldChangeCursor,
    WorldChangePage, WorldChangeRecord, WorldEntityRemovalReason, WorldQuery, WorldQueryBudget, WorldQueryBuildError, WorldQueryCursor,
    WorldQueryItem, WorldQueryPage, WorldReadBarrier, WorldReadError,
};
