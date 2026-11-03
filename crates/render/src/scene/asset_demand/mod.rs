mod mesh;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    ops::Bound::{Excluded, Unbounded},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use assets::{Content, Failure, Kind, MaterialDescription, Priority, Request, Resource, Status, TextureRef, TextureSize, mesh::MeshLod};
use math::DVec3;
use utils::AssetId;
use uuid::Uuid;
use world::{Entity, FaceAppearance, Region};

use crate::material::{self, AssetState, FaceMaterial, Resolution};
use crate::scene::{Budget, Camera, MAX_OBJECTS, Object, Placement, budget::Lease, model::Mirror};

use self::mesh::{MeshAssets, MeshResolution};

pub(in crate::scene) use self::mesh::MeshPublication;

const MAX_TARGETS: usize = MAX_OBJECTS;
const MAX_FACES: usize = 32_768;
const METADATA_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AssetTicket(u64);

impl AssetTicket {
    pub(in crate::scene) fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1)).expect("asset tickets exhausted"))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AssetRequest {
    pub ticket: AssetTicket,
    pub region: Region,
    pub request: Request,
    pub priority: Priority,
    pub importance: u16,
}

#[derive(Debug)]
pub enum AssetChange {
    Request(AssetRequest),
    Release(AssetTicket),
    UpdateImportance { ticket: AssetTicket, importance: u16 },
    UpdatePriority { ticket: AssetTicket, priority: Priority },
}

#[derive(Clone)]
pub(in crate::scene) struct Materials(Arc<Retained>);

impl Materials {
    pub(in crate::scene) fn faces(&self) -> &[FaceMaterial] {
        &self.0.faces
    }
}

struct Retained {
    faces: Vec<FaceMaterial>,
    _lease: Lease,
}

