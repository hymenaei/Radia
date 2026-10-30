mod terrain;
mod water;

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use math::DVec3;
use world::{Region, TerrainPatchId};

use assets::mesh::Mesh;

use self::terrain::Terrain;
pub(in crate::scene) use self::terrain::height_at as terrain_height_at;
use crate::scene::{Budget, MeshBuildError, PreparedMesh, SharedMesh, budget::MeshWork, mesh::retained_bytes, model::Mirror};

type Key = (SurfaceKind, Region);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub enum SurfaceKind {
    Water,
    Terrain(TerrainPatchId),
}

#[derive(Clone)]
pub struct Surface {
    pub region: Region,
    pub kind: SurfaceKind,
    pub position: DVec3,
    pub mesh: PreparedMesh,
    pub terrain_composition: Option<Arc<[f32]>>,
}

enum Input {
    Terrain(Terrain),
    Water(f32),
}

pub(in crate::scene) struct SurfaceJob {
    key: Key,
    position: DVec3,
    input: Input,
    work: MeshWork,
    cancelled: Arc<AtomicBool>,
}

impl SurfaceJob {
    pub(in crate::scene) fn run(self) -> SurfaceCompletion {
        let result = match &self.input {
            Input::Terrain(tiles) => {
                tiles.mesh(&self.cancelled).map(|(mesh, composition)| BuiltSurface { mesh, composition: Some(composition) })
            }
            Input::Water(height) => water::mesh(*height).map(|mesh| BuiltSurface { mesh, composition: None }),
        };
        SurfaceCompletion { key: self.key, position: self.position, result, work: self.work, cancelled: self.cancelled }
    }
}

pub(in crate::scene) struct SurfaceCompletion {
    key: Key,
    position: DVec3,
    result: Result<BuiltSurface, MeshBuildError>,
    work: MeshWork,
    cancelled: Arc<AtomicBool>,
}

struct BuiltSurface {
    mesh: Mesh,
    composition: Option<Vec<f32>>,
}

pub(in crate::scene) enum SurfaceStep {
    Build(SurfaceJob),
    Remove(Region, SurfaceKind),
    Continue,
    Waiting,
    Idle,
}

pub(in crate::scene) struct SurfaceJobs {
    budget: Budget,
    dirty: BTreeSet<Key>,
    deferred: BTreeSet<Key>,
    running: BTreeMap<Key, Weak<AtomicBool>>,
    hydrating: BTreeSet<Region>,
}

impl SurfaceJobs {
    pub(in crate::scene) fn new(budget: Budget) -> Self {
        Self { budget, dirty: BTreeSet::new(), deferred: BTreeSet::new(), running: BTreeMap::new(), hydrating: BTreeSet::new() }
    }

    pub(in crate::scene) fn hydrating(&mut self, region: Region, pending: bool) {
        if pending {
            self.hydrating.insert(region);
        } else {
            self.hydrating.remove(&region);
            self.deferred.retain(|key| {
                if key.1 != region {
                    return true;
                }
                self.dirty.insert(*key);
                false
            });
        }
    }

    fn invalidate(&mut self, key: Key) {
        self.deferred.remove(&key);
        if let Some(cancelled) = self.running.remove(&key).and_then(|cancelled| cancelled.upgrade()) {
            cancelled.store(true, Ordering::Release);
        }
        self.dirty.insert(key);
    }

    pub(in crate::scene) fn patch(&mut self, mirror: &Mirror, region: Region, patch: TerrainPatchId) {
        for (region, patch) in terrain::affected_patches(mirror, region, patch) {
            self.invalidate((SurfaceKind::Terrain(patch), region));
        }
    }
    pub(in crate::scene) fn water(&mut self, region: Region) {
        self.invalidate((SurfaceKind::Water, region));
    }

    pub(in crate::scene) fn terrain_settings(&mut self, mirror: &Mirror, region: Region) {
        for &(patch_region, patch) in mirror.patches.keys() {
            if patch_region == region {
                self.invalidate((SurfaceKind::Terrain(patch), region));
            }
        }
    }

