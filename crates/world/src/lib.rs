//! Renderer-independent state for one simulator session.
//!
//! The world owns entity identity, region lifetimes, and source-ordered updates. It does not
//! depend on the network or rendering crates. A caller supplies explicit limits when creating it.

mod avatar;
mod media;
mod model;
mod state;
mod update;

pub use avatar::{
    AvatarAppearance, AvatarAppearanceError, AvatarAppearanceUpdate, AvatarBakeSlot, AvatarTextureSlot, AvatarTextureSlots,
    AvatarVisualParamIndex, MAX_AVATAR_TEXTURE_SLOTS, MAX_AVATAR_VISUAL_PARAMS,
};
pub use media::{
    MAX_MEDIA_DIMENSION, MAX_MEDIA_URL_BYTES, MAX_MEDIA_WHITELIST_BYTES, MAX_MEDIA_WHITELIST_ENTRIES, MediaControls, MediaEntry,
    ObjectMedia, ObjectMediaError, ObjectMediaUpdate,
};
pub use model::{
    Appearance, AssetId, AttachmentPoint, CoordinateFrame, Entity, FaceAppearance, GLTF_TEXTURE_INFO_COUNT, GltfAlphaMode,
    GltfMaterialOverride, GltfMaterialOverrides, GltfMaterialOverridesError, GltfTextureTransform, Identity, Kind, LinearMotion,
    LocalAddress, LocalTransform, MAX_RENDER_MATERIAL_BINDINGS, MAX_SURFACE_FACES, MotionPatch, Parent, ParentUpdate, Pose, PoseError,
    PoseState, Properties, PropertiesPatch, PropertiesProvenance, PropertiesTarget, PropertyProvenance, Region, RegionHandle, RegionInfo,
    RenderMaterials, RenderMaterialsError, SculptFlags, SculptType, Shape, SitTarget, SurfaceAppearance, SurfaceAppearanceError, Transform,
    UnresolvedReason, WorldPose, WorldTransform, WorldTransformState,
};
pub use state::{
    MAX_OBJECT_DETAIL_BATCH_BYTES, MAX_OBJECT_DETAIL_BATCH_ITEMS, MAX_OBJECT_DETAIL_BYTES, MAX_OBJECT_PAGE_ITEMS,
    MAX_QUERY_STATE_MAINTENANCE_WORK, MAX_RECENT_OBJECT_BYTES, MAX_RECENT_OBJECTS, MAX_REGION_PAGE_BYTES, MAX_REGION_PAGE_ITEMS,
    MAX_WORLD_CHANGE_PAGE_ITEMS, MAX_WORLD_QUERY_RESULTS, MAX_WORLD_QUERY_WORK, Object, ObjectDetail, ObjectDetailBatch,
    ObjectDetailResult, ObjectListItem, ObjectListItemRead, ObjectListPage, ObjectPage, ObjectPageCursor, ObjectSummary, ObjectSummaryRead,
    QueryCompleteness, QueryIncompleteReason, QueryScope, QueryStateMaintenance, QueryUnresolved, RECENT_OBJECT_TTL, RecentObject,
    RegionPage, RegionPageCursor, RegionSummary, RegionSummaryRead, ValueFilter, World, WorldChange, WorldChangeClasses, WorldChangeCursor,
    WorldChangePage, WorldChangeRecord, WorldEntityRemovalReason, WorldQuery, WorldQueryBudget, WorldQueryBuildError, WorldQueryCursor,
    WorldQueryItem, WorldQueryPage, WorldReadBarrier, WorldReadError,
};
pub use update::{
    AgentHandoff, Applied, ApplyError, ApplyOutcome, DeferredReason, FieldGroup, FieldRejection, FieldRejectionReason, ObjectObservation,
    ObjectPatch, OrderEvidence, Recovery, Rejections, UpdateAuthority, UpdateSource, WorldLimits, WorldRevision,
};
