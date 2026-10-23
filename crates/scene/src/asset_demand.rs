use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    ops::Bound::{Excluded, Unbounded},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use assets::{Content, Failure, Kind, MaterialDescription, Priority, Request, Resource, Status, TextureRef, TextureSize};
use utils::AssetId;
use uuid::Uuid;
use world::{Entity, FaceAppearance, MAX_SURFACE_FACES, Region};

use crate::{AssetState, Budget, Camera, FaceMaterial, Object, Placement, Resolution, appearance, budget::Lease, model::Mirror};

const MAX_TARGETS: usize = crate::MAX_OBJECTS;
const MAX_FACES: usize = 32_768;
const METADATA_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AssetTicket(u64);

impl AssetTicket {
    pub(crate) fn new() -> Self {
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
}

pub enum AssetChange {
    Request(AssetRequest),
    Release(AssetTicket),
}

#[derive(Clone)]
pub(crate) struct Materials(Arc<Retained>);

impl Materials {
    pub(crate) fn faces(&self) -> &[FaceMaterial] {
        &self.0.faces
    }
}

struct Retained {
    faces: Vec<FaceMaterial>,
    _lease: Lease,
}

#[derive(Clone)]
pub(crate) struct MaterialChange {
    pub(crate) source: Object,
    pub(crate) materials: Option<Materials>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct AssetKey {
    region: Region,
    resource: Resource,
}

struct Entry {
    ticket: AssetTicket,
    users: HashMap<Entity, (TextureSize, Priority)>,
    size: TextureSize,
    priority: Priority,
    status: Option<Status>,
}

struct ObjectDemand {
    keys: HashSet<AssetKey>,
    faces: usize,
    capacity: bool,
    size: TextureSize,
    priority: Priority,
}

pub(crate) struct AssetDemand {
    budget: Budget,
    metadata: Option<Lease>,
    camera: Option<Camera>,
    scan: bool,
    rescan: bool,
    deferred: bool,
    cursor: Option<Uuid>,
    dirty: BTreeMap<Uuid, Entity>,
    entries: HashMap<AssetKey, Entry>,
    tickets: HashMap<AssetTicket, AssetKey>,
    targets: HashMap<Entity, ObjectDemand>,
    events: VecDeque<AssetChange>,
    faces: usize,
}

impl AssetDemand {
    pub(crate) fn new(budget: Budget) -> Self {
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
            tickets: HashMap::new(),
            targets: HashMap::new(),
            events: VecDeque::new(),
            faces: 0,
        }
    }

    pub(crate) fn set_camera(&mut self, camera: Camera) {
        if self.camera != Some(camera) {
            self.camera = Some(camera);
            self.rescan = true;
        }
    }

    pub(crate) fn invalidate(&mut self, object: &Object) {
        if self.dirty.len() < crate::MAX_OBJECTS {
            self.dirty.insert(object.detail().summary.id, object.detail().summary.entity);
        }
    }

    pub(crate) fn event(&mut self) -> Option<AssetChange> {
        self.events.pop_front()
    }

    pub(crate) fn has_work(&self) -> bool {
        self.camera.is_some() && (self.scan || self.rescan || !self.dirty.is_empty())
    }

    pub(crate) fn retry_admission(&mut self) -> bool {
        if self.deferred {
            self.deferred = false;
            self.rescan = true;
            true
        } else {
            false
        }
    }

    pub(crate) fn complete(&mut self, ticket: AssetTicket, mut status: Status) {
        let Some(key) = self.tickets.get(&ticket).copied() else { return };
        if let Status::Ready(content) | Status::Refining(content) | Status::RefinementFailed { content, .. } = &status {
            let valid = match (key.resource.kind, content) {
                (Kind::Texture, Content::Texture(texture)) => texture.asset() == key.resource.id,
                (Kind::LegacyMaterial, Content::Material(material)) => matches!(**material, MaterialDescription::Legacy(_)),
                (Kind::PbrMaterial, Content::Material(material)) => matches!(**material, MaterialDescription::Pbr(_)),
                _ => false,
            };
            if !valid {
                status = Status::Failed { failure: Failure::Malformed, retry_at: None };
            }
        }
        let Some(entry) = self.entries.get_mut(&key) else { return };
        if status.content().is_none()
            && let Some(content) = entry.status.as_ref().and_then(Status::content)
        {
            status = match status {
                Status::Failed { failure, .. } => Status::RefinementFailed { content: content.clone(), failure },
                Status::Queued | Status::Fetching | Status::Decoding => Status::Refining(content.clone()),
                other => other,
            };
        }
        entry.status = Some(status);
    }

    pub(crate) fn affected_entities(&self, ticket: AssetTicket) -> Vec<Entity> {
        self.tickets.get(&ticket).and_then(|key| self.entries.get(key)).map_or_else(Vec::new, |entry| entry.users.keys().copied().collect())
    }