#[derive(Clone)]
pub(in crate::scene) struct MaterialChange {
    pub(in crate::scene) source: Object,
    pub(in crate::scene) materials: Option<Materials>,
    pub(in crate::scene) mesh: MeshPublication,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct AssetKey {
    region: Region,
    resource: Resource,
    unit: AssetUnit,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum AssetUnit {
    Whole,
    MeshHeader,
    MeshLod(MeshLod),
}

struct Entry {
    ticket: AssetTicket,
    request: Request,
    users: HashMap<AssetUser, UserDemand>,
    size: TextureSize,
    priority: Priority,
    importance: u16,
    status: Option<Status>,
}

#[derive(Clone, Copy)]
struct UserDemand {
    size: TextureSize,
    priority: Priority,
    importance: u16,
}

struct ObjectDemand {
    keys: HashSet<AssetKey>,
    faces: usize,
    size: TextureSize,
    asset: Option<AssetId>,
}

struct RejectedObject {
    id: Uuid,
    entity: Entity,
    faces: usize,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::scene) enum AssetUser {
    Object(Entity),
    Terrain(Region),
}

struct TerrainDemand {
    textures: [Uuid; 4],
    keys: HashSet<AssetKey>,
    size: TextureSize,
    priority: Priority,
}

pub(in crate::scene) struct AssetDemand {
    budget: Budget,
    metadata: Option<Lease>,
    camera: Option<Camera>,
    scan: bool,
    rescan: bool,
    deferred: bool,
    cursor: Option<Uuid>,
    dirty: BTreeMap<Uuid, Entity>,
    entries: HashMap<AssetKey, Entry>,
    meshes: MeshAssets,
    tickets: HashMap<AssetTicket, AssetKey>,
    targets: HashMap<Entity, ObjectDemand>,
    rejected: VecDeque<RejectedObject>,
    rejected_entities: HashSet<Entity>,
    rejected_scan_remaining: usize,
    terrain_targets: HashMap<Region, TerrainDemand>,
    dirty_terrain: BTreeSet<Region>,
    terrain_changes: BTreeSet<Region>,
    terrain_rescan: bool,
    events: VecDeque<AssetChange>,
    faces: usize,
}

impl AssetDemand {
    pub(in crate::scene) fn new(budget: Budget) -> Self {
        Self {
            budget,
            metadata: None,
            camera: None,
            scan: false,
            rescan: false,
            deferred: false,
            cursor: None,
            dirty: BTreeMap::new(),
            entries: HashMap::new(),
            meshes: MeshAssets::default(),
            tickets: HashMap::new(),
            targets: HashMap::new(),
            rejected: VecDeque::new(),
            rejected_entities: HashSet::new(),
            rejected_scan_remaining: 0,
            terrain_targets: HashMap::new(),
            dirty_terrain: BTreeSet::new(),
            terrain_changes: BTreeSet::new(),
            terrain_rescan: false,
            events: VecDeque::new(),
            faces: 0,
        }
    }

    pub(in crate::scene) fn set_camera(&mut self, camera: Camera) {
        if self.camera != Some(camera) {
            self.camera = Some(camera);
            self.rescan = true;
            self.terrain_rescan = true;
        }
    }

    pub(in crate::scene) fn invalidate(&mut self, object: &Object) {
        if self.dirty.len() < crate::scene::MAX_OBJECTS {
            self.dirty.insert(object.detail().summary.id, object.detail().summary.entity);
        }
    }

    pub(in crate::scene) fn invalidate_region(&mut self, region: Region) {
        self.dirty_terrain.insert(region);
    }

    pub(in crate::scene) fn event(&mut self) -> Option<AssetChange> {
        self.events.pop_front()
    }

    pub(in crate::scene) fn take_terrain_changes(&mut self) -> Vec<Region> {
        std::mem::take(&mut self.terrain_changes).into_iter().collect()
    }

    pub(in crate::scene) fn terrain_textures(&self, region: Region) -> [Option<assets::TextureHandle>; 4] {
        let Some(target) = self.terrain_targets.get(&region) else { return [None, None, None, None] };
        std::array::from_fn(|index| {
            let key = AssetKey {
                region,
                resource: Resource { id: AssetId::new(target.textures[index]), kind: Kind::Texture },
                unit: AssetUnit::Whole,
            };
            self.entries.get(&key).and_then(|entry| entry.status.as_ref()).and_then(Status::content).and_then(|content| match content {
                Content::Texture(texture) => Some(texture.clone()),
                _ => None,
            })
        })
    }

    pub(in crate::scene) fn has_work(&self) -> bool {
        self.camera.is_some()
            && (self.scan
                || self.rescan
                || !self.dirty.is_empty()
                || !self.dirty_terrain.is_empty()
                || self.terrain_rescan
                || self.rejected_scan_remaining > 0)
    }

    pub(in crate::scene) fn retry_admission(&mut self) -> bool {
        if self.deferred {
            self.deferred = false;
            self.rescan = true;
            true
        } else {
            false
        }
    }

    pub(in crate::scene) fn complete(&mut self, ticket: AssetTicket, mut status: Status) -> Vec<AssetUser> {
        let Some(key) = self.tickets.get(&ticket).copied() else { return Vec::new() };
        let mut content_update = None;
        if let Status::Ready(content) | Status::Refining(content) | Status::RefinementFailed { content, .. } = &status {
            let valid = match (key.unit, key.resource.kind, content) {
                (AssetUnit::Whole, Kind::Texture, Content::Texture(texture)) => texture.asset() == key.resource.id,
                (AssetUnit::Whole, Kind::LegacyMaterial, Content::Material(material)) => {
                    matches!(**material, MaterialDescription::Legacy(_))
                }
                (AssetUnit::Whole, Kind::PbrMaterial, Content::Material(material)) => matches!(**material, MaterialDescription::Pbr(_)),
                (AssetUnit::MeshHeader, Kind::Mesh, Content::MeshHeader(_)) => true,
                (AssetUnit::MeshLod(lod), Kind::Mesh, Content::Mesh(mesh)) => {
                    mesh.lod() == lod
                        && match mesh.geometry() {
                            assets::mesh::LodGeometry::Geometry(mesh) => mesh.faces().len() <= 8,
                            assets::mesh::LodGeometry::Empty { face_count } => usize::from(*face_count) <= 8,
                        }
                }
                _ => false,
            };
            if !valid {
                status = Status::Failed { failure: Failure::Malformed, retry_at: None };
            } else {
                content_update = Some(content.clone());
            }
        }
        let users = {
            let Some(entry) = self.entries.get_mut(&key) else { return Vec::new() };
            entry.status = Some(status.retaining(entry.status.as_ref().and_then(Status::content)));
            entry.users.keys().copied().collect::<Vec<_>>()
        };
        self.terrain_changes.extend(users.iter().filter_map(|user| match user {
            AssetUser::Terrain(region) => Some(*region),
            AssetUser::Object(_) => None,
        }));
        let mesh_users = match (key.unit, content_update) {
            (AssetUnit::MeshHeader, Some(Content::MeshHeader(header))) => {
                if self.meshes.set_header(key.resource.id, header) {
                    self.meshes.users(key.resource.id).into_iter().map(AssetUser::Object).collect()
                } else {
                    Vec::new()
                }
            }
            (AssetUnit::MeshLod(_), Some(Content::Mesh(mesh))) => {
                let update = self.meshes.set_lod(key.resource.id, mesh, &self.budget);
                self.deferred |= update.deferred;
                if update.changed || update.deferred {
                    self.meshes.users(key.resource.id).into_iter().map(AssetUser::Object).collect()
                } else {
                    Vec::new()
                }
            }
            (AssetUnit::MeshHeader | AssetUnit::MeshLod(_), None) => {
                self.meshes.users(key.resource.id).into_iter().map(AssetUser::Object).collect()
            }
            _ => Vec::new(),
        };
        if matches!(key.unit, AssetUnit::MeshHeader | AssetUnit::MeshLod(_)) { mesh_users } else { users }
    }

    pub(in crate::scene) fn remove(&mut self, entity: Entity) {
        if let Some(target) = self.targets.remove(&entity) {
            self.rescan = true;
            self.faces -= target.faces;
            self.mark_rejected_for_retry();
            if let Some(asset) = target.asset {
                self.meshes.remove_user(asset, entity);
            }
            for key in target.keys {
                self.release(AssetUser::Object(entity), key);
            }
        }
    }

    pub(in crate::scene) fn remove_region(&mut self, region: Region) {
        self.dirty_terrain.remove(&region);
        self.terrain_changes.remove(&region);
        if let Some(target) = self.terrain_targets.remove(&region) {
            for key in target.keys {
                self.release(AssetUser::Terrain(region), key);
            }
        }
    }

    fn release(&mut self, user: AssetUser, key: AssetKey) {
        self.remove_user(user, key);
        if self.entries.get(&key).is_some_and(|entry| entry.users.is_empty()) {
            let entry = self.entries.remove(&key).expect("entry exists");
            if self.tickets.remove(&entry.ticket).is_some() {
                self.events.push_back(AssetChange::Release(entry.ticket));
            }
        }
        self.refresh(key);
    }

    fn remove_user(&mut self, user: AssetUser, key: AssetKey) {
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.users.remove(&user);
        }
    }

    fn queue_rejected(&mut self, id: Uuid, entity: Entity, faces: usize) {
        if self.rejected_entities.contains(&entity) || self.rejected.len() >= MAX_OBJECTS {
            return;
        }
        let rejected = RejectedObject { id, entity, faces };
        self.rejected_entities.insert(entity);
        self.rejected.push_back(rejected);
        if self.can_admit_rejected(self.rejected.back().expect("rejected object was inserted")) {
            self.rejected_scan_remaining = self.rejected.len();
        }
    }

    fn can_admit_rejected(&self, rejected: &RejectedObject) -> bool {
        self.targets.len() < MAX_TARGETS && self.faces.saturating_add(rejected.faces) <= MAX_FACES
    }

    fn mark_rejected_for_retry(&mut self) {
        self.rejected_scan_remaining = self.rejected.len();
    }

    fn refresh(&mut self, key: AssetKey) {
        let Some(entry) = self.entries.get_mut(&key) else { return };
        let Some(size) = entry.users.values().map(|demand| demand.size).max() else { return };
        let priority = entry.users.values().map(|demand| demand.priority).max().unwrap_or(Priority::Background);
        let importance = entry.users.values().map(|demand| demand.importance).max().unwrap_or_default();
        let size_changed = key.unit == AssetUnit::Whole && key.resource.kind == Kind::Texture && entry.size != size;
        let priority_changed = entry.priority != priority;
        let importance_changed = entry.importance != importance;
        if !size_changed && !priority_changed && !importance_changed {
            return;
        }
        if !size_changed && !priority_changed && importance_changed {
            entry.importance = importance;
            self.events.push_back(AssetChange::UpdateImportance { ticket: entry.ticket, importance });
            return;
        }
        if self.tickets.remove(&entry.ticket).is_some() {
            self.events.push_back(AssetChange::Release(entry.ticket));
        }
        entry.size = size;
        entry.priority = priority;
        entry.importance = importance;
        entry.request = request_with_size(entry.request, size);
        entry.ticket = AssetTicket::new();
        self.tickets.insert(entry.ticket, key);
        self.events.push_back(AssetChange::Request(AssetRequest {
            ticket: entry.ticket,
            region: key.region,
            request: entry.request,
            priority,
            importance,
        }));
    }

    fn resolve_terrain(&mut self, camera: Camera, mirror: &Mirror, region: Region) {
        let Some(data) = mirror.regions.get(&region) else {
            self.remove_region(region);
            return;
        };
        let Some(settings) = data.summary().terrain_settings else {
            self.remove_region(region);
            return;
        };
        let textures = *settings.detail_textures();
        let previous = self.terrain_targets.get(&region).map(|target| target.size);
        let (size, priority) = data.summary().info.resolved_origin().map_or((TextureSize::new(128), Priority::Background), |origin| {
            let center = origin + DVec3::new(128.0, 128.0, 0.0);
            let priority = if camera.importance(center, 181.0).is_some() { Priority::Visible } else { Priority::Background };
            let size = if priority == Priority::Visible {
                let position = camera.position();
                let closest =
                    DVec3::new(position.x.clamp(origin.x, origin.x + 256.0), position.y.clamp(origin.y, origin.y + 256.0), origin.z);
                camera.texture_size(closest, 16.0, previous)
            } else {
                TextureSize::new(128)
            };
            (size, priority)
        });
        if self
            .terrain_targets
            .get(&region)
            .is_some_and(|target| target.textures == textures && target.size == size && target.priority == priority)
        {
            return;
        }
        let user = AssetUser::Terrain(region);
        if let Some(target) = self.terrain_targets.get(&region) {
            for key in &target.keys {
                if let Some(entry) = self.entries.get_mut(key) {
                    entry.users.remove(&user);
                }
            }
        }
        let mut keys = HashSet::new();
        for id in textures {
            self.register(user, region, Request::Texture { id: AssetId::new(id), size }, size, priority, 0, &mut keys);
        }
        let old = self.terrain_targets.remove(&region);
        if let Some(old) = &old {
            for key in old.keys.difference(&keys) {
                self.release(user, *key);
            }
        }
        for key in &keys {
            self.refresh(*key);
        }
        let changed = old.as_ref().is_none_or(|old| old.textures != textures);
        self.terrain_targets.insert(region, TerrainDemand { textures, keys, size, priority });
        if changed {
            self.terrain_changes.insert(region);
        }
    }

    pub(in crate::scene) fn reset(&mut self) {
        for ticket in self.tickets.keys() {
            self.events.push_back(AssetChange::Release(*ticket));
        }
        self.entries.clear();
        self.meshes.clear();
        self.tickets.clear();
        self.targets.clear();
        self.rejected.clear();
        self.rejected_entities.clear();
        self.rejected_scan_remaining = 0;
        self.terrain_targets.clear();
        self.dirty_terrain.clear();
        self.terrain_changes.clear();
        self.terrain_rescan = false;
        self.dirty.clear();
        self.faces = 0;
        self.scan = self.camera.is_some();
        self.rescan = false;
        self.deferred = false;
        self.cursor = None;
    }

    pub(in crate::scene) fn step(&mut self, mirror: &Mirror, identities: &BTreeMap<Uuid, Entity>) -> Vec<MaterialChange> {
        let Some(camera) = self.camera else { return Vec::new() };
        if self.terrain_rescan {
            self.dirty_terrain.extend(mirror.regions.keys().copied());
            self.terrain_rescan = false;
        }
        if let Some(region) = self.dirty_terrain.pop_first() {
            self.resolve_terrain(camera, mirror, region);
        }
        if !self.scan && self.rescan {
            self.scan = true;
            self.rescan = false;
            self.cursor = None;
        }
        if self.metadata.is_none() {
            self.metadata = self.budget.reserve(METADATA_BYTES);
            if self.metadata.is_none() {
                self.deferred = true;
                self.scan = false;
                self.dirty.clear();
                return Vec::new();
            }
        }
        for _ in 0..self.rejected_scan_remaining.min(128) {
            let Some(rejected) = self.rejected.pop_front() else { break };
            self.rejected_scan_remaining = self.rejected_scan_remaining.saturating_sub(1);
            if self.can_admit_rejected(&rejected) {
                self.rejected_entities.remove(&rejected.entity);
                self.dirty.insert(rejected.id, rejected.entity);
            } else {
                self.rejected.push_back(rejected);
            }
        }
        let mut changes = Vec::new();
        let mut entries = identities.range((self.cursor.map_or(Unbounded, Excluded), Unbounded));
        for _ in 0..128 {
            let entity = if let Some((_, entity)) = self.dirty.pop_first() {
                entity
            } else if self.scan {
                let Some((id, entity)) = entries.next() else {
                    self.scan = false;
                    break;
                };
                self.cursor = Some(*id);
                *entity
            } else {
                break;
            };
            if let Some(object) = mirror.objects.get(&entity)
                && let Some(change) = self.resolve_object_materials(camera, object)
            {
                changes.push(change);
            }
        }
        changes
    }

    fn resolve_object_materials(&mut self, camera: Camera, object: &Object) -> Option<MaterialChange> {
        let entity = object.detail().summary.entity;
        let (previous_faces, previous_size, previous_asset, previous_keys) =
            self.targets.get(&entity).map_or((0, None, None, Vec::new()), |target| {
                (target.faces, Some(target.size), target.asset, target.keys.iter().copied().collect())
            });
        let mesh_asset = match object.geometry() {
            crate::scene::Geometry::Mesh(asset) => Some(asset),
            _ => None,
        };
        let (interested, importance) = match (mesh_asset, object.placement()) {
            (Some(_), Placement::Ready(transform)) => {
                camera.mesh_importance(transform.pose.position, transform.scale).map_or((false, 0), |importance| (true, importance))
            }
            (None, Placement::Ready(transform)) => {
                (camera.importance(transform.pose.position, 3.0 * transform.scale.as_dvec3().abs().max_element()).is_some(), 0)
            }
            _ => (false, 0),
        };
        let mut tracked_asset = previous_asset.filter(|asset| Some(*asset) == mesh_asset);
        if previous_asset != mesh_asset
            && let Some(asset) = previous_asset
        {
            self.meshes.remove_user(asset, entity);
        }
        if let Some(asset) = mesh_asset
            && tracked_asset.is_none()
        {
            if self.meshes.add_user(asset, entity) {
                tracked_asset = Some(asset);
            } else {
                self.deferred = true;
            }
        }
        let priority =
            if interested && (mesh_asset.is_some() || object.mesh_matches_geometry()) { Priority::Visible } else { Priority::Background };
        let region = object.detail().summary.address.region();
        let size = match object.placement() {
            _ if priority == Priority::Background => TextureSize::new(32),
            Placement::Ready(transform) => camera.texture_size(transform.pose.position, transform.scale.as_dvec3().length(), previous_size),
            _ => TextureSize::new(32),
        };
        let mut keys = HashSet::new();
        for key in previous_keys {
            self.remove_user(AssetUser::Object(entity), key);
        }
        let mesh_resolution = if let (Some(asset), Some(tracked_asset)) = (mesh_asset, tracked_asset) {
            Some(self.resolve_mesh(camera, object, asset, tracked_asset, size, priority, importance, &mut keys))
        } else {
            mesh_asset.map(|_| MeshResolution::Failed)
        };
        let publication = mesh::publication(object, mesh_asset, mesh_resolution);
        let slots = mesh::material_slots(object, mesh_asset.is_some(), publication.mesh.as_ref(), priority);
        let count = slots.len();
        if self.faces.saturating_sub(previous_faces).saturating_add(count) > MAX_FACES
            || (previous_faces == 0 && !self.targets.contains_key(&entity) && self.targets.len() == MAX_TARGETS)
        {
            for key in keys.drain() {
                self.release(AssetUser::Object(entity), key);
            }
            if tracked_asset != previous_asset
                && let Some(asset) = tracked_asset
            {
                self.meshes.remove_user(asset, entity);
            }
            self.remove(entity);
            self.queue_rejected(object.detail().summary.id, entity, count);
            return (object.materials().is_some() || object.mesh().is_some()).then(|| MaterialChange {
                source: object.clone(),
                materials: None,
                mesh: if mesh_asset.is_some() { MeshPublication::Clear } else { MeshPublication::Keep },
            });
        }
        let mut faces = Vec::with_capacity(count);
        for slot in slots {
            faces.push(self.face(object, region, slot, size, priority, &mut keys));
        }
        let old = self.targets.remove(&entity);
        self.faces = self.faces.saturating_sub(previous_faces).saturating_add(count);
        if count < previous_faces {
            self.mark_rejected_for_retry();
        }
        if let Some(old) = old {
            for key in old.keys.difference(&keys) {
                self.release(AssetUser::Object(entity), *key);
            }
        }
        for key in &keys {
            self.refresh(*key);
        }
        self.targets.insert(entity, ObjectDemand { keys, faces: count, size, asset: tracked_asset });
        let mesh_changed = !matches!(&publication.publication, MeshPublication::Keep);
        let materials_unchanged = object.materials().is_some_and(|old| old == faces) || object.materials().is_none() && faces.is_empty();
        if !mesh_changed && materials_unchanged {
            return None;
        }
        let materials = if faces.is_empty() {
            None
        } else {
            let bytes = size_of::<Retained>() + faces.capacity() * size_of::<FaceMaterial>() + 32;
            let materials = self.budget.reserve(bytes).map(|lease| Materials(Arc::new(Retained { faces, _lease: lease })));
            self.deferred |= materials.is_none();
            materials
        };
        Some(MaterialChange { source: object.clone(), materials, mesh: publication.publication })
    }

    fn resolve_mesh(
        &mut self,
        camera: Camera,
        object: &Object,
        asset: AssetId,
        tracked_asset: AssetId,
        size: TextureSize,
        priority: Priority,
        importance: u16,
        keys: &mut HashSet<AssetKey>,
    ) -> MeshResolution {
        if asset != tracked_asset {
            return MeshResolution::Failed;
        }
        let current = object.mesh().and_then(|(key, _)| match key {
            crate::scene::DrawnMeshKey::Asset { asset: current, lod } if current == asset => Some(lod),
            _ => None,
        });
        let Placement::Ready(transform) = object.placement() else {
            self.meshes.set_user_lods(asset, object.detail().summary.entity, [None; 2], current);
            return MeshResolution::Waiting;
        };
        let Some(desired) = camera.mesh_lod(transform.pose.position, transform.scale) else {
            self.meshes.set_user_lods(asset, object.detail().summary.entity, [None; 2], current);
            return MeshResolution::Waiting;
        };
        let region = object.detail().summary.address.region();
        self.deferred |= self.meshes.retry_pending(asset, &self.budget);
        let mut plan = self.meshes.request_plan(asset, desired, current, priority);
        if plan.needs_header {
            let (resolution, content) =
                self.observe(object.detail().summary.entity, region, Request::MeshHeader(asset), size, priority, importance, keys);
            if let Some(Content::MeshHeader(header)) = content {
                self.meshes.set_header(asset, header);
            } else if matches!(resolution, Resolution::Failed(_)) {
                self.meshes.set_user_lods(asset, object.detail().summary.entity, [None; 2], None);
                return MeshResolution::Failed;
            }
            plan = self.meshes.request_plan(asset, desired, current, priority);
            if plan.needs_header {
                self.meshes.set_user_lods(asset, object.detail().summary.entity, plan.retained, current);
                return MeshResolution::Waiting;
            }
        }
        let mut failed = false;
        for request in plan.requests {
            let (resolution, content) = self.observe(object.detail().summary.entity, region, request, size, priority, importance, keys);
            failed |= matches!(resolution, Resolution::Failed(_));
            if let Some(Content::Mesh(content)) = content {
                let update = self.meshes.set_lod(asset, content, &self.budget);
                self.deferred |= update.deferred;
            }
        }
        let resolution = match self.meshes.resolve(asset, desired, current) {
            MeshResolution::Waiting if failed => MeshResolution::Failed,
            resolution => resolution,
        };
        let published = match &resolution {
            MeshResolution::Ready(crate::scene::DrawnMeshKey::Asset { asset: published, lod }, _) if *published == asset => Some(*lod),
            MeshResolution::Waiting => current,
            _ => None,
        };
        self.meshes.set_user_lods(asset, object.detail().summary.entity, plan.retained, published);
        resolution
    }

    fn observe(
        &mut self,
        entity: Entity,
        region: Region,
        request: Request,
        size: TextureSize,
        priority: Priority,
        importance: u16,
        keys: &mut HashSet<AssetKey>,
    ) -> (Resolution, Option<Content>) {
        let key = self.register(AssetUser::Object(entity), region, request, size, priority, importance, keys);
        let entry = self.entries.get(&key).expect("registered asset entry");
        match &entry.status {
            Some(Status::Ready(content)) => (Resolution::Ready, Some(content.clone())),
            Some(Status::Refining(content)) => (Resolution::Pending, Some(content.clone())),
            Some(Status::RefinementFailed { content, failure }) => (Resolution::Failed(*failure), Some(content.clone())),
            Some(Status::Failed { failure, .. }) => (Resolution::Failed(*failure), None),
            Some(Status::StaleSession) => (Resolution::Failed(Failure::SessionClosed), None),
            _ => (Resolution::Pending, None),
        }
    }

    fn register(
        &mut self,
        user: AssetUser,
        region: Region,
        request: Request,
        size: TextureSize,
        priority: Priority,
        importance: u16,
        keys: &mut HashSet<AssetKey>,
    ) -> AssetKey {
        let key = asset_key(region, request);
        keys.insert(key);
        let entry = self.entries.entry(key).or_insert_with(|| {
            let ticket = AssetTicket::new();
            self.tickets.insert(ticket, key);
            let request = request_with_size(request, size);
            self.events.push_back(AssetChange::Request(AssetRequest { ticket, region, request, priority, importance }));
            Entry { ticket, request, users: HashMap::new(), size, priority, importance, status: None }
        });
        entry
            .users
            .entry(user)
            .and_modify(|old| {
                old.size = old.size.max(size);
                old.priority = old.priority.max(priority);
                old.importance = old.importance.max(importance);
            })
            .or_insert(UserDemand { size, priority, importance });
        key
    }

    fn face(
        &mut self,
        object: &Object,
        region: Region,
        slot: u8,
        size: TextureSize,
        priority: Priority,
        keys: &mut HashSet<AssetKey>,
    ) -> FaceMaterial {
        let source = object.detail();
        let entity = source.summary.entity;
        let authored = source
            .surface_appearance
            .as_ref()
            .and_then(|appearance| appearance.face(slot))
            .copied()
            .unwrap_or_else(FaceAppearance::default);
        let (legacy_state, legacy) = if let Some(id) = authored.material {
            let (resolution, content) = self.observe(entity, region, Request::LegacyMaterial(id), size, priority, 0, keys);
            let material = match content {
                Some(Content::Material(material)) => match *material {
                    MaterialDescription::Legacy(material) => Some(material),
                    _ => None,
                },
                _ => None,
            };
            (Some(AssetState { asset: id, resolution }), material)
        } else {
            (None, None)
        };
        let (pbr_state, pbr) = if let Some(id) = source.pbr_materials.as_ref().and_then(|materials| materials.material(slot)) {
            let (resolution, content) = self.observe(entity, region, Request::PbrMaterial(id), size, priority, 0, keys);
            let material = match content {
                Some(Content::Material(material)) => match *material {
                    MaterialDescription::Pbr(material) => Some(material),
                    _ => None,
                },
                _ => None,
            };
            (Some(AssetState { asset: id, resolution }), material)
        } else {
            (None, None)
        };
        let overrides = source.pbr_material_overrides.as_ref().and_then(|overrides| overrides.face(slot));
        let (material, alpha) = material::resolve(authored, legacy, pbr, overrides);
        let mut face = FaceMaterial { slot, material, legacy_state, pbr_state, alpha, base_color_texture: None };
        if let TextureRef::Asset(id) = material.base_color_texture().source {
            let scale = face.material.base_color_texture().transform.scale;
            let repeats = f64::from(scale[0].abs().min(scale[1].abs())).max(1.0 / 64.0);
            let size = if priority == Priority::Background {
                TextureSize::new(32)
            } else {
                TextureSize::new((f64::from(size.edge()) / repeats).ceil() as u32)
            };
            let (resolution, content) = self.observe(entity, region, Request::Texture { id, size }, size, priority, 0, keys);
            let image = match content {
                Some(Content::Texture(image)) => Some(image),
                _ => None,
            };
            face.resolve_base_color(AssetState { asset: id, resolution }, image);
        }
        face
    }
}

fn asset_key(region: Region, request: Request) -> AssetKey {
    let unit = match request {
        Request::MeshHeader(_) => AssetUnit::MeshHeader,
        Request::MeshLod(lod) => AssetUnit::MeshLod(lod.lod()),
        _ => AssetUnit::Whole,
    };
    AssetKey { region, resource: request.resource(), unit }
}

fn request_with_size(request: Request, size: TextureSize) -> Request {
    match request {
        Request::Texture { id, .. } => Request::Texture { id, size },
        request => request,
    }
}

#[cfg(test)]
mod tests;
