use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use math::DVec3;
use world::{Region, TerrainPatchId};

use crate::{
    Budget, Mesh, MeshError, PreparedMesh, SharedMesh,
    budget::MeshWork,
    model::Mirror,
    terrain::{self, Terrain},
    water,
};

type Key = (SurfaceKind, Region);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SurfaceKind {
    Water,
    Ground(TerrainPatchId),
}

#[derive(Clone)]
pub struct Surface {
    pub region: Region,
    pub kind: SurfaceKind,
    pub position: DVec3,
    pub mesh: PreparedMesh,
}

struct Control {
    cancelled: AtomicBool,
    finished: AtomicBool,
}

enum Input {
    Ground(Terrain),
    Water(f32),
}

pub(crate) struct SurfaceJob {
    key: Key,
    position: DVec3,
    input: Input,
    flight: Flight,
}

impl SurfaceJob {
    pub(crate) fn run(self) -> SurfaceCompletion {
        let result = match &self.input {
            Input::Ground(tiles) => tiles.mesh(&self.flight.control.cancelled),
            Input::Water(height) => water::mesh(*height),
        };
        SurfaceCompletion { key: self.key, position: self.position, result, flight: self.flight }
    }
}

struct Flight {
    control: Arc<Control>,
    work: MeshWork,
}

impl Drop for Flight {
    fn drop(&mut self) {
        self.control.finished.store(true, Ordering::Release);
    }
}

pub(crate) struct SurfaceCompletion {
    key: Key,
    position: DVec3,
    result: Result<Mesh, MeshError>,
    flight: Flight,
}

pub(crate) enum SurfaceStep {
    Build(SurfaceJob),
    Remove(Region, SurfaceKind),
    Waiting,
    Idle,
}

pub(crate) struct SurfaceJobs {
    budget: Budget,
    dirty: BTreeSet<Key>,
    running: BTreeMap<Key, Arc<Control>>,
    hydrating: BTreeSet<Region>,
}

impl SurfaceJobs {
    pub(crate) fn new(budget: Budget) -> Self {
        Self { budget, dirty: BTreeSet::new(), running: BTreeMap::new(), hydrating: BTreeSet::new() }
    }

    pub(crate) fn hydrating(&mut self, region: Region, pending: bool) {
        if pending {
            self.hydrating.insert(region);
        } else {
            self.hydrating.remove(&region);
        }
    }

    fn invalidate(&mut self, key: Key) {
        if let Some(control) = self.running.remove(&key) {
            control.cancelled.store(true, Ordering::Release);
        }
        self.dirty.insert(key);
    }

    pub(crate) fn patch(&mut self, mirror: &Mirror, region: Region, patch: TerrainPatchId) {
        for (region, patch) in terrain::affected_patches(mirror, region, patch) {
            self.invalidate((SurfaceKind::Ground(patch), region));
        }
    }
    pub(crate) fn water(&mut self, region: Region) {
        self.invalidate((SurfaceKind::Water, region));
    }

    pub(crate) fn regions(&mut self, mirror: &Mirror) {
        for &(region, patch) in mirror.patches.keys() {
            self.invalidate((SurfaceKind::Ground(patch), region));
        }
        for (&region, height) in &mirror.water {
            if height.is_some() {
                self.water(region);
            }
        }
    }

    pub(crate) fn retire(&mut self, region: Region) {
        self.hydrating.remove(&region);
        self.dirty.retain(|key| key.1 != region);
        self.running.retain(|key, control| {
            if key.1 != region {
                return true;
            }
            control.cancelled.store(true, Ordering::Release);
            false
        });
    }

    pub(crate) fn start(&mut self, mirror: &Mirror) -> SurfaceStep {
        let finished: Vec<_> =
            self.running.iter().filter(|(_, control)| control.finished.load(Ordering::Acquire)).map(|(key, _)| *key).collect();
        for key in finished {
            self.invalidate(key);
        }
        let Some(&key) = self.dirty.first() else {
            return if self.running.is_empty() { SurfaceStep::Idle } else { SurfaceStep::Waiting };
        };
        let origin = mirror.regions.get(&key.1).and_then(|region| region.summary().info.resolved_origin());
        let Some(origin) = origin.filter(|origin| origin.is_finite()) else {
            self.dirty.remove(&key);
            return SurfaceStep::Remove(key.1, key.0);
        };
        let (position, input) = match key.0 {
            SurfaceKind::Ground(patch) => {
                let (x, y) = patch.coordinates();
                let tiles = Terrain::capture(mirror, key.1, patch);
                if self.hydrating.contains(&key.1) && !tiles.complete() {
                    return SurfaceStep::Waiting;
                }
                (origin + DVec3::new(f64::from(x) * 16.0, f64::from(y) * 16.0, 0.0), Input::Ground(tiles))
            }
            SurfaceKind::Water => {
                let Some(height) = mirror.water.get(&key.1).copied().flatten() else {
                    self.dirty.remove(&key);
                    return SurfaceStep::Remove(key.1, key.0);
                };
                (origin, Input::Water(height))
            }
        };
        let Some(work) = self.budget.mesh_work() else { return SurfaceStep::Waiting };
        let control = Arc::new(Control { cancelled: AtomicBool::new(false), finished: AtomicBool::new(false) });
        self.dirty.remove(&key);
        self.running.insert(key, control.clone());
        SurfaceStep::Build(SurfaceJob { key, position, input, flight: Flight { control, work } })
    }

    pub(crate) fn complete(&mut self, mut completion: SurfaceCompletion) -> Option<Surface> {
        let control = self.running.get(&completion.key)?;
        if !Arc::ptr_eq(control, &completion.flight.control) {
            return None;
        }
        self.running.remove(&completion.key);
        let mesh = match completion.result {
            Ok(mesh) => {
                let lease = completion.flight.work.lease.split(mesh.shared_bytes()).expect("terrain fits admitted work");
                PreparedMesh::Ready(SharedMesh::new(mesh, lease))
            }
            Err(MeshError::Cancelled) => {
                self.dirty.insert(completion.key);
                return None;
            }
            Err(error) => PreparedMesh::Failed(error),
        };
        Some(Surface { region: completion.key.1, kind: completion.key.0, position: completion.position, mesh })
    }
}

impl Drop for SurfaceJobs {
    fn drop(&mut self) {
        for control in self.running.values() {
            control.cancelled.store(true, Ordering::Release);
        }
    }
}
