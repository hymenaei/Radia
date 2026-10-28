mod delivery;
mod object;
mod view;

pub(in crate::scene) use delivery::{Delivery, DeliveryState, Receipt};
pub use object::{Geometry, HiddenReason, Object, Placement};
pub use view::{ApplyError, SceneView, ViewVersion};

use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use world::{Entity, Region, RegionSummary, TerrainPatch, TerrainPatchId, WorldRevision};

use crate::scene::region_environment::RegionEnvironment;
use crate::scene::{MeshKey, PreparedMesh, Surface, SurfaceKind, ViewerEnvironment, asset_demand::MaterialChange, budget::Lease};

#[derive(Clone, Debug)]
pub(in crate::scene) struct SceneLifetime(Arc<AtomicBool>);

impl SceneLifetime {
    pub(in crate::scene) fn new() -> Self {
        Self(Arc::new(AtomicBool::new(true)))
    }
    pub(in crate::scene) fn is_active(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub(in crate::scene) fn close(&self) {
        self.0.store(false, Ordering::Release);
    }
    pub(in crate::scene) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
pub struct RegionData(Arc<RegionValue>);

impl RegionData {
    pub(in crate::scene) fn new(summary: RegionSummary, lease: Lease) -> Self {
        Self(Arc::new(RegionValue { summary, _lease: lease }))
    }
    pub fn summary(&self) -> &RegionSummary {
        &self.0.summary
    }
}

struct RegionValue {
    summary: RegionSummary,
    _lease: Lease,
}

#[derive(Clone)]
pub struct Patch(Arc<PatchValue>);

impl Patch {
    pub(in crate::scene) fn new(patch: TerrainPatch, lease: Lease) -> Self {
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

pub(in crate::scene) enum MeshChange {
    Set(Entity, MeshKey, PreparedMesh),
    Clear(Entity),
}

pub(in crate::scene) enum Change {
    ViewerEnvironment(Option<ViewerEnvironment>),
    Object(Object),
    Mesh(MeshChange),
    Material(MaterialChange),
    Remove(Entity),
    Region(RegionData),
    Environment(Region, Option<RegionEnvironment>),
    Retire(Region),
    Patch(Region, Patch),
    Water(Region, Option<f32>),
    Surface(Surface),
    RemoveSurface(Region, SurfaceKind),
}

pub struct Batch {
    pub(in crate::scene) session: SceneLifetime,
    pub(in crate::scene) sequence: u64,
    pub(in crate::scene) revision: Option<WorldRevision>,
    pub(in crate::scene) status: Status,
    pub(in crate::scene) replace: bool,
    pub(in crate::scene) changes: Vec<Change>,
    pub(in crate::scene) delivery: Delivery,
}

impl Batch {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn status(&self) -> Status {
        self.status
    }
}

#[derive(Default)]
pub(in crate::scene) struct Mirror {
    pub(in crate::scene) viewer_environment: Option<ViewerEnvironment>,
    pub(in crate::scene) objects: HashMap<Entity, Object>,
    pub(in crate::scene) regions: BTreeMap<Region, RegionData>,
    pub(in crate::scene) environments: BTreeMap<Region, RegionEnvironment>,
    pub(in crate::scene) patches: BTreeMap<(Region, TerrainPatchId), Patch>,
    pub(in crate::scene) water: BTreeMap<Region, Option<f32>>,
    pub(in crate::scene) surfaces: BTreeMap<(Region, SurfaceKind), Surface>,
}

impl Mirror {
    pub(in crate::scene) fn apply_mesh(&mut self, change: &MeshChange) {
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

    pub(in crate::scene) fn apply(&mut self, change: Change) {
        match change {
            Change::ViewerEnvironment(environment) => self.viewer_environment = environment,
            Change::Object(mut object) => {
                if let Some(old) = self.objects.get(&object.detail().summary.entity) {
                    object.inherit_animation_timing(old, std::time::Instant::now());
                    if old.geometry() == object.geometry() && object.preparation.is_none() {
                        object.preparation = old.preparation.clone();
                        object.mesh = old.mesh.clone();
                        if object.same_appearance(old) {
                            object.materials = old.materials.clone();
                        }
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
