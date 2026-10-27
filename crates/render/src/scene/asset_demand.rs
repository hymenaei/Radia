use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    ops::Bound::{Excluded, Unbounded},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use assets::{Content, Failure, Kind, MaterialDescription, Priority, Request, Resource, Status, TextureRef, TextureSize};
use math::DVec3;
use utils::AssetId;
use uuid::Uuid;
use world::{Entity, FaceAppearance, MAX_SURFACE_FACES, Region};

use crate::material::{self, AssetState, FaceMaterial, Resolution};
use crate::scene::{Budget, Camera, MAX_OBJECTS, Object, Placement, budget::Lease, model::Mirror};

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
}

pub enum AssetChange {
    Request(AssetRequest),
    Release(AssetTicket),
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
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct AssetKey {
    region: Region,
    resource: Resource,
}

struct Entry {
    ticket: AssetTicket,
    users: HashMap<AssetUser, (TextureSize, Priority)>,
    size: TextureSize,
    priority: Priority,
    status: Option<Status>,
}

struct ObjectDemand {
    keys: HashSet<AssetKey>,
    faces: usize,
    size: TextureSize,
    priority: Priority,
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
    tickets: HashMap<AssetTicket, AssetKey>,
    targets: HashMap<Entity, ObjectDemand>,
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
            tickets: HashMap::new(),
            targets: HashMap::new(),
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
            let key = AssetKey { region, resource: Resource { id: AssetId::new(target.textures[index]), kind: Kind::Texture } };
            self.entries.get(&key).and_then(|entry| entry.status.as_ref()).and_then(Status::content).and_then(|content| match content {
                Content::Texture(texture) => Some(texture.clone()),
                _ => None,
            })
        })
    }

    pub(in crate::scene) fn has_work(&self) -> bool {
        self.camera.is_some()
            && (self.scan || self.rescan || !self.dirty.is_empty() || !self.dirty_terrain.is_empty() || self.terrain_rescan)
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
        let Some(entry) = self.entries.get_mut(&key) else { return Vec::new() };
        entry.status = Some(status.retaining(entry.status.as_ref().and_then(Status::content)));
        let users: Vec<_> = entry.users.keys().copied().collect();
        self.terrain_changes.extend(users.iter().filter_map(|user| match user {
            AssetUser::Terrain(region) => Some(*region),
            AssetUser::Object(_) => None,
        }));
        users
    }

    pub(in crate::scene) fn remove(&mut self, entity: Entity) {
        if let Some(target) = self.targets.remove(&entity) {
            self.rescan = true;
            self.faces -= target.faces;
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
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.users.remove(&user);
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
        let size = TextureSize::new(128);
        let priority = data.summary().info.resolved_origin().map_or(Priority::Background, |origin| {
            let center = origin + DVec3::new(128.0, 128.0, 0.0);
            if camera.importance(center, 181.0).is_some() { Priority::Visible } else { Priority::Background }
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
            self.register(user, region, Resource { id: AssetId::new(id), kind: Kind::Texture }, size, priority, &mut keys);
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
        self.tickets.clear();
        self.targets.clear();
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
        let mut changes = Vec::new();
        let mut entries = identities.range((self.cursor.map_or(Unbounded, Excluded), Unbounded));
        for _ in 0..128 {
            let (entity, force) = if let Some((_, entity)) = self.dirty.pop_first() {
                (entity, true)
            } else if self.scan {
                let Some((id, entity)) = entries.next() else {
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
            if let Some(materials) = &source.pbr_materials {
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
            && self.targets.get(&entity).is_some_and(|target| target.size == size && target.priority == priority)
        {
            return None;
        }
        let region = object.detail().summary.address.region();
        let mut keys = HashSet::new();
        if let Some(target) = self.targets.get(&entity) {
            for key in &target.keys {
                if let Some(entry) = self.entries.get_mut(key) {
                    entry.users.remove(&AssetUser::Object(entity));
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
                self.release(AssetUser::Object(entity), *key);
            }
        }
        for key in &keys {
            self.refresh(*key);
        }
        self.targets.insert(entity, ObjectDemand { keys, faces: count, size, priority });
        if object.materials().is_some_and(|old| old == faces) {
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
        let key = self.register(AssetUser::Object(entity), region, resource, size, priority, keys);
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
        resource: Resource,
        size: TextureSize,
        priority: Priority,
        keys: &mut HashSet<AssetKey>,
    ) -> AssetKey {
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
            .entry(user)
            .and_modify(|old| {
                old.0 = old.0.max(size);
                old.1 = old.1.max(priority);
            })
            .or_insert((size, priority));
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
            let (resolution, content) = self.observe(entity, region, Resource { id, kind: Kind::LegacyMaterial }, size, priority, keys);
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
            let (resolution, content) = self.observe(entity, region, Resource { id, kind: Kind::Texture }, size, priority, keys);
            let image = match content {
                Some(Content::Texture(image)) => Some(image),
                _ => None,
            };
            face.resolve_base_color(AssetState { asset: id, resolution }, image);
        }
        face
    }
}

fn request(resource: Resource, size: TextureSize) -> Request {
    if resource.kind == Kind::Texture { Request::Texture { id: resource.id, size } } else { Request::new(resource) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::RegionData;
    use math::DVec3;
    use world::{RegionInfo, RegionSummary, TerrainSettings};

    fn settings(textures: [Uuid; 4]) -> TerrainSettings {
        TerrainSettings::new(textures, [0.0; 4], [64.0; 4]).unwrap()
    }

    fn region_data(budget: &Budget, region: Region, origin: DVec3, textures: [Uuid; 4]) -> RegionData {
        RegionData::new(
            RegionSummary {
                region,
                info: RegionInfo { origin: Some(origin), ..RegionInfo::default() },
                terrain_settings: Some(settings(textures)),
            },
            budget.reserve(512).unwrap(),
        )
    }

    fn camera() -> Camera {
        Camera::new(DVec3::new(128.0, 128.0, 32.0), DVec3::X.as_vec3(), 1.0, [1280, 720]).unwrap()
    }

    fn ids(seed: u128) -> [Uuid; 4] {
        std::array::from_fn(|index| Uuid::from_u128(seed + index as u128))
    }

    fn process(demand: &mut AssetDemand, mirror: &Mirror) {
        let _ = demand.step(mirror, &BTreeMap::new());
    }

    #[test]
    fn live_region_texture_changes_release_old_demands_and_request_replacements() {
        let budget = Budget::default();
        let region = Region::new(1);
        let original = ids(10);
        let replacement = ids(20);
        let mut mirror = Mirror::default();
        mirror.regions.insert(region, region_data(&budget, region, DVec3::ZERO, original));
        let mut demand = AssetDemand::new(budget.clone());
        demand.set_camera(camera());
        demand.invalidate_region(region);
        process(&mut demand, &mirror);
        let old_tickets: Vec<_> = std::iter::from_fn(|| demand.event())
            .filter_map(|change| match change {
                AssetChange::Request(request) => Some(request.ticket),
                AssetChange::Release(_) => None,
            })
            .collect();
        assert_eq!(old_tickets.len(), 4);
        assert_eq!(demand.terrain_targets[&region].textures, original);

        mirror.regions.insert(region, region_data(&budget, region, DVec3::ZERO, replacement));
        demand.invalidate_region(region);
        process(&mut demand, &mirror);
        assert_eq!(demand.terrain_targets[&region].textures, replacement);
        let mut requested = 0;
        let mut released = 0;
        while let Some(change) = demand.event() {
            match change {
                AssetChange::Request(request) => {
                    assert_eq!(request.region, region);
                    requested += 1;
                }
                AssetChange::Release(_) => released += 1,
            }
        }
        assert_eq!(requested, 4);
        assert_eq!(released, 4);
        assert!(demand.entries.keys().all(|key| key.region != region || replacement.contains(&key.resource.id.uuid())));
    }

    #[test]
    fn missing_and_failed_details_keep_the_neutral_fallback_for_each_layer() {
        let budget = Budget::default();
        let region = Region::new(1);
        let textures = ids(30);
        let mut mirror = Mirror::default();
        mirror.regions.insert(region, region_data(&budget, region, DVec3::ZERO, textures));
        let mut demand = AssetDemand::new(budget);
        demand.set_camera(camera());
        demand.invalidate_region(region);
        process(&mut demand, &mirror);
        assert!(demand.terrain_textures(region).iter().all(Option::is_none));
        let tickets: Vec<_> = std::iter::from_fn(|| demand.event())
            .filter_map(|change| match change {
                AssetChange::Request(request) => Some(request.ticket),
                AssetChange::Release(_) => None,
            })
            .collect();
        assert_eq!(tickets.len(), 4);
        for ticket in tickets {
            let users = demand.complete(ticket, Status::Failed { failure: Failure::NotFound, retry_at: None });
            assert_eq!(users, [AssetUser::Terrain(region)]);
        }
        assert!(demand.terrain_textures(region).iter().all(Option::is_none));
        assert_eq!(demand.take_terrain_changes(), [region]);
    }

    #[test]
    fn neighboring_regions_keep_separate_detail_texture_sets() {
        let budget = Budget::default();
        let south = Region::new(1);
        let east = Region::new(2);
        let south_textures = ids(40);
        let east_textures = ids(50);
        let mut mirror = Mirror::default();
        mirror.regions.insert(south, region_data(&budget, south, DVec3::ZERO, south_textures));
        mirror.regions.insert(east, region_data(&budget, east, DVec3::new(256.0, 0.0, 0.0), east_textures));
        let mut demand = AssetDemand::new(budget);
        demand.set_camera(camera());
        demand.invalidate_region(south);
        demand.invalidate_region(east);
        process(&mut demand, &mirror);
        process(&mut demand, &mirror);
        assert_eq!(demand.terrain_targets[&south].textures, south_textures);
        assert_eq!(demand.terrain_targets[&east].textures, east_textures);
        assert_eq!(demand.entries.keys().filter(|key| key.region == south).count(), 4);
        assert_eq!(demand.entries.keys().filter(|key| key.region == east).count(), 4);
    }
}
