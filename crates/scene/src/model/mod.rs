mod object;
mod view;

pub use object::{Geometry, HiddenReason, Object, Placement};
pub use view::{ApplyError, SceneView, ViewVersion};

use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
};

use world::{Entity, EnvironmentRead, Region, RegionSummary, TerrainPatch, TerrainPatchId, WorldRevision};

use crate::{MeshKey, PreparedMesh, Surface, SurfaceKind, ViewerEnvironment, asset_demand::MaterialChange, budget::Lease};

#[derive(Clone, Debug)]
pub(crate) struct SceneLifetime(Arc<AtomicBool>);

impl SceneLifetime {
    pub(crate) fn new() -> Self {
        Self(Arc::new(AtomicBool::new(true)))
    }
    pub(crate) fn is_active(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub(crate) fn close(&self) {
        self.0.store(false, Ordering::Release);
    }
    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
pub struct RegionData(Arc<RegionValue>);

impl RegionData {
    pub(crate) fn new(summary: RegionSummary, lease: Lease) -> Self {
        Self(Arc::new(RegionValue { summary, _lease: lease }))
    }
    pub fn summary(&self) -> &RegionSummary {
        &self.0.summary
    }
}

#[derive(Clone)]
pub(crate) struct EnvironmentData(Arc<EnvironmentValue>);

impl EnvironmentData {
    pub(crate) fn new(source: EnvironmentRead, lease: Lease) -> Self {
        Self(Arc::new(EnvironmentValue { source, _lease: lease }))
    }

    pub(crate) fn source(&self) -> &EnvironmentRead {
        &self.0.source
    }
}

struct EnvironmentValue {
    source: EnvironmentRead,
    _lease: Lease,
}

struct RegionValue {
    summary: RegionSummary,
    _lease: Lease,
}

#[derive(Clone)]
pub struct Patch(Arc<PatchValue>);

impl Patch {
    pub(crate) fn new(patch: TerrainPatch, lease: Lease) -> Self {
        Self(Arc::new(PatchValue { patch, _lease: lease }))
    }
    pub fn source(&self) -> &TerrainPatch {
        &self.0.patch
    }
}

struct PatchValue {
    patch: TerrainPatch,
    _lease: Lease,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryReason {
    HistoryLost,
    Capacity,
    ReadFailed,
    InvalidReply,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Loading,
    Ready,
    Recovering { reason: RecoveryReason, attempt: u8 },
    Degraded { reason: RecoveryReason },
    Closed,
}

pub(crate) enum MeshChange {
    Set(Entity, MeshKey, PreparedMesh),
    Clear(Entity),
}

pub(crate) enum Change {
    ViewerEnvironment(Option<ViewerEnvironment>),
    Object(Object),
    Mesh(MeshChange),
    Material(MaterialChange),
    Remove(Entity),
    Region(RegionData),
    Environment(Region, Option<EnvironmentData>),
    Retire(Region),
    Patch(Region, Patch),
    Water(Region, Option<f32>),
    Surface(Surface),
    RemoveSurface(Region, SurfaceKind),
}

pub struct Batch {
    pub(crate) session: SceneLifetime,
    pub(crate) sequence: u64,
    pub(crate) revision: Option<WorldRevision>,
    pub(crate) status: Status,
    pub(crate) replace: bool,
    pub(crate) changes: Vec<Change>,
    pub(crate) receipt: Arc<AtomicU8>,
}

impl Batch {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn status(&self) -> Status {
        self.status
    }
}

impl Drop for Batch {
    fn drop(&mut self) {
        let _ = self.receipt.compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire);
    }
}

#[derive(Default)]
pub(crate) struct Mirror {
    pub(crate) viewer_environment: Option<ViewerEnvironment>,
    pub(crate) objects: HashMap<Entity, Object>,
    pub(crate) regions: BTreeMap<Region, RegionData>,
    pub(crate) environments: BTreeMap<Region, EnvironmentData>,
    pub(crate) patches: BTreeMap<(Region, TerrainPatchId), Patch>,
    pub(crate) water: BTreeMap<Region, Option<f32>>,
    pub(crate) surfaces: BTreeMap<(Region, SurfaceKind), Surface>,
}

impl Mirror {
    pub(crate) fn apply_mesh(&mut self, change: &MeshChange) {
        match change {
            MeshChange::Set(entity, key, state) => {
                if let Some(object) = self.objects.get_mut(entity)
                    && object.geometry() == Geometry::Prim(key.shape)
                {
                    if let PreparedMesh::Ready(mesh) = state {
                        object.mesh = Some((*key, mesh.clone()));
                    }
                    object.preparation = Some((*key, state.clone()));
                }
            }
            MeshChange::Clear(entity) => {
                if let Some(object) = self.objects.get_mut(entity) {
                    object.preparation = None;
                    object.mesh = None;
                }
            }
        }
    }

    pub(crate) fn apply(&mut self, change: Change) {
        match change {
            Change::ViewerEnvironment(environment) => self.viewer_environment = environment,
            Change::Object(mut object) => {
                if let Some(old) = self.objects.get(&object.detail().summary.entity)
                    && old.geometry() == object.geometry()
                    && object.preparation.is_none()
                {
                    object.preparation = old.preparation.clone();
                    object.mesh = old.mesh.clone();
                    if object.same_appearance(old) {
                        object.materials = old.materials.clone();
                    }
                }
                self.objects.insert(object.detail().summary.entity, object);
            }
            Change::Mesh(change) => self.apply_mesh(&change),
            Change::Material(change) => {
                if let Some(object) = self.objects.get_mut(&change.source.detail().summary.entity)
                    && object.same_appearance(&change.source)
                {
                    object.materials = change.materials;
                }
            }
            Change::Remove(entity) => {
                self.objects.remove(&entity);
            }
            Change::Region(region) => {
                self.regions.insert(region.summary().region, region);
            }
            Change::Retire(region) => {
                self.regions.remove(&region);
                self.environments.remove(&region);
                self.water.remove(&region);
                self.patches.retain(|(r, _), _| *r != region);
                self.surfaces.retain(|(r, _), _| *r != region);
            }
            Change::Patch(region, patch) => {
                self.patches.insert((region, patch.source().id()), patch);
            }
            Change::Environment(region, source) => match source {
                Some(source) => {
                    self.environments.insert(region, source);
                }
                None => {
                    self.environments.remove(&region);
                }
            },
            Change::Water(region, height) => {
                self.water.insert(region, height);
            }
            Change::Surface(surface) => {
                self.surfaces.insert((surface.region, surface.kind), surface);
            }
            Change::RemoveSurface(region, kind) => {
                self.surfaces.remove(&(region, kind));
            }
        }
    }
}