    pub(in crate::scene) fn regions(&mut self, mirror: &Mirror) {
        for &(region, patch) in mirror.patches.keys() {
            self.invalidate((SurfaceKind::Terrain(patch), region));
        }
        for (&region, height) in &mirror.water {
            if height.is_some() {
                self.water(region);
            }
        }
    }

    pub(in crate::scene) fn retire(&mut self, region: Region) {
        self.hydrating.remove(&region);
        self.dirty.retain(|key| key.1 != region);
        self.deferred.retain(|key| key.1 != region);
        self.running.retain(|key, cancelled| {
            if key.1 != region {
                return true;
            }
            if let Some(cancelled) = cancelled.upgrade() {
                cancelled.store(true, Ordering::Release);
            }
            false
        });
    }

    pub(in crate::scene) fn start(&mut self, mirror: &Mirror) -> SurfaceStep {
        let (dirty, deferred, running) = (&mut self.dirty, &mut self.deferred, &mut self.running);
        running.retain(|key, cancelled| {
            if cancelled.upgrade().is_some() {
                return true;
            }
            deferred.remove(key);
            dirty.insert(*key);
            false
        });
        let Some(&key) = self.dirty.first() else {
            return if self.running.is_empty() && self.deferred.is_empty() { SurfaceStep::Idle } else { SurfaceStep::Waiting };
        };
        let origin = mirror.regions.get(&key.1).and_then(|region| region.summary().info.resolved_origin());
        let Some(origin) = origin.filter(|origin| origin.is_finite()) else {
            self.dirty.remove(&key);
            return SurfaceStep::Remove(key.1, key.0);
        };
        let (position, input) = match key.0 {
            SurfaceKind::Terrain(patch) => {
                let (x, y) = patch.coordinates();
                let tiles = Terrain::capture(mirror, key.1, patch, origin);
                if self.hydrating.contains(&key.1) && !tiles.complete() {
                    self.dirty.remove(&key);
                    self.deferred.insert(key);
                    return SurfaceStep::Continue;
                }
                (origin + DVec3::new(f64::from(x) * 16.0, f64::from(y) * 16.0, 0.0), Input::Terrain(tiles))
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
        let cancelled = Arc::new(AtomicBool::new(false));
        self.dirty.remove(&key);
        self.running.insert(key, Arc::downgrade(&cancelled));
        SurfaceStep::Build(SurfaceJob { key, position, input, work, cancelled })
    }

    pub(in crate::scene) fn complete(&mut self, mut completion: SurfaceCompletion) -> Option<Surface> {
        let cancelled = self.running.get(&completion.key)?;
        if !Weak::ptr_eq(cancelled, &Arc::downgrade(&completion.cancelled)) {
            return None;
        }
        self.running.remove(&completion.key);
        let (mesh, terrain_composition) = match completion.result {
            Ok(BuiltSurface { mesh, composition }) => {
                let composition_bytes = composition.as_ref().map_or(0, |values| std::mem::size_of_val(values.as_slice()));
                let retained = retained_bytes(&mesh).checked_add(composition_bytes).expect("terrain allocation size is representable");
                let lease = completion.work.lease.split(retained).expect("terrain fits admitted work");
                (PreparedMesh::Ready(SharedMesh::new(mesh, lease)), composition.map(Arc::from))
            }
            Err(MeshBuildError::Cancelled) => {
                self.dirty.insert(completion.key);
                return None;
            }
            Err(error) => (PreparedMesh::Failed(error), None),
        };
        Some(Surface { region: completion.key.1, kind: completion.key.0, position: completion.position, mesh, terrain_composition })
    }
}

impl Drop for SurfaceJobs {
    fn drop(&mut self) {
        for cancelled in self.running.values() {
            if let Some(cancelled) = cancelled.upgrade() {
                cancelled.store(true, Ordering::Release);
            }
        }
    }
}
