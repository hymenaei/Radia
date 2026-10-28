use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};

use assets::TextureHandle;
use utils::AssetId;
use world::{Entity, Region, WorldRevision};

use super::{Batch, Mirror, SceneLifetime, Status};
use crate::scene::{Object, Patch, RegionData, Surface, ViewerEnvironment, region_environment::RegionEnvironment};

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
    geometry_sequence: u64,
    revision: Option<WorldRevision>,
    status: Status,
    mirror: Mirror,
}

impl SceneView {
    pub fn viewer_environment(&self) -> Option<&ViewerEnvironment> {
        if self.session.is_active() { self.mirror.viewer_environment.as_ref() } else { None }
    }

    pub(in crate::scene) fn new(session: SceneLifetime) -> Self {
        Self { session, sequence: 0, geometry_sequence: 0, revision: None, status: Status::Loading, mirror: Mirror::default() }
    }

    pub fn apply(&mut self, batch: Batch) -> Result<(), ApplyError> {
        if !self.session.is_active() || !self.session.same(&batch.session) {
            return Err(ApplyError::StaleSession);
        }
        if self.sequence.checked_add(1) != Some(batch.sequence) {
            return Err(ApplyError::OutOfOrder);
        }
        let geometry_changed = batch.replace || batch.changes.iter().any(change_affects_geometry);
        if batch.replace {
            self.mirror = Mirror::default();
        }
        for change in batch.changes {
            self.mirror.apply(change);
        }
        self.sequence = batch.sequence;
        if geometry_changed {
            self.geometry_sequence = self.geometry_sequence.checked_add(1).expect("scene geometry version space exhausted");
        }
        self.revision = batch.revision;
        self.status = batch.status;
        batch.delivery.acknowledge();
        Ok(())
    }

    pub fn status(&self) -> Status {
        if self.session.is_active() { self.status } else { Status::Closed }
    }
    pub fn version(&self) -> ViewVersion {
        ViewVersion { session: self.session.clone(), sequence: self.sequence, status: self.status() }
    }
    pub(crate) fn geometry_version(&self) -> ViewVersion {
        ViewVersion { session: self.session.clone(), sequence: self.geometry_sequence, status: self.status() }
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
    pub fn environment_texture(&self, id: AssetId) -> Option<&TextureHandle> {
        if self.session.is_active() { self.mirror.environment_textures.get(&id) } else { None }
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
}

fn change_affects_geometry(change: &super::Change) -> bool {
    matches!(
        change,
        super::Change::Object(_)
            | super::Change::Mesh(_)
            | super::Change::Material(_)
            | super::Change::Remove(_)
            | super::Change::Region(_)
            | super::Change::Retire(_)
            | super::Change::Patch(_, _)
            | super::Change::Surface(_)
            | super::Change::RemoveSurface(_, _)
    )
}
