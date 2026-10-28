use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ops::Bound::{Excluded, Unbounded},
};

use uuid::Uuid;
use world::Entity;

use crate::scene::{
    Budget, Camera, Detail, Geometry, MeshBuildError, MeshKey, Object, Placement, PreparedMesh,
    mesh_cache::{MeshCache, PrimCompletion, PrimJob},
    model::{MeshChange, Mirror},
    region_environment::{EnvironmentDecodeCompletion, EnvironmentDecodeJob},
    surface::{SurfaceCompletion, SurfaceJob},
};

pub struct MeshJob(pub(in crate::scene) Work);

impl MeshJob {
    pub fn run(self) -> MeshCompletion {
        MeshCompletion(match self.0 {
            Work::Prim(job) => Completion::Prim(job.run()),
            Work::Surface(job) => Completion::Surface(job.run()),
            Work::Environment(job) => Completion::Environment(job.run()),
        })
    }
}

pub(in crate::scene) enum Work {
    Prim(PrimJob),
    Surface(Box<SurfaceJob>),
    Environment(EnvironmentDecodeJob),
}

pub struct MeshCompletion(pub(in crate::scene) Completion);

pub(in crate::scene) enum Completion {
    Prim(PrimCompletion),
    Surface(SurfaceCompletion),
    Environment(EnvironmentDecodeCompletion),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stamp {
    Pending,
    Ready,
    Failed(MeshBuildError),
    Capacity,
}

struct MeshTarget {
    key: MeshKey,
    sent: Option<Stamp>,
    admitted: bool,
    priority: u32,
}

pub(in crate::scene) struct MeshPlanner {
    camera: Option<Camera>,
    meshes: MeshCache,
    targets: HashMap<Entity, MeshTarget>,
    target_users: HashMap<MeshKey, HashSet<Entity>>,
    dirty: bool,
    scanning: bool,
    cursor: Option<Uuid>,
}

impl MeshPlanner {
    pub(in crate::scene) fn new(budget: Budget) -> Self {
        Self {
            camera: None,
            meshes: MeshCache::new(budget),
            targets: HashMap::new(),
            target_users: HashMap::new(),
            dirty: false,
            scanning: false,
            cursor: None,
        }
    }

    pub(in crate::scene) fn set_camera(&mut self, camera: Camera) {
        if self.camera != Some(camera) {
            self.camera = Some(camera);
            self.dirty = true;
        }
    }

    pub(in crate::scene) fn invalidate(&mut self) {
        self.dirty = true;
    }

    pub(in crate::scene) fn remove(&mut self, entity: Entity) {
        if self.detach_target(entity).is_some() {
            self.dirty = true;
        }
    }

    pub(in crate::scene) fn reset(&mut self, budget: Budget) {
        let camera = self.camera;
        *self = Self::new(budget);
        self.camera = camera;
        self.dirty = camera.is_some();
    }

    pub(in crate::scene) fn complete(&mut self, completion: PrimCompletion) {
        self.meshes.complete(completion);
    }

    pub(in crate::scene) fn start(&mut self) -> Option<MeshJob> {
        self.meshes.start().map(|job| MeshJob(Work::Prim(job)))
    }

    pub(in crate::scene) fn has_work(&self) -> bool {
        self.camera.is_some() && (self.dirty || self.scanning || self.meshes.has_updates())
    }

    pub(in crate::scene) fn waiting(&self) -> bool {
        self.meshes.waiting()
    }

