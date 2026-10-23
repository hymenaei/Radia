use std::{fmt, sync::Arc};

use utils::AssetId;

use crate::{ApplyError, Region, RegionApplied, RegionOutcome, UpdateSource, WorldRevision, identity::WorldId, update::FieldStamp};

use super::{World, WorldChange, newer_field};

pub const MAX_ENVIRONMENT_SOURCE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ENVIRONMENT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ENVIRONMENT_READ_BYTES: usize = size_of::<EnvironmentRead>() + 2 * MAX_ENVIRONMENT_SOURCE_BYTES + 4 * size_of::<usize>();

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ParcelId(u32);

impl ParcelId {
    pub fn new(value: i32) -> Option<Self> {
        u32::try_from(value).ok().map(Self)
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EnvironmentScope {
    Region,
    Parcel(ParcelId),
}

#[derive(Clone, PartialEq)]
pub enum EnvironmentSource {
    Inherit,
    Asset(AssetId),
    Inline(Arc<[u8]>),
}

impl fmt::Debug for EnvironmentSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inherit => f.write_str("Inherit"),
            Self::Asset(asset) => f.debug_tuple("Asset").field(asset).finish(),
            Self::Inline(bytes) => f.debug_struct("Inline").field("bytes", &bytes.len()).finish(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnvironmentAssignment {
    pub source: EnvironmentSource,
    pub version: Option<u32>,
    pub day_length_seconds: Option<u32>,
    pub day_offset_seconds: Option<i32>,
    pub altitudes: Option<[f32; 3]>,
}

impl EnvironmentAssignment {
    fn validate(&self) -> Result<(), ApplyError> {
        match &self.source {
            EnvironmentSource::Asset(asset) if asset.uuid().is_nil() => {
                return Err(ApplyError::InvalidValue { field: "environment_asset" });
            }
            EnvironmentSource::Inline(bytes) if bytes.is_empty() || bytes.len() > MAX_ENVIRONMENT_SOURCE_BYTES => {
                return Err(ApplyError::InvalidValue { field: "environment_source" });
            }
            _ => {}
        }
        if self.altitudes.is_some_and(|altitudes| altitudes.into_iter().any(|value| !value.is_finite())) {
            return Err(ApplyError::InvalidValue { field: "environment_altitudes" });
        }
        Ok(())
    }

    fn source_bytes(&self) -> usize {
        match &self.source {
            EnvironmentSource::Inline(bytes) => bytes.len(),
            EnvironmentSource::Inherit | EnvironmentSource::Asset(_) => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParcelEnvironment {
    pub parcel: ParcelId,
    pub assignment: Option<EnvironmentAssignment>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnvironmentRead {
    pub region: Region,
    pub hydration_revision: WorldRevision,
    pub region_assignment: Option<EnvironmentAssignment>,
    pub parcel: Option<ParcelEnvironment>,
}

impl EnvironmentRead {
    pub fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.region_assignment.as_ref().map_or(0, assignment_bytes)
            + self.parcel.as_ref().and_then(|parcel| parcel.assignment.as_ref()).map_or(0, assignment_bytes)
    }
}

fn assignment_bytes(assignment: &EnvironmentAssignment) -> usize {
    match &assignment.source {
        EnvironmentSource::Inline(bytes) => bytes.len() + 2 * size_of::<usize>(),
        EnvironmentSource::Inherit | EnvironmentSource::Asset(_) => 0,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnvironmentTarget {
    world: WorldId,
    region: Region,
    scope: EnvironmentScope,
    parcel_revision: Option<WorldRevision>,
}

impl EnvironmentTarget {
    pub fn region(self) -> Region {
        self.region
    }

    pub fn scope(self) -> EnvironmentScope {
        self.scope
    }
}

struct Observed {
    assignment: EnvironmentAssignment,
    fence: FieldStamp,
}

#[derive(Default)]
pub(super) struct Environments {
    region: Option<Observed>,
    parcel: Option<Observed>,
    selected_parcel: Option<(ParcelId, WorldRevision)>,
    parcel_fence: Option<FieldStamp>,
}

impl Environments {
    pub(super) fn source_bytes(&self) -> usize {
        [&self.region, &self.parcel].into_iter().flatten().map(|observed| observed.assignment.source_bytes()).sum()
    }

    fn assignment(&self, scope: EnvironmentScope) -> &Option<Observed> {
        match scope {
            EnvironmentScope::Region => &self.region,
            EnvironmentScope::Parcel(_) => &self.parcel,
        }
    }
}

impl World {
    pub fn environment(&self, region: Region) -> Option<EnvironmentRead> {
        let environment = &self.regions.get(&region)?.environment;
        Some(EnvironmentRead {
            region,
            hydration_revision: self.revision(),
            region_assignment: environment.region.as_ref().map(|observed| observed.assignment.clone()),
            parcel: environment.selected_parcel.map(|(parcel, _)| ParcelEnvironment {
                parcel,
                assignment: environment.parcel.as_ref().map(|observed| observed.assignment.clone()),
            }),
        })
    }

    pub fn environment_target(&self, region: Region, scope: EnvironmentScope) -> Option<EnvironmentTarget> {
        let environment = &self.regions.get(&region)?.environment;
        let parcel_revision = match scope {
            EnvironmentScope::Region => None,
            EnvironmentScope::Parcel(parcel) => {
                let (selected, revision) = environment.selected_parcel?;
                if parcel != selected {
                    return None;
                }
                Some(revision)
            }
        };
        Some(EnvironmentTarget { world: self.id, region, scope, parcel_revision })
    }

    pub fn observe_agent_parcel(&mut self, source: UpdateSource, parcel: Option<ParcelId>) -> Result<RegionApplied, ApplyError> {
        let region = source.region();
        self.require_active_region(region)?;
        let Some(stamp) = source.stamp() else { return Ok(unchanged()) };
        let fence = FieldStamp { authority: source.authority(), stamp };
        let environment = &self.regions[&region].environment;
        if !newer_field(fence, environment.parcel_fence) {
            return Ok(unchanged());
        }
        if environment.selected_parcel.map(|(parcel, _)| parcel) == parcel {
            self.regions.get_mut(&region).expect("active region").environment.parcel_fence = Some(fence);
            return Ok(unchanged());
        }
        self.ensure_revision_available()?;
        let revision = WorldRevision(self.revision().get() + 1);
        let environment = &mut self.regions.get_mut(&region).expect("active region").environment;
        self.environment_bytes -= environment.parcel.as_ref().map_or(0, |observed| observed.assignment.source_bytes());
        environment.parcel = None;
        environment.selected_parcel = parcel.map(|parcel| (parcel, revision));
        environment.parcel_fence = Some(fence);
        let revision = self.publish_changes(vec![WorldChange::EnvironmentUpdated(region)]).expect("parcel selection changed");
        Ok(RegionApplied { outcome: RegionOutcome::Patched { revision }, recovery_cancellations: Vec::new() })
    }

    pub fn observe_environment(
        &mut self,
        target: EnvironmentTarget,
        source: UpdateSource,
        assignment: EnvironmentAssignment,
    ) -> Result<RegionApplied, ApplyError> {
        if target.world != self.id || self.environment_target(target.region, target.scope) != Some(target) {
            return Ok(unchanged());
        }
        if source.region() != target.region {
            return Err(ApplyError::InvalidValue { field: "environment_region" });
        }
        assignment.validate()?;
        let Some(stamp) = source.stamp() else { return Ok(unchanged()) };
        let fence = FieldStamp { authority: source.authority(), stamp };
        let current = self.regions[&target.region].environment.assignment(target.scope);
        if !newer_field(fence, current.as_ref().map(|current| current.fence)) {
            return Ok(unchanged());
        }
        if let Some(current) = current
            && let (Some(incoming), Some(previous)) = (assignment.version, current.assignment.version)
            && (incoming < previous || (incoming == previous && assignment != current.assignment))
        {
            return Ok(unchanged());
        }
        let same = current.as_ref().is_some_and(|current| current.assignment == assignment);
        let bytes =
            self.environment_bytes - current.as_ref().map_or(0, |current| current.assignment.source_bytes()) + assignment.source_bytes();
        if bytes > MAX_ENVIRONMENT_BYTES {
            return Err(ApplyError::EnvironmentBudgetExhausted);
        }
        if !same {
            self.ensure_revision_available()?;
        }
        let environment = &mut self.regions.get_mut(&target.region).expect("current target region").environment;
        let slot = match target.scope {
            EnvironmentScope::Region => &mut environment.region,
            EnvironmentScope::Parcel(_) => &mut environment.parcel,
        };
        *slot = Some(Observed { assignment, fence });
        self.environment_bytes = bytes;
        if same {
            return Ok(unchanged());
        }
        let revision = self.publish_changes(vec![WorldChange::EnvironmentUpdated(target.region)]).expect("environment changed");
        Ok(RegionApplied { outcome: RegionOutcome::Patched { revision }, recovery_cancellations: Vec::new() })
    }
}

fn unchanged() -> RegionApplied {
    RegionApplied { outcome: RegionOutcome::Unchanged, recovery_cancellations: Vec::new() }
}