    pub(crate) fn remove(&mut self, entity: Entity) {
        if let Some(target) = self.targets.remove(&entity) {
            self.rescan = true;
            self.faces -= target.faces;
            for key in target.keys {
                self.release(entity, key);
            }
        }
    }

    fn release(&mut self, entity: Entity, key: AssetKey) {
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.users.remove(&entity);
            if entry.users.is_empty() {
                let entry = self.entries.remove(&key).expect("entry exists");
                if self.tickets.remove(&entry.ticket).is_some() {
                    self.events.push_back(AssetChange::Release(entry.ticket));
                }
            }
        }
        self.refresh(key);
    }

    fn refresh(&mut self, key: AssetKey) {
        let Some(entry) = self.entries.get_mut(&key) else { return };
        let Some(size) = entry.users.values().map(|(size, _)| *size).max() else { return };
        let priority = entry.users.values().map(|(_, priority)| *priority).max().unwrap_or(Priority::Background);
        if (entry.size == size || key.resource.kind != Kind::Texture) && entry.priority == priority {
            return;
        }
        if self.tickets.remove(&entry.ticket).is_some() {
            self.events.push_back(AssetChange::Release(entry.ticket));
        }
        entry.size = size;
        entry.priority = priority;
        entry.ticket = AssetTicket::new();
        self.tickets.insert(entry.ticket, key);
        self.events.push_back(AssetChange::Request(AssetRequest {
            ticket: entry.ticket,
            region: key.region,
            request: request(key.resource, size),
            priority,
        }));
    }

    pub(crate) fn reset(&mut self) {
        for ticket in self.tickets.keys() {
            self.events.push_back(AssetChange::Release(*ticket));
        }
        self.entries.clear();
        self.tickets.clear();
        self.targets.clear();
        self.dirty.clear();
        self.faces = 0;
        self.scan = self.camera.is_some();
        self.rescan = false;
        self.deferred = false;
        self.cursor = None;
    }

    pub(crate) fn step(&mut self, mirror: &Mirror, identities: &BTreeMap<Uuid, Entity>) -> Vec<MaterialChange> {
        let Some(camera) = self.camera else { return Vec::new() };
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
        let mut changes = Vec::new();
        for _ in 0..128 {
            let (entity, force) = if let Some((_, entity)) = self.dirty.pop_first() {
                (entity, true)
            } else if self.scan {
                let Some((id, entity)) = identities.range((self.cursor.map_or(Unbounded, Excluded), Unbounded)).next() else {
                    self.scan = false;
                    break;
                };
                self.cursor = Some(*id);
                (*entity, false)
            } else {
                break;
            };
            if let Some(object) = mirror.objects.get(&entity)
                && let Some(change) = self.resolve_object_materials(camera, object, force)
            {
                changes.push(change);
            }
        }
        changes
    }

    fn resolve_object_materials(&mut self, camera: Camera, object: &Object, force: bool) -> Option<MaterialChange> {
        let entity = object.detail().summary.entity;
        let mesh = object.mesh().map(|(_, mesh)| mesh);
        let interested = match object.placement() {
            Placement::Ready(transform) => {
                camera.importance(transform.pose.position, 3.0 * transform.scale.as_dvec3().abs().max_element()).is_some()
            }
            _ => false,
        };
        let priority = if interested && mesh.is_some() { Priority::Visible } else { Priority::Background };
        let slots: BTreeSet<_> = if priority == Priority::Visible {
            mesh.expect("visible mesh").mesh().faces().iter().map(|face| face.slot).collect()
        } else {
            let source = object.detail();
            let mut slots = BTreeSet::new();
            if let Some(appearance) = &source.surface_appearance {
                slots.extend(appearance.overrides().map(|(slot, _)| slot));
            }
            if let Some(materials) = &source.render_materials {
                slots.extend(materials.bindings().map(|(slot, _)| slot));
            }
            if source.surface_appearance.is_some()
                && let Some(slot) = (0..MAX_SURFACE_FACES as u8).find(|slot| !slots.contains(slot))
            {
                slots.insert(slot);
            }
            slots
        };
        let count = slots.len();
        let previous = self.targets.get(&entity).map_or(0, |target| target.faces);
        if count == 0 || self.faces - previous + count > MAX_FACES || (previous == 0 && self.targets.len() == MAX_TARGETS) {
            self.remove(entity);
            return object.materials().is_some().then(|| MaterialChange { source: object.clone(), materials: None });
        }
        let size = match object.placement() {
            _ if priority == Priority::Background => TextureSize::new(32),
            Placement::Ready(transform) => camera.texture_size(
                transform.pose.position,
                transform.scale.as_dvec3().length(),
                self.targets.get(&entity).map(|target| target.size),
            ),
            _ => TextureSize::new(32),
        };
        if !force
            && previous == count
            && object.materials().is_some()
            && self.targets.get(&entity).is_some_and(|target| !target.capacity && target.size == size && target.priority == priority)
        {
            return None;
        }
        let region = object.detail().summary.address.region();
        let mut keys = HashSet::new();
        if let Some(target) = self.targets.get(&entity) {
            for key in &target.keys {
                if let Some(entry) = self.entries.get_mut(key) {
                    entry.users.remove(&entity);
                }
            }
        }
        let mut faces = Vec::with_capacity(count);
        for slot in slots {
            faces.push(self.face(object, region, slot, size, priority, &mut keys));
        }
        let old = self.targets.remove(&entity);
        self.faces = self.faces - previous + count;
        if let Some(old) = old {
            for key in old.keys.difference(&keys) {
                self.release(entity, *key);
            }
        }
        let capacity = faces.iter().any(|face| {
            [face.legacy_state, face.pbr_state, face.base_color_state]
                .into_iter()
                .flatten()
                .any(|state| state.resolution == Resolution::Capacity)
        });
        for key in &keys {
            self.refresh(*key);
        }
        self.targets.insert(entity, ObjectDemand { keys, faces: count, capacity, size, priority });
        if object.materials().is_some_and(|old| old.len() == faces.len() && old.iter().zip(&faces).all(|(a, b)| a.equivalent(b))) {
            return None;
        }
        let bytes = size_of::<Retained>() + faces.capacity() * size_of::<FaceMaterial>() + 32;
        let materials = self.budget.reserve(bytes).map(|lease| Materials(Arc::new(Retained { faces, _lease: lease })));
        self.deferred |= materials.is_none();
        Some(MaterialChange { source: object.clone(), materials })
    }

