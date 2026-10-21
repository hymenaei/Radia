use std::{
    collections::{BTreeMap, HashMap},
    error::Error,
    fmt::{self, Display, Formatter},
    mem::take,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
};

use world::{
    Entity, EnvironmentRead, Kind, ObjectDetail, PrimShape, Region, RegionSummary, Shape, TerrainPatch, TerrainPatchId, WorldRevision,
    WorldTransform, WorldTransformState,
};

use crate::{
    FaceMaterial, MeshKey, PreparedMesh, SharedMesh, Surface, SurfaceKind, ViewerEnvironment,
    asset_demand::{MaterialChange, Materials},
    budget::Lease,
};

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
pub struct Object {
    data: Arc<ObjectData>,
    preparation: Option<(MeshKey, PreparedMesh)>,
    mesh: Option<(MeshKey, SharedMesh)>,
    materials: Option<Materials>,
}

struct ObjectData {
    source: ObjectDetail,
    _lease: Lease,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HiddenReason {
    Avatar,
    UnresolvedPose,
    InvalidDimensions,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    Ready(WorldTransform),
    Hidden(HiddenReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Geometry {
    Prim(PrimShape),
    Unsupported(Shape),
    Missing,
}

impl Object {
    pub(crate) fn new(source: ObjectDetail, lease: Lease) -> Self {
        Self { data: Arc::new(ObjectData { source, _lease: lease }), preparation: None, mesh: None, materials: None }
    }

    pub fn detail(&self) -> &ObjectDetail {
        &self.data.source
    }

    pub fn mesh(&self) -> Option<(MeshKey, &SharedMesh)> {
        self.mesh.as_ref().map(|(key, mesh)| (*key, mesh))
    }

    pub fn materials(&self) -> Option<&[FaceMaterial]> {
        self.materials.as_ref().map(Materials::faces)
    }

    fn same_appearance(&self, other: &Self) -> bool {
        let a = self.detail();
        let b = other.detail();
        a.summary.address.region() == b.summary.address.region()
            && self.geometry() == other.geometry()
            && a.surface_appearance == b.surface_appearance
            && a.render_materials == b.render_materials
            && a.gltf_material_overrides == b.gltf_material_overrides
    }

    pub fn mesh_state(&self) -> Option<(MeshKey, &PreparedMesh)> {
        self.preparation.as_ref().map(|(key, state)| (*key, state))
    }

    pub fn placement(&self) -> Placement {
        let detail = self.detail();
        if detail.summary.kind == Kind::Avatar {
            return Placement::Hidden(HiddenReason::Avatar);
        }
        let Ok(WorldTransformState::Resolved(transform)) = detail.world_transform else {
            return Placement::Hidden(HiddenReason::UnresolvedPose);
        };
        if !transform.scale.is_finite() || transform.scale.abs().min_element() == 0.0 {
            return Placement::Hidden(HiddenReason::InvalidDimensions);
        }
        Placement::Ready(transform)
    }

    pub fn geometry(&self) -> Geometry {
        match self.detail().shape {
            Some(Shape::Prim(shape)) => Geometry::Prim(shape),
            Some(shape) => Geometry::Unsupported(shape),
            None => Geometry::Missing,
        }
    }
}

#[derive(Clone)]
pub struct RegionData(Arc<RegionValue>);

#[derive(Clone)]
pub(crate) struct EnvironmentData(Arc<EnvironmentValue>);

struct EnvironmentValue {
    source: EnvironmentRead,
    _lease: Lease,
}

impl EnvironmentData {
    pub(crate) fn new(source: EnvironmentRead, lease: Lease) -> Self {
        Self(Arc::new(EnvironmentValue { source, _lease: lease }))
    }

    pub(crate) fn source(&self) -> &EnvironmentRead {
        &self.0.source
    }
}

struct RegionValue {
    summary: RegionSummary,
    _lease: Lease,
}

impl RegionData {
    pub(crate) fn new(summary: RegionSummary, lease: Lease) -> Self {
        Self(Arc::new(RegionValue { summary, _lease: lease }))
    }
    pub fn summary(&self) -> &RegionSummary {
        &self.0.summary
    }
}

#[derive(Clone)]
pub struct Patch(Arc<PatchValue>);

struct PatchValue {
    patch: TerrainPatch,
    _lease: Lease,
}

impl Patch {
    pub(crate) fn new(patch: TerrainPatch, lease: Lease) -> Self {
        Self(Arc::new(PatchValue { patch, _lease: lease }))
    }
    pub fn source(&self) -> &TerrainPatch {
        &self.0.patch
    }
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
}

impl SceneView {
    pub fn viewer_environment(&self) -> Option<&ViewerEnvironment> {
        self.session.is_active().then_some(self.mirror.viewer_environment.as_ref()).flatten()
    }

    pub(crate) fn new(session: SceneLifetime) -> Self {
        Self { session, sequence: 0, revision: None, status: Status::Loading, mirror: Mirror::default() }
    }

    pub fn apply(&mut self, mut batch: Batch) -> Result<(), ApplyError> {
        if !self.session.is_active() || !self.session.same(&batch.session) {
            return Err(ApplyError::StaleSession);
        }
        if self.sequence.checked_add(1) != Some(batch.sequence) {
            return Err(ApplyError::OutOfOrder);
        }
        if batch.replace {
            self.mirror = Mirror::default();
        }
        for change in take(&mut batch.changes) {
            self.mirror.apply(change);
        }
        self.sequence = batch.sequence;
        self.revision = batch.revision;
        self.status = batch.status;
        batch.receipt.store(1, Ordering::Release);
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
    pub fn regions(&self) -> impl Iterator<Item = &RegionData> {
        self.mirror.regions.values().filter(|_| self.session.is_active())
    }

    pub fn environment(&self, region: Region) -> Option<&EnvironmentRead> {
        if self.session.is_active() { self.mirror.environments.get(&region).map(EnvironmentData::source) } else { None }
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
