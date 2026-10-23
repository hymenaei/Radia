use std::{
    collections::{BTreeMap, HashMap},
    ops::Bound::{Excluded, Unbounded},
};

use uuid::Uuid;
use world::Entity;

use crate::{
    Budget, Camera, Geometry, MeshError, MeshKey, Object, Placement, PreparedMesh,
    mesh_cache::{MeshCache, PrimCompletion, PrimJob},
    model::{MeshChange, Mirror},
    surfaces::{SurfaceCompletion, SurfaceJob},
};

pub struct MeshJob(pub(crate) Work);

impl MeshJob {
    pub fn run(self) -> MeshCompletion {
        MeshCompletion(match self.0 {
            Work::Prim(job) => Completion::Prim(job.run()),
            Work::Surface(job) => Completion::Surface(job.run()),
        })
    }
}

pub(crate) enum Work {
    Prim(PrimJob),
    Surface(Box<SurfaceJob>),
}

pub struct MeshCompletion(pub(crate) Completion);

pub(crate) enum Completion {
    Prim(PrimCompletion),
    Surface(SurfaceCompletion),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stamp {
    Pending,
    Ready,
    Failed(MeshError),
    Capacity,
}

struct MeshTarget {
    key: MeshKey,
    sent: Option<Stamp>,
    admitted: bool,
}

pub(crate) struct MeshPlanner {
    camera: Option<Camera>,
    meshes: MeshCache,
    targets: HashMap<Entity, MeshTarget>,
    dirty: bool,
    scanning: bool,
    cursor: Option<Uuid>,
}

impl MeshPlanner {
    pub(crate) fn new(budget: Budget) -> Self {
        Self { camera: None, meshes: MeshCache::new(budget), targets: HashMap::new(), dirty: false, scanning: false, cursor: None }
    }

    pub(crate) fn set_camera(&mut self, camera: Camera) {
        if self.camera != Some(camera) {
            self.camera = Some(camera);
            self.dirty = true;
        }
    }

    pub(crate) fn invalidate(&mut self) {
        self.dirty = true;
    }

    pub(crate) fn remove(&mut self, entity: Entity) {
        if let Some(target) = self.targets.remove(&entity)
            && target.admitted
        {
            self.meshes.release(target.key);
        }
    }

    pub(crate) fn reset(&mut self, budget: Budget) {
        let camera = self.camera;
        *self = Self::new(budget);
        self.camera = camera;
        self.dirty = camera.is_some();
    }

    pub(crate) fn complete(&mut self, completion: PrimCompletion) {
        self.meshes.complete(completion);
        self.dirty = true;
    }

    pub(crate) fn start(&mut self) -> Option<MeshJob> {
        self.meshes.start().map(|job| MeshJob(Work::Prim(job)))
    }

    pub(crate) fn has_work(&self) -> bool {
        self.camera.is_some() && (self.dirty || self.scanning)
    }

    pub(crate) fn waiting(&self) -> bool {
        self.meshes.waiting() || self.targets.values().any(|target| !target.admitted)
    }

    pub(crate) fn retry_admission(&mut self) -> bool {
        let deferred = self.targets.values().any(|target| !target.admitted);
        self.dirty |= deferred;
        deferred
    }

    pub(crate) fn step(&mut self, mirror: &Mirror, identities: &BTreeMap<Uuid, Entity>) -> Vec<MeshChange> {
        let Some(camera) = self.camera else {
            return Vec::new();
        };
        if !self.scanning {
            if !self.dirty {
                return Vec::new();
            }
            self.meshes.reset_priorities();
            self.scanning = true;
            self.dirty = false;
            self.cursor = None;
        }
        let mut changes = Vec::new();
        let mut entries = identities.range((self.cursor.map_or(Unbounded, Excluded), Unbounded));
        for _ in 0..128 {
            let Some((id, entity)) = entries.next() else {
                self.scanning = false;
                break;
            };
            self.cursor = Some(*id);
            let object = &mirror.objects[entity];
            let desired = self.desired(camera, object);
            if self.targets.get(entity).map(|t| t.key) != desired {
                self.remove(*entity);
                if let Some(key) = desired {
                    let admitted = self.meshes.request(key);
                    self.targets.insert(*entity, MeshTarget { key, sent: None, admitted });
                } else {
                    changes.push(MeshChange::Clear(*entity));
                }
            }
            if let Some(target) = self.targets.get_mut(entity) {
                if !target.admitted {
                    target.admitted = self.meshes.request(target.key);
                }
                if let Placement::Ready(transform) = object.placement() {
                    let priority = camera.importance(transform.pose.position, 3.0 * transform.scale.as_dvec3().abs().max_element());
                    self.meshes.prioritize(target.key, priority.unwrap_or(0));
                }
                let state = if target.admitted { self.meshes.get(target.key) } else { PreparedMesh::Capacity };
                let stamp = match &state {
                    PreparedMesh::Pending => Stamp::Pending,
                    PreparedMesh::Ready(_) => Stamp::Ready,
                    PreparedMesh::Failed(error) => Stamp::Failed(*error),
                    PreparedMesh::Capacity => Stamp::Capacity,
                };
                if target.sent != Some(stamp) {
                    changes.push(MeshChange::Set(*entity, target.key, state));
                    target.sent = Some(stamp);
                }
            }
        }
        changes
    }

    fn desired(&self, camera: Camera, object: &Object) -> Option<MeshKey> {
        let Geometry::Prim(shape) = object.geometry() else {
            return None;
        };
        let Placement::Ready(transform) = object.placement() else {
            return None;
        };
        let radius = 3.0 * transform.scale.as_dvec3().abs().max_element();
        let previous =
            self.targets.get(&object.detail().summary.entity).filter(|target| target.key.shape == shape).map(|target| target.key.detail);
        camera.detail(transform.pose.position, radius, previous).map(|detail| MeshKey { shape, detail })
    }
}