    fn observe(
        &mut self,
        entity: Entity,
        region: Region,
        resource: Resource,
        size: TextureSize,
        priority: Priority,
        keys: &mut HashSet<AssetKey>,
    ) -> (Resolution, Option<Content>) {
        let key = AssetKey { region, resource };
        keys.insert(key);
        let entry = self.entries.entry(key).or_insert_with(|| {
            let ticket = AssetTicket::new();
            self.tickets.insert(ticket, key);
            self.events.push_back(AssetChange::Request(AssetRequest { ticket, region, request: request(resource, size), priority }));
            Entry { ticket, users: HashMap::new(), size, priority, status: None }
        });
        entry
            .users
            .entry(entity)
            .and_modify(|old| {
                old.0 = old.0.max(size);
                old.1 = old.1.max(priority);
            })
            .or_insert((size, priority));
        match &entry.status {
            Some(Status::Ready(content)) => (Resolution::Ready, Some(content.clone())),
            Some(Status::Refining(content)) => (Resolution::Pending, Some(content.clone())),
            Some(Status::RefinementFailed { content, failure }) => (Resolution::Failed(*failure), Some(content.clone())),
            Some(Status::Failed { failure, .. }) => (Resolution::Failed(*failure), None),
            Some(Status::StaleSession) => (Resolution::Failed(Failure::SessionClosed), None),
            _ => (Resolution::Pending, None),
        }
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
        let (legacy_state, legacy) = if authored.material_id.is_nil() {
            (None, None)
        } else {
            let id = AssetId::new(authored.material_id);
            let (resolution, content) = self.observe(entity, region, Resource { id, kind: Kind::LegacyMaterial }, size, priority, keys);
            let material = match content {
                Some(Content::Material(material)) => match *material {
                    MaterialDescription::Legacy(material) => Some(material),
                    _ => None,
                },
                _ => None,
            };
            (Some(AssetState { asset: id, resolution }), material)
        };
        let (pbr_state, pbr) = if let Some(id) = source.render_materials.as_ref().and_then(|materials| materials.material(slot)) {
            let (resolution, content) = self.observe(entity, region, Resource { id, kind: Kind::PbrMaterial }, size, priority, keys);
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
        let overrides = source.gltf_material_overrides.as_ref().and_then(|overrides| overrides.face(slot));
        let (material, alpha) = appearance::resolve(authored, legacy, pbr, overrides);
        let mut face = FaceMaterial { slot, material, legacy_state, pbr_state, alpha, base_color: None, base_color_state: None };
        if let TextureRef::Asset(id) = face.base_color_texture() {
            let scale = face.material.base_color_texture().transform.scale;
            let repeats = f64::from(scale[0].abs().min(scale[1].abs())).max(1.0 / 64.0);
            let size = if priority == Priority::Background {
                TextureSize::new(32)
            } else {
                TextureSize::new((f64::from(size.edge()) / repeats).ceil() as u32)
            };
            let (resolution, content) = self.observe(entity, region, Resource { id, kind: Kind::Texture }, size, priority, keys);
            face.base_color_state = Some(AssetState { asset: id, resolution });
            if let Some(Content::Texture(image)) = content {
                face.resolve_base_color(image);
            }
        }
        face
    }
}

fn request(resource: Resource, size: TextureSize) -> Request {
    if resource.kind == Kind::Texture { Request::Texture { id: resource.id, size } } else { Request::new(resource) }
}
