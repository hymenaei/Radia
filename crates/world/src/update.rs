use std::{
    collections::HashSet,
    error::Error,
    fmt::{self, Display, Formatter},
    mem::size_of,
    num::NonZeroU32,
    time::{Duration, Instant},
};

use math::Vec3;
use uuid::Uuid;

use crate::AvatarAppearanceUpdate;
use crate::ObjectMediaUpdate;
use crate::{
    Appearance, AttachmentPoint, Entity, Kind, LinearMotion, LocalAddress, ParentUpdate, PbrMaterialAssignments, PbrMaterialOverrides,
    Pose, Region, Shape, SurfaceAppearance, TextureAnimation,
};
use OrderEvidence::{Ambiguous, Degraded, Ordered};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderEvidence {
    Ordered(u64),
    Ambiguous,
    Degraded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateAuthority {
    Tracked,
    Untracked,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MotionUpdateKind {
    #[default]
    Full,
    Terse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UpdateSource {
    pub(super) region: Region,
    pub(super) sequence: u32,
    pub(super) order: OrderEvidence,
    pub(super) block_ordinal: u32,
    pub(super) authority: UpdateAuthority,
    motion_update_kind: MotionUpdateKind,
    explicit_angular_velocity: bool,
    motion_received_at: Option<Instant>,
    motion_ping_delay: Duration,
}

impl UpdateSource {
    pub const fn new(region: Region, sequence: u32, order: OrderEvidence, block_ordinal: u32) -> Self {
        Self {
            region,
            sequence,
            order,
            block_ordinal,
            authority: UpdateAuthority::Tracked,
            motion_update_kind: MotionUpdateKind::Full,
            explicit_angular_velocity: false,
            motion_received_at: None,
            motion_ping_delay: Duration::ZERO,
        }
    }

    pub const fn with_authority(mut self, authority: UpdateAuthority) -> Self {
        self.authority = authority;
        self
    }

    pub const fn with_motion_update_kind(mut self, kind: MotionUpdateKind) -> Self {
        self.motion_update_kind = kind;
        self
    }

    pub(crate) const fn with_explicit_angular_velocity(mut self, explicit: bool) -> Self {
        self.explicit_angular_velocity = explicit;
        self
    }

    pub const fn with_motion_timing(mut self, received_at: Instant, ping_delay: Duration) -> Self {
        self.motion_received_at = Some(received_at);
        self.motion_ping_delay = ping_delay;
        self
    }

    pub const fn authority(self) -> UpdateAuthority {
        self.authority
    }

    pub const fn motion_update_kind(self) -> MotionUpdateKind {
        self.motion_update_kind
    }

    pub const fn has_explicit_angular_velocity(self) -> bool {
        self.explicit_angular_velocity
    }

    pub const fn motion_received_at(self) -> Option<Instant> {
        self.motion_received_at
    }

    pub const fn motion_ping_delay(self) -> Duration {
        self.motion_ping_delay
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

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FieldGroup {
    Pose,
    LinearMotion,
    AngularVelocity,
    Physical,
    TimeDilation,
    Scale,
    Parent,
    AttachmentPoint,
    Shape,
    Appearance,
    TextureEntry,
    TextureAnimation,
    PbrMaterials,
    ObjectMedia,
    SitTarget,
    AvatarAppearance,
    PbrMaterialOverrides,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FieldUpdate<T> {
    #[default]
    Keep,
    Set(T),
    Clear,
}

impl<T> FieldUpdate<T> {
    pub const fn is_present(&self) -> bool {
        matches!(self, Self::Set(_) | Self::Clear)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldRejectionReason {
    InvalidValue { field: &'static str },
    ResourceLimit { field: &'static str },
    InvalidRotation,
    ParentCycle,
    ParentDependsOnRejectedParent,
    StaleFrame,
    UntrustedSource,
    StaleVersion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldRejection {
    pub group: FieldGroup,
    pub reason: FieldRejectionReason,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rejections {
    entries: Vec<FieldRejection>,
}

impl Rejections {
    pub const fn new() -> Self {
        Self { entries: Vec::new() }
    }

    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &FieldRejection> {
        self.entries.iter()
    }

    pub(crate) fn reject(&mut self, group: FieldGroup, reason: FieldRejectionReason) {
        if let Err(index) = self.entries.binary_search_by_key(&group, |entry| entry.group) {
            self.entries.insert(index, FieldRejection { group, reason });
        }
    }
}

impl Default for Rejections {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectObservation {
    pub source: UpdateSource,
    pub id: Uuid,
    pub local_id: NonZeroU32,
    pub kind: Kind,
    pub parent: ParentUpdate,
    pub pose: Option<Pose>,
    pub linear_motion: Option<LinearMotion>,
    pub angular_velocity: Option<Vec3>,
    pub is_physical: Option<bool>,
    pub time_dilation: Option<f32>,
    pub scale: Option<Vec3>,
    pub attachment_point: FieldUpdate<AttachmentPoint>,
    pub shape: FieldUpdate<Shape>,
    pub appearance: FieldUpdate<Appearance>,
    pub surface_appearance: Option<SurfaceAppearance>,
    pub texture_animation: FieldUpdate<TextureAnimation>,
    pub pbr_materials: Option<PbrMaterialAssignments>,
}

impl ObjectObservation {
    pub const fn new(source: UpdateSource, id: Uuid, local_id: NonZeroU32, kind: Kind, parent: ParentUpdate) -> Self {
        Self {
            source,
            id,
            local_id,
            kind,
            parent,
            pose: None,
            linear_motion: None,
            angular_velocity: None,
            is_physical: None,
            time_dilation: None,
            scale: None,
            attachment_point: FieldUpdate::Keep,
            shape: FieldUpdate::Keep,
            appearance: FieldUpdate::Keep,
            surface_appearance: None,
            texture_animation: FieldUpdate::Keep,
            pbr_materials: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectPatch {
    pub source: UpdateSource,
    pub local_id: NonZeroU32,
    pub pose: Option<Pose>,
    pub linear_motion: LinearMotion,
    pub angular_velocity: Option<Vec3>,
    pub is_physical: Option<bool>,
    pub time_dilation: Option<f32>,
    pub scale: Option<Vec3>,
    pub parent: Option<ParentUpdate>,
    pub attachment_point: FieldUpdate<AttachmentPoint>,
    pub shape: FieldUpdate<Shape>,
    pub appearance: FieldUpdate<Appearance>,
    pub surface_appearance: Option<SurfaceAppearance>,
    pub texture_animation: FieldUpdate<TextureAnimation>,
    pub pbr_materials: Option<PbrMaterialAssignments>,
    pub pbr_material_overrides: Option<PbrMaterialOverrides>,
    pub object_media: Option<ObjectMediaUpdate>,
    pub avatar_appearance: Option<AvatarAppearanceUpdate>,
}

impl ObjectPatch {
    pub const fn new(source: UpdateSource, local_id: NonZeroU32) -> Self {
        Self {
            source,
            local_id,
            pose: None,
            linear_motion: LinearMotion { velocity: None, acceleration: None },
            angular_velocity: None,
            is_physical: None,
            time_dilation: None,
            scale: None,
            parent: None,
            attachment_point: FieldUpdate::Keep,
            shape: FieldUpdate::Keep,
            appearance: FieldUpdate::Keep,
            surface_appearance: None,
            texture_animation: FieldUpdate::Keep,
            pbr_materials: None,
            pbr_material_overrides: None,
            object_media: None,
            avatar_appearance: None,
        }
    }
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
    Introduced(Entity),
    Updated(Entity),
    Removed(Uuid),
    TombstoneRecorded(LocalAddress),
    AgentHandedOff(Entity),
    Deferred(DeferredReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegionOutcome {
    Unchanged,
    Adopted { revision: WorldRevision },
    Patched { revision: WorldRevision },
    Retired { revision: WorldRevision, entities_removed: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyError {
    StaleRegion,
    RegionRetired,
    RegistrationBudgetExhausted,
    EntityBudgetExhausted,
    TerrainBudgetExhausted,
    EnvironmentBudgetExhausted,
    ParcelBudgetExhausted,
    RevisionExhausted,
    FrameGenerationExhausted,
    AddressGenerationExhausted,
    InvalidValue { field: &'static str },
    KindMismatch { current: Kind, observed: Kind },
}

impl Display for ApplyError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleRegion => f.write_str("region is not registered"),
            Self::RegionRetired => f.write_str("region lifetime is retired"),
            Self::RegistrationBudgetExhausted => f.write_str("region registration budget is exhausted"),
            Self::EntityBudgetExhausted => f.write_str("object budget is exhausted"),
            Self::TerrainBudgetExhausted => f.write_str("terrain patch budget is exhausted"),
            Self::EnvironmentBudgetExhausted => f.write_str("environment source budget is exhausted"),
            Self::ParcelBudgetExhausted => f.write_str("parcel state budget is exhausted"),
            Self::RevisionExhausted => f.write_str("World revision space is exhausted"),
            Self::FrameGenerationExhausted => f.write_str("coordinate-frame generation space is exhausted"),
            Self::AddressGenerationExhausted => f.write_str("object address generation space is exhausted"),
            Self::InvalidValue { field } => write!(f, "invalid value for {field}"),
            Self::KindMismatch { current, observed } => write!(f, "object kind mismatch: stored {current:?}, observed {observed:?}"),
        }
    }
}

impl Error for ApplyError {}

#[must_use]
#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    pub outcome: ApplyOutcome,
    pub revision: Option<WorldRevision>,
    pub recovery: Recovery,
    pub recovery_cancellations: Vec<LocalAddress>,
    pub rejections: Rejections,
}

impl Applied {
    pub(crate) fn new(outcome: ApplyOutcome) -> Self {
        Self {
            outcome,
            revision: None,
            recovery: Recovery::default(),
            recovery_cancellations: Vec::new(),
            rejections: Rejections::default(),
        }
    }

    pub(crate) fn unchanged() -> Self {
        Self::new(ApplyOutcome::Unchanged)
    }

    pub(crate) fn deferred(reason: DeferredReason, recovery: Recovery) -> Self {
        Self { recovery, ..Self::new(ApplyOutcome::Deferred(reason)) }
    }

    pub(crate) fn with_rejections(mut self, rejections: Rejections) -> Self {
        self.rejections = rejections;
        self
    }

    pub(crate) fn with_recovery_satisfied(mut self, satisfied: bool) -> Self {
        self.recovery.satisfied = satisfied;
        self
    }
}

#[must_use]
#[derive(Clone, Debug, PartialEq)]
pub struct RegionApplied {
    pub outcome: RegionOutcome,
    pub recovery_cancellations: Vec<LocalAddress>,
}

impl RegionApplied {
    pub(crate) const fn new(outcome: RegionOutcome) -> Self {
        Self { outcome, recovery_cancellations: Vec::new() }
    }

    pub(crate) const fn unchanged() -> Self {
        Self::new(RegionOutcome::Unchanged)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorldRevision(pub(super) u64);

impl WorldRevision {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Recovery {
    pub requests: Vec<LocalAddress>,
    pub satisfied: bool,
}

impl Recovery {
    pub(crate) fn from_targets(mut targets: Vec<LocalAddress>, max: usize) -> Self {
        let mut seen = HashSet::new();
        targets.retain(|target| seen.insert(*target));
        targets.truncate(max);
        Self { requests: targets, satisfied: false }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorldLimits {
    pub max_region_registrations: usize,
    pub max_entities: usize,
    pub max_entities_per_region: usize,
    pub max_tombstones_per_region: usize,
    pub max_tombstones: usize,
    pub max_tombstone_bytes_per_region: usize,
    pub max_tombstone_bytes: usize,
    pub max_journal_entries: usize,
    pub max_journal_changes: usize,
    pub max_region_name_bytes: usize,
    pub max_recovery_targets: usize,
}

impl WorldLimits {
    pub const fn tombstone_bytes_for_count(count: usize) -> usize {
        count.saturating_mul(size_of::<LocalAddress>() + size_of::<Stamp>())
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::{FieldGroup, FieldRejection, FieldRejectionReason, Recovery, Rejections};
    use crate::{LocalAddress, Region};

    #[test]
    fn recovery_keeps_first_occurrences_within_limit() {
        let region = Region::new(1);
        let first = LocalAddress::new(region, NonZeroU32::new(3).unwrap());
        let second = LocalAddress::new(region, NonZeroU32::new(1).unwrap());
        let third = LocalAddress::new(region, NonZeroU32::new(2).unwrap());
        let targets = vec![first, first, second, third];
        assert_eq!(Recovery::from_targets(targets.clone(), 2).requests, [first, second]);
        assert!(Recovery::from_targets(targets, 0).requests.is_empty());
    }

    #[test]
    fn rejections_keep_field_order_and_the_first_reason() {
        let first = FieldRejection { group: FieldGroup::Pose, reason: FieldRejectionReason::InvalidRotation };
        let later = FieldRejection { group: FieldGroup::Shape, reason: FieldRejectionReason::UntrustedSource };
        let mut rejections = Rejections::new();
        rejections.reject(later.group, later.reason);
        rejections.reject(first.group, first.reason);
        rejections.reject(FieldGroup::Pose, FieldRejectionReason::StaleFrame);

        assert_eq!(rejections.len(), 2);
        assert_eq!(rejections.iter().copied().collect::<Vec<_>>(), [first, later]);
    }
}
