use std::{
    collections::{HashMap, VecDeque},
    error::Error,
    fmt::{self, Display, Formatter},
    mem::size_of,
    time::Instant,
};

use world::{Entity, Region, TERRAIN_PATCH_SIZE, TERRAIN_REGION_SIZE, TerrainPatchId, WorldRevision};

use super::{Batch, Change, MeshChange, Mirror, SceneLifetime, Status};
use crate::scene::motion::MotionCircuit;
use crate::scene::{Object, Patch, RegionData, Surface, ViewerEnvironment, region_environment::RegionEnvironment};

const CHANGE_HISTORY: usize = 64;
const CHANGE_HISTORY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct SceneChanges {
    pub(crate) objects: Vec<Entity>,
    pub(crate) surfaces: Vec<(Region, crate::scene::SurfaceKind)>,
    pub(crate) regions: Vec<Region>,
    pub(crate) retired: Vec<Region>,
    pub(crate) full: bool,
    pub(crate) reconcile_all: bool,
}

impl SceneChanges {
    fn from_batch(batch: &Batch) -> Self {
        let mut changes = Self::default();
        if batch.replace {
            changes.full = true;
            return changes;
        }
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

    fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.objects.capacity() * size_of::<Entity>()
            + self.surfaces.capacity() * size_of::<(Region, crate::scene::SurfaceKind)>()
            + self.regions.capacity() * size_of::<Region>()
            + self.retired.capacity() * size_of::<Region>()
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
    change_history_bytes: usize,
    reconcile_marker: SceneChanges,
    replacement_marker: SceneChanges,
    replacement_sequence: Option<u64>,
    motion_circuits: HashMap<Region, MotionCircuit>,
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
            change_history_bytes: 0,
            reconcile_marker: SceneChanges { reconcile_all: true, ..SceneChanges::default() },
            replacement_marker: SceneChanges { full: true, ..SceneChanges::default() },
            replacement_sequence: None,
            motion_circuits: HashMap::new(),
        }
    }

    pub fn apply(&mut self, batch: Batch) -> Result<(), ApplyError> {
        if !self.session.is_active() || !self.session.same(&batch.session) {
            return Err(ApplyError::StaleSession);
        }
        if self.sequence.checked_add(1) != Some(batch.sequence) {
            return Err(ApplyError::OutOfOrder);
        }
        let mut changes = SceneChanges::from_batch(&batch);
        if changes.retained_bytes() > CHANGE_HISTORY_BYTES {
            changes = SceneChanges { reconcile_all: true, ..SceneChanges::default() };
        }
        if batch.replace {
            self.mirror = Mirror::default();
        }
        let now = Instant::now();
        for change in batch.changes {
            self.mirror.apply_at(change, now);
        }
        self.sequence = batch.sequence;
        if batch.replace {
            self.replacement_sequence = Some(self.sequence);
        }
        self.revision = batch.revision;
        self.status = batch.status;
        self.change_history_bytes += changes.retained_bytes();
        self.change_history.push_back((self.sequence, changes));
        while self.change_history.len() > CHANGE_HISTORY || self.change_history_bytes > CHANGE_HISTORY_BYTES {
            if let Some((_, removed)) = self.change_history.pop_front() {
                self.change_history_bytes -= removed.retained_bytes();
            }
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
    pub fn set_motion_circuits(&mut self, circuits: &[MotionCircuit]) {
        self.motion_circuits.clear();
        self.motion_circuits.extend(circuits.iter().copied().map(|circuit| (circuit.region, circuit)));
    }

    pub(crate) fn motion_circuit(&self, region: Region) -> Option<MotionCircuit> {
        self.motion_circuits.get(&region).copied()
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

    pub fn terrain_height_at(&self, position: math::DVec3) -> Option<f64> {
        if !self.session.is_active() || !position.is_finite() {
            return None;
        }
        let (region, origin) = self.mirror.regions.values().find_map(|region| {
            let origin = region.summary().info.resolved_origin()?;
            (position.x >= origin.x
                && position.x < origin.x + TERRAIN_REGION_SIZE as f64
                && position.y >= origin.y
                && position.y < origin.y + TERRAIN_REGION_SIZE as f64)
                .then_some((region.summary().region, origin))
        })?;
        let local_x = position.x - origin.x;
        let local_y = position.y - origin.y;
        let patch_x = (local_x / TERRAIN_PATCH_SIZE as f64).floor() as u8;
        let patch_y = (local_y / TERRAIN_PATCH_SIZE as f64).floor() as u8;
        let patch_id = TerrainPatchId::new(patch_x, patch_y)?;
        let patch = self.mirror.patches.get(&(region, patch_id))?;
        let x = (local_x - f64::from(patch_x) * TERRAIN_PATCH_SIZE as f64).clamp(0.0, (TERRAIN_PATCH_SIZE - 1) as f64);
        let y = (local_y - f64::from(patch_y) * TERRAIN_PATCH_SIZE as f64).clamp(0.0, (TERRAIN_PATCH_SIZE - 1) as f64);
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = (x0 + 1).min(TERRAIN_PATCH_SIZE - 1);
        let y1 = (y0 + 1).min(TERRAIN_PATCH_SIZE - 1);
        let tx = x - x0 as f64;
        let ty = y - y0 as f64;
        let heights = patch.source().heights();
        let height = |x: usize, y: usize| f64::from(heights[y * TERRAIN_PATCH_SIZE + x]);
        let low = height(x0, y0) * (1.0 - tx) + height(x1, y0) * tx;
        let high = height(x0, y1) * (1.0 - tx) + height(x1, y1) * tx;
        Some(origin.z + low * (1.0 - ty) + high * ty)
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
        if !self.session.same(&version.session) || version.sequence > self.sequence {
            return None;
        }
        let next = version.sequence.checked_add(1)?;
        let replaced = self.replacement_sequence.is_some_and(|sequence| version.sequence < sequence);
        let missed = !replaced && next <= self.sequence && self.change_history.front().is_none_or(|(sequence, _)| *sequence > next);
        let status_changed = version.status != self.status();
        let sequence = version.sequence;
        let marker = if replaced { Some(&self.replacement_marker) } else { (missed || status_changed).then_some(&self.reconcile_marker) };
        let history = self.change_history.iter().filter(move |(current, _)| *current > sequence).map(|(_, changes)| changes);
        Some(marker.into_iter().chain(history))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Reconciler;

    #[test]
    fn status_change_requests_reconciliation_without_full_rebuild() {
        let (_scene, mut view) = Reconciler::new(crate::scene::Budget::default());
        let version = view.version();
        view.status = Status::Ready;

        let changes = view.changes_since(&version).unwrap().collect::<Vec<_>>();

        assert_eq!(changes.len(), 1);
        assert!(changes[0].reconcile_all);
        assert!(!changes[0].full);
    }
}
