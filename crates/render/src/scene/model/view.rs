use std::{
    collections::VecDeque,
    error::Error,
    fmt::{self, Display, Formatter},
};

use world::{Entity, Region, WorldRevision};

use super::{Batch, Change, MeshChange, Mirror, SceneLifetime, Status};
use crate::scene::{Object, Patch, RegionData, Surface, ViewerEnvironment, region_environment::RegionEnvironment};

const CHANGE_HISTORY: usize = 64;

pub(crate) struct SceneChanges {
    pub(crate) objects: Vec<Entity>,
    pub(crate) surfaces: Vec<(Region, crate::scene::SurfaceKind)>,
    pub(crate) regions: Vec<Region>,
    pub(crate) retired: Vec<Region>,
    pub(crate) full: bool,
}

impl SceneChanges {
    fn from_batch(batch: &Batch) -> Self {
        let mut changes = Self { objects: Vec::new(), surfaces: Vec::new(), regions: Vec::new(), retired: Vec::new(), full: batch.replace };
        for change in &batch.changes {
            match change {
                Change::Object(object) => changes.objects.push(object.detail().summary.entity),
                Change::Remove(entity) => changes.objects.push(*entity),
                Change::Mesh(MeshChange::Set(entity, ..) | MeshChange::Clear(entity)) => changes.objects.push(*entity),
                Change::Material(change) => changes.objects.push(change.source.detail().summary.entity),
                Change::Region(region) => changes.regions.push(region.summary().region),
                Change::Retire(region) => changes.retired.push(*region),
                Change::Surface(surface) => changes.surfaces.push((surface.region, surface.kind)),
                Change::RemoveSurface(region, kind) => changes.surfaces.push((*region, *kind)),
                Change::ViewerEnvironment(_) | Change::Environment(..) | Change::Water(..) | Change::Patch(..) => {}
            }
        }
        changes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyError {
    StaleSession,
    OutOfOrder,
}

impl Display for ApplyError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::StaleSession => "the Scene session ended or the batch belongs to another session",
            Self::OutOfOrder => "the Scene publication sequence has a gap or duplicate",
        })
    }
}

impl Error for ApplyError {}

#[derive(Clone, Debug)]
pub struct ViewVersion {
    session: SceneLifetime,
    sequence: u64,
    status: Status,
}

impl PartialEq for ViewVersion {
    fn eq(&self, other: &Self) -> bool {
        self.session.same(&other.session) && self.sequence == other.sequence && self.status == other.status
    }
}

impl Eq for ViewVersion {}

pub struct SceneView {
    session: SceneLifetime,
    sequence: u64,
    revision: Option<WorldRevision>,
    status: Status,
    mirror: Mirror,
    change_history: VecDeque<(u64, SceneChanges)>,
}

impl SceneView {
    pub fn viewer_environment(&self) -> Option<&ViewerEnvironment> {
        if self.session.is_active() { self.mirror.viewer_environment.as_ref() } else { None }
    }

    pub(in crate::scene) fn new(session: SceneLifetime) -> Self {
        Self {
            session,
            sequence: 0,
            revision: None,
            status: Status::Loading,
            mirror: Mirror::default(),
            change_history: VecDeque::with_capacity(CHANGE_HISTORY),
        }
    }

    pub fn apply(&mut self, batch: Batch) -> Result<(), ApplyError> {
        if !self.session.is_active() || !self.session.same(&batch.session) {
            return Err(ApplyError::StaleSession);
        }
        if self.sequence.checked_add(1) != Some(batch.sequence) {
            return Err(ApplyError::OutOfOrder);
        }
        let changes = SceneChanges::from_batch(&batch);
        if batch.replace {
            self.mirror = Mirror::default();
        }
        for change in batch.changes {
            self.mirror.apply(change);
        }
        self.sequence = batch.sequence;
        self.revision = batch.revision;
        self.status = batch.status;
        self.change_history.push_back((self.sequence, changes));
        if self.change_history.len() > CHANGE_HISTORY {
            self.change_history.pop_front();
        }
        batch.delivery.acknowledge();
        Ok(())
    }

    pub fn status(&self) -> Status {
        if self.session.is_active() { self.status } else { Status::Closed }
    }
    pub fn version(&self) -> ViewVersion {
        ViewVersion { session: self.session.clone(), sequence: self.sequence, status: self.status() }
    }
    pub fn revision(&self) -> Option<WorldRevision> {
        self.revision
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn object_count(&self) -> usize {
        if self.session.is_active() { self.mirror.objects.len() } else { 0 }
    }
    pub fn region_count(&self) -> usize {
        if self.session.is_active() { self.mirror.regions.len() } else { 0 }
    }
    pub fn patch_count(&self) -> usize {
        if self.session.is_active() { self.mirror.patches.len() } else { 0 }
    }
    pub fn objects(&self) -> impl Iterator<Item = &Object> {
        self.mirror.objects.values().filter(|_| self.session.is_active())
    }
    pub fn object(&self, entity: Entity) -> Option<&Object> {
        if self.session.is_active() { self.mirror.objects.get(&entity) } else { None }
    }
    pub fn regions(&self) -> impl Iterator<Item = &RegionData> {
        self.mirror.regions.values().filter(|_| self.session.is_active())
    }
    pub fn region(&self, region: Region) -> Option<&RegionData> {
        if self.session.is_active() { self.mirror.regions.get(&region) } else { None }
    }

    pub fn environment(&self, region: Region) -> Option<&RegionEnvironment> {
        if self.session.is_active() { self.mirror.environments.get(&region) } else { None }
    }
    pub fn patches(&self) -> impl Iterator<Item = (Region, &Patch)> {
        self.mirror.patches.iter().filter(|_| self.session.is_active()).map(|((region, _), patch)| (*region, patch))
    }
    pub fn water_height(&self, region: Region) -> Option<f32> {
        if self.session.is_active() { self.mirror.water.get(&region).copied().flatten() } else { None }
    }

    pub fn surfaces(&self) -> impl Iterator<Item = &Surface> {
        self.mirror.surfaces.values().filter(|_| self.session.is_active())
    }

    pub(crate) fn surface(&self, region: Region, kind: crate::scene::SurfaceKind) -> Option<&Surface> {
        if self.session.is_active() { self.mirror.surfaces.get(&(region, kind)) } else { None }
    }

    pub(crate) fn changes_since<'a>(&'a self, version: &ViewVersion) -> Option<impl Iterator<Item = &'a SceneChanges>> {
        if !self.session.same(&version.session) || version.sequence > self.sequence || version.status != self.status() {
            return None;
        }
        let next = version.sequence.checked_add(1)?;
        if next <= self.sequence && self.change_history.front().is_none_or(|(sequence, _)| *sequence > next) {
            return None;
        }
        let sequence = version.sequence;
        Some(self.change_history.iter().filter(move |(current, _)| *current > sequence).map(|(_, changes)| changes))
    }
}
