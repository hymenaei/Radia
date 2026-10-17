use std::{mem::size_of, num::NonZeroU32};

use base::UUID;
use math::Vec3;

use crate::AvatarAppearanceUpdate;
use crate::ObjectMediaUpdate;
use crate::model::{
    Appearance, AttachmentPoint, Entity, GltfMaterialOverrides, Kind, LinearMotion, LocalAddress, MotionPatch, ParentUpdate, Pose, Region,
    RenderMaterials, Shape, SurfaceAppearance,
};

use OrderEvidence::{Ambiguous, Degraded, Ordered};

/// Source order is extended by the owning circuit. World never interprets raw wraparound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderEvidence {
    Ordered(u64),
    Ambiguous,
    Degraded,
}

/// Trust level assigned to an update source by the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateAuthority {
    /// This source may replace data from an untracked source, regardless of order.
    Tracked,
    /// This source cannot replace data from a tracked source, even at a newer order.
    Untracked,
}

/// Source evidence for one decoded LLUDP object block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UpdateSource {
    pub(super) region: Region,
    pub(super) sequence: u32,
    pub(super) order: OrderEvidence,
    pub(super) block_ordinal: u32,
    pub(super) authority: UpdateAuthority,
}

impl UpdateSource {
    /// Creates a source whose ordering evidence is tracked.
    pub const fn new(region: Region, sequence: u32, order: OrderEvidence, block_ordinal: u32) -> Self {
        Self { region, sequence, order, block_ordinal, authority: UpdateAuthority::Tracked }
    }

    /// Sets the source's trust independently from its ordering evidence.
    pub const fn with_authority(mut self, authority: UpdateAuthority) -> Self {
        self.authority = authority;
        self
    }

    /// Returns the source's trust level.
    pub const fn authority(self) -> UpdateAuthority {
        self.authority
    }

    pub const fn region(self) -> Region {
        self.region
    }

    pub const fn sequence(self) -> u32 {
        self.sequence
    }

    pub const fn order(self) -> OrderEvidence {
        self.order
    }

    pub const fn block_ordinal(self) -> u32 {
        self.block_ordinal
    }