    pub(in crate::scene) fn step(&mut self, mirror: &Mirror, identities: &BTreeMap<Uuid, Entity>) -> Vec<MeshChange> {
        let Some(camera) = self.camera else {
            return Vec::new();
        };
        if !self.scanning {
            if !self.dirty {
                let mut changes = Vec::new();
                self.publish_cache_updates(&mut changes);
                return changes;
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
                self.detach_target(*entity);
                if let Some(key) = desired {
                    let priority = object_priority(camera, object);
                    let admitted = self.meshes.request(key, priority);
                    self.targets.insert(*entity, MeshTarget { key, sent: None, admitted, priority });
                    self.target_users.entry(key).or_default().insert(*entity);
                } else {
                    changes.push(MeshChange::Clear(*entity));
                }
            }
            if let Some(target) = self.targets.get_mut(entity) {
                target.priority = object_priority(camera, object);
                self.meshes.prioritize(target.key, target.priority);
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
        self.publish_cache_updates(&mut changes);
        changes
    }

    fn detach_target(&mut self, entity: Entity) -> Option<MeshTarget> {
        let target = self.targets.remove(&entity)?;
        if target.admitted {
            self.meshes.release(target.key);
        }
        if let Some(users) = self.target_users.get_mut(&target.key) {
            users.remove(&entity);
            if users.is_empty() {
                self.target_users.remove(&target.key);
            }
        }
        Some(target)
    }

    fn publish_cache_updates(&mut self, changes: &mut Vec<MeshChange>) {
        for key in self.meshes.take_updated() {
            let entities = self.target_users.get(&key).map(|users| users.iter().copied().collect::<Vec<_>>()).unwrap_or_default();
            let state = self.meshes.get(key);
            let stamp = mesh_stamp(&state);
            for entity in entities {
                let Some(target) = self.targets.get_mut(&entity) else { continue };
                if target.key == key && target.sent != Some(stamp) {
                    changes.push(MeshChange::Set(entity, key, state.clone()));
                    target.sent = Some(stamp);
                }
            }
        }
        let mut evicted = self.meshes.take_evicted();
        while let Some(key) = evicted.pop() {
            let entities = self.target_users.get(&key).map(|users| users.iter().copied().collect::<Vec<_>>()).unwrap_or_default();
            for entity in entities {
                let Some(target) = self.targets.get(&entity) else { continue };
                if target.key != key {
                    continue;
                }
                let fallback = lower_detail(key.detail).map(|detail| MeshKey { shape: key.shape, detail });
                let priority = target.priority;
                if let Some(fallback) = fallback {
                    self.detach_target(entity);
                    let admitted = self.meshes.request(fallback, priority);
                    let mut target = MeshTarget { key: fallback, sent: None, admitted, priority };
                    let state = if admitted { self.meshes.get(fallback) } else { PreparedMesh::Capacity };
                    target.sent = Some(mesh_stamp(&state));
                    changes.push(MeshChange::Set(entity, fallback, state));
                    self.targets.insert(entity, target);
                    self.target_users.entry(fallback).or_default().insert(entity);
                    evicted.extend(self.meshes.take_evicted());
                } else {
                    let state = self.meshes.get(key);
                    let stamp = mesh_stamp(&state);
                    if target.sent != Some(stamp) {
                        changes.push(MeshChange::Set(entity, key, state));
                        self.targets.get_mut(&entity).expect("evicted target remains cached").sent = Some(stamp);
                    }
                }
            }
        }
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

fn lower_detail(detail: Detail) -> Option<Detail> {
    match detail {
        Detail::Lowest => None,
        Detail::Low => Some(Detail::Lowest),
        Detail::Medium => Some(Detail::Low),
        Detail::High => Some(Detail::Medium),
    }
}

fn mesh_stamp(mesh: &PreparedMesh) -> Stamp {
    match mesh {
        PreparedMesh::Pending => Stamp::Pending,
        PreparedMesh::Ready(_) => Stamp::Ready,
        PreparedMesh::Failed(error) => Stamp::Failed(*error),
        PreparedMesh::Capacity => Stamp::Capacity,
    }
}

fn object_priority(camera: Camera, object: &Object) -> u32 {
    let Placement::Ready(transform) = object.placement() else { return 0 };
    camera.importance(transform.pose.position, 3.0 * transform.scale.as_dvec3().abs().max_element()).unwrap_or(0)
}