    pub(super) fn stamp(self) -> Option<Stamp> {
        match self.order {
            Ordered(order) => Some(Stamp { order, block_ordinal: self.block_ordinal }),
            Ambiguous | Degraded => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct Stamp {
    pub(super) order: u64,
    pub(super) block_ordinal: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FieldStamp {
    pub(super) authority: UpdateAuthority,
    pub(super) stamp: Stamp,
}

/// A field group that the reducer validates and accepts atomically.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FieldGroup {
    Pose,
    LinearMotion,
    AngularVelocity,
    Scale,
    Parent,
    AttachmentPoint,
    Shape,
    /// Surface or avatar appearance category.
    Appearance,
    /// Per-face surface appearance values.
    TextureEntry,
    /// Per-face PBR RenderMaterials bindings.
    RenderMaterials,
    /// Per-face ObjectMedia values and their fetched version.
    ObjectMedia,
    SitTarget,
    /// Owner and creator properties.
    Properties,
    /// Avatar texture slots and positional visual-parameter values.
    AvatarAppearance,
    /// Per-face GLTF material overrides.
    GltfMaterialOverrides,
}

impl FieldGroup {
    const fn index(self) -> usize {
        match self {
            Self::Pose => 0,
            Self::LinearMotion => 1,
            Self::AngularVelocity => 2,
            Self::Scale => 3,
            Self::Parent => 4,
            Self::AttachmentPoint => 5,
            Self::Shape => 6,
            Self::Appearance => 7,
            Self::TextureEntry => 8,
            Self::RenderMaterials => 9,
            Self::ObjectMedia => 10,
            Self::SitTarget => 11,
            Self::Properties => 12,
            Self::AvatarAppearance => 13,
            Self::GltfMaterialOverrides => 14,
        }
    }
}

/// Why the reducer rejected one field group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldRejectionReason {
    /// A supplied numeric value was invalid.
    InvalidValue { field: &'static str },
    /// The field would exceed a World-owned resource budget.
    ResourceLimit { field: &'static str },
    /// The quaternion was non-finite, degenerate, or outside decoder tolerance.
    InvalidRotation,
    /// Applying the proposed parent would create a cycle.
    ParentCycle,
    /// The pose was supplied with a parent proposal that was rejected for a cycle.
    ParentDependsOnRejectedParent,
    /// The pose predates the relationship or attachment frame it would describe.
    StaleFrame,
    /// The update would replace a value established by a tracked source.
    UntrustedSource,
    /// The self avatar appearance message has an older COF version.
    StaleVersion,
}

/// A rejected field group and its cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldRejection {
    pub group: FieldGroup,
    pub reason: FieldRejectionReason,
}

/// Rejections from one update, with at most one entry for each field group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rejections {
    entries: [Option<FieldRejection>; 15],
    len: usize,
}

impl Rejections {
    pub const fn new() -> Self {
        Self { entries: [None; 15], len: 0 }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the rejections in [`FieldGroup`] order.
    pub fn iter(&self) -> impl Iterator<Item = &FieldRejection> {
        self.entries.iter().filter_map(Option::as_ref)
    }

    pub(crate) fn push(&mut self, rejection: FieldRejection) {
        let slot = &mut self.entries[rejection.group.index()];
        if slot.is_none() {
            self.len += 1;
            *slot = Some(rejection);
        }
    }
}

impl Default for Rejections {
    fn default() -> Self {
        Self::new()
    }
}

/// Identity-bearing observation. Parent is always declared, including root.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectObservation {
    pub source: UpdateSource,
    pub id: UUID,
    pub local_id: NonZeroU32,
    pub kind: Kind,
    pub parent: ParentUpdate,
    pub pose: Option<Pose>,
    pub linear_motion: Option<LinearMotion>,
    pub angular_velocity: Option<Vec3>,
    pub scale: Option<Vec3>,
    /// `None` leaves this metadata unchanged; `Some(None)` clears it.
    pub attachment_point: Option<Option<AttachmentPoint>>,
    /// `None` leaves the shape unchanged; `Some(None)` clears it.
    pub shape: Option<Option<Shape>>,
    /// `None` leaves the category unchanged; `Some(None)` clears it.
    pub appearance: Option<Option<Appearance>>,
    /// `None` leaves face data unchanged; a successful entry replaces all decoded face values.
    pub surface_appearance: Option<SurfaceAppearance>,
    /// `None` leaves PBR bindings unchanged; `Some(empty)` clears every face binding.
    pub render_materials: Option<RenderMaterials>,
}

/// A source-local patch. It cannot introduce an object.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectPatch {
    pub source: UpdateSource,
    pub local_id: NonZeroU32,
    pub pose: Option<Pose>,
    pub linear_motion: MotionPatch,
    pub angular_velocity: Option<Vec3>,
    pub scale: Option<Vec3>,
    pub parent: Option<ParentUpdate>,
    /// `None` leaves this metadata unchanged; `Some(None)` clears it.
    pub attachment_point: Option<Option<AttachmentPoint>>,
    /// `None` leaves the shape unchanged; `Some(None)` clears it.
    pub shape: Option<Option<Shape>>,
    /// `None` leaves the category unchanged; `Some(None)` clears it.
    pub appearance: Option<Option<Appearance>>,
    /// `None` leaves face data unchanged; a successful entry replaces all decoded face values.
    pub surface_appearance: Option<SurfaceAppearance>,
    /// `None` leaves PBR bindings unchanged; `Some(empty)` clears every face binding.
    pub render_materials: Option<RenderMaterials>,
    /// `None` leaves GLTF overrides unchanged; `Some(empty)` clears the complete snapshot.
    pub gltf_material_overrides: Option<GltfMaterialOverrides>,
    /// `None` leaves fetched media unchanged; a newer response patches its supplied face prefix.
    pub object_media: Option<ObjectMediaUpdate>,
    /// `None` leaves avatar data unchanged. Positional visual parameters update supplied indices;
    /// a supplied texture table replaces all 45 avatar slots, with nil textures represented as clears.
    pub avatar_appearance: Option<AvatarAppearanceUpdate>,
}

impl ObjectPatch {
    pub const fn new(source: UpdateSource, local_id: NonZeroU32) -> Self {
        Self {
            source,
            local_id,
            pose: None,
            linear_motion: MotionPatch { velocity: None, acceleration: None },
            angular_velocity: None,
            scale: None,
            parent: None,
            attachment_point: None,
            shape: None,
            appearance: None,
            surface_appearance: None,
            render_materials: None,
            gltf_material_overrides: None,
            object_media: None,
            avatar_appearance: None,
        }
    }
}

/// Data for transferring the authenticated agent between region lifetimes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgentHandoff {
    pub source: UpdateSource,
    pub local_id: NonZeroU32,
    pub parent: ParentUpdate,
    pub pose: Option<Pose>,
    pub linear_motion: Option<LinearMotion>,
    pub angular_velocity: Option<Vec3>,
    pub scale: Option<Vec3>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeferredReason {
    UnknownObject,
    AmbiguousOrder,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ApplyOutcome {
    Unchanged,
    IgnoredStale,
    RegionAdopted,
    RegionPatched,
    RegionRetired { entities_removed: usize },
    Introduced(Entity),
    Updated(Entity),
    Removed(UUID),
    TombstoneRecorded(LocalAddress),
    AgentHandedOff(Entity),
    Deferred(DeferredReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyError {
    StaleRegion,
    RegionRetired,
    RegistrationBudgetExhausted,
    EntityBudgetExhausted,
    RevisionExhausted,
    FrameGenerationExhausted,
    AddressGenerationExhausted,
    InvalidValue { field: &'static str },
    KindMismatch { current: Kind, observed: Kind },
}

#[must_use]
#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    pub outcome: ApplyOutcome,
    pub revision: Option<WorldRevision>,
    pub recovery: Option<Recovery>,
    /// Addresses whose pending recovery requests this operation made obsolete.
    pub recovery_cancellations: Vec<LocalAddress>,
    /// Bounded diagnostics for rejected groups. Other groups may still apply.
    pub rejections: Rejections,
    /// Whether the supplied parent declaration passed ordering and validation.
    pub parent_update_accepted: bool,
}

impl Applied {
    pub(crate) fn with_rejections(mut self, rejections: Rejections) -> Self {
        self.rejections = rejections;
        self
    }

    pub(crate) fn with_recovery_cancellations(mut self, addresses: Vec<LocalAddress>) -> Self {
        self.recovery_cancellations = addresses;
        self
    }

    pub(crate) fn with_parent_update_accepted(mut self, accepted: bool) -> Self {
        self.parent_update_accepted = accepted;
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorldRevision(pub(super) u64);

impl WorldRevision {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recovery {
    pub targets: Vec<LocalAddress>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorldLimits {
    pub max_region_registrations: usize,
    pub max_entities: usize,
    pub max_entities_per_region: usize,
    pub max_tombstones_per_region: usize,
    pub max_tombstones: usize,
    /// Charged bytes for retained tombstone address and stamp data in one region.
    pub max_tombstone_bytes_per_region: usize,
    /// Charged bytes for all retained tombstone address and stamp data.
    pub max_tombstone_bytes: usize,
    pub max_journal_entries: usize,
    /// Maximum fixed-size change records retained across journal entries.
    pub max_journal_changes: usize,
    pub max_region_name_bytes: usize,
    pub max_recovery_targets: usize,
}

impl WorldLimits {
    /// Returns the deterministic byte charge for `count` retained tombstones.
    ///
    /// This counts the address and source stamp. Callers should also set count limits, which
    /// bound map and eviction-index overhead that this accounting charge does not include.
    pub const fn tombstone_bytes_for_count(count: usize) -> usize {
        count.saturating_mul(size_of::<LocalAddress>() + size_of::<Stamp>())
    }
}
