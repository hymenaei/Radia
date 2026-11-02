use std::collections::{BTreeSet, HashMap};

use assets::{
    MeshAsset, MeshHeaderAsset, Priority, Request,
    mesh::{LodGeometry, MeshLod},
};
use utils::AssetId;
use world::{Entity, MAX_SURFACE_FACES};

use crate::scene::{Budget, DrawnMeshKey, MAX_OBJECTS, Object, SharedMesh, mesh::asset_mesh_bytes};

#[derive(Default)]
pub(super) struct MeshAssets {
    records: HashMap<AssetId, MeshRecord>,
}

#[derive(Default)]
struct MeshRecord {
    header: Option<MeshHeaderAsset>,
    lods: [LodContent; 4],
    lod_users: [usize; 4],
    users: HashMap<Entity, MeshUser>,
}

#[derive(Default)]
struct MeshUser {
    requested: [Option<MeshLod>; 2],
    published: Option<MeshLod>,
}

#[derive(Default)]
enum LodContent {
    #[default]
    Pending,
    PendingAsset(MeshAsset),
    Empty,
    Unavailable,
    Ready(SharedMesh),
}

pub(super) struct MeshRequestPlan {
    pub(super) needs_header: bool,
    pub(super) requests: Vec<Request>,
    pub(super) retained: [Option<MeshLod>; 2],
}

pub(super) enum MeshResolution {
    Waiting,
    Empty,
    Failed,
    Ready(DrawnMeshKey, SharedMesh),
}

pub(super) struct MeshContentUpdate {
    pub(super) changed: bool,
    pub(super) deferred: bool,
}

#[derive(Clone)]
pub(in crate::scene) enum MeshPublication {
    Keep,
    Clear,
    Set(DrawnMeshKey, SharedMesh),
}

pub(super) struct MeshPublicationResult {
    pub(super) publication: MeshPublication,
    pub(super) mesh: Option<SharedMesh>,
}

pub(super) fn publication(object: &Object, asset: Option<AssetId>, resolution: Option<MeshResolution>) -> MeshPublicationResult {
    match (asset, resolution) {
        (Some(_), Some(MeshResolution::Ready(key, mesh))) => {
            let current = object.mesh().is_some_and(|(current_key, current)| current_key == key && current.id() == mesh.id());
            let publication = if current { MeshPublication::Keep } else { MeshPublication::Set(key, mesh.clone()) };
            MeshPublicationResult { publication, mesh: Some(mesh) }
        }
        (Some(_), Some(MeshResolution::Empty | MeshResolution::Failed)) | (Some(_), None) => {
            MeshPublicationResult { publication: MeshPublication::Clear, mesh: None }
        }
        (Some(_), Some(MeshResolution::Waiting)) => {
            let mesh = object.mesh_matches_geometry().then(|| object.mesh().map(|(_, mesh)| mesh.clone())).flatten();
            MeshPublicationResult { publication: MeshPublication::Keep, mesh }
        }
        (None, _) if object.mesh_matches_geometry() => {
            let mesh = object.mesh().map(|(_, mesh)| mesh.clone());
            MeshPublicationResult { publication: MeshPublication::Keep, mesh }
        }
        _ => MeshPublicationResult { publication: MeshPublication::Keep, mesh: None },
    }
}

pub(super) fn material_slots(object: &Object, asset: bool, mesh: Option<&SharedMesh>, priority: Priority) -> BTreeSet<u8> {
    if asset {
        return match mesh {
            Some(mesh) => mesh.mesh().faces().iter().map(|face| face.slot).collect(),
            None if priority == Priority::Visible => fallback_slots(object).into_iter().take(8).collect(),
            None => BTreeSet::new(),
        };
    }
    if priority == Priority::Visible
        && let Some(mesh) = mesh
    {
        return mesh.mesh().faces().iter().map(|face| face.slot).collect();
    }
    fallback_slots(object)
}

fn fallback_slots(object: &Object) -> BTreeSet<u8> {
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
}

impl MeshAssets {
    pub(super) fn add_user(&mut self, asset: AssetId, entity: Entity) -> bool {
        if !self.records.contains_key(&asset) && self.records.len() >= MAX_OBJECTS {
            return false;
        }
        self.records.entry(asset).or_default().users.entry(entity).or_default();
        true
    }

    pub(super) fn set_user_lods(&mut self, asset: AssetId, entity: Entity, requested: [Option<MeshLod>; 2], published: Option<MeshLod>) {
        let Some(record) = self.records.get_mut(&asset) else { return };
        let changed = if let Some(user) = record.users.get_mut(&entity) {
            let previous = retained_lods(user);
            user.requested = requested;
            user.published = published;
            Some((previous, retained_lods(user)))
        } else {
            None
        };
        if let Some((previous, next)) = changed {
            update_lod_users(record, previous, next);
        }
        release_unretained_lods(record);
    }

    pub(super) fn remove_user(&mut self, asset: AssetId, entity: Entity) {
        let empty = self.records.get_mut(&asset).is_some_and(|record| {
            if let Some(user) = record.users.remove(&entity) {
                update_lod_users(record, retained_lods(&user), [false; 4]);
            }
            release_unretained_lods(record);
            record.users.is_empty()
        });
        if empty {
            self.records.remove(&asset);
        }
    }

    pub(super) fn clear(&mut self) {
        self.records.clear();
    }

    pub(super) fn users(&self, asset: AssetId) -> Vec<Entity> {
        self.records.get(&asset).map_or_else(Vec::new, |record| record.users.keys().copied().collect())
    }

    pub(super) fn set_header(&mut self, asset: AssetId, header: MeshHeaderAsset) -> bool {
        let Some(record) = self.records.get_mut(&asset) else { return false };
        if record.header.is_some() {
            return false;
        }
        record.header = Some(header);
        true
    }

    pub(super) fn set_lod(&mut self, asset: AssetId, content: MeshAsset, budget: &Budget) -> MeshContentUpdate {
        let lod = content.lod();
        let Some(record) = self.records.get_mut(&asset) else { return MeshContentUpdate { changed: false, deferred: false } };
        let Some(state) = record.lods.get_mut(lod.index()) else { return MeshContentUpdate { changed: false, deferred: false } };
        if !matches!(state, LodContent::Pending | LodContent::PendingAsset(_)) {
            return MeshContentUpdate { changed: false, deferred: false };
        }
        match content.geometry() {
            LodGeometry::Empty { face_count: 0 } => {
                *state = LodContent::Unavailable;
                MeshContentUpdate { changed: true, deferred: false }
            }
            LodGeometry::Empty { .. } => {
                *state = LodContent::Empty;
                MeshContentUpdate { changed: true, deferred: false }
            }
            LodGeometry::Geometry(_) => {
                let Some(lease) = budget.reserve(asset_mesh_bytes()) else {
                    *state = LodContent::PendingAsset(content);
                    return MeshContentUpdate { changed: false, deferred: true };
                };
                let Some(mesh) = SharedMesh::from_asset(content, lease) else {
                    return MeshContentUpdate { changed: false, deferred: false };
                };
                *state = LodContent::Ready(mesh);
                MeshContentUpdate { changed: true, deferred: false }
            }
        }
    }

    pub(super) fn retry_pending(&mut self, asset: AssetId, budget: &Budget) -> bool {
        let mut deferred = false;
        for lod in [MeshLod::Lowest, MeshLod::Low, MeshLod::Medium, MeshLod::High] {
            let content = self.records.get(&asset).and_then(|record| match record.lods.get(lod.index()) {
                Some(LodContent::PendingAsset(content)) => Some(content.clone()),
                _ => None,
            });
            let Some(content) = content else { continue };
            let Some(lease) = budget.reserve(asset_mesh_bytes()) else {
                deferred = true;
                continue;
            };
            let Some(mesh) = SharedMesh::from_asset(content, lease) else { continue };
            if let Some(record) = self.records.get_mut(&asset)
                && let Some(state @ LodContent::PendingAsset(_)) = record.lods.get_mut(lod.index())
            {
                *state = LodContent::Ready(mesh);
            }
        }
        deferred
    }

    pub(super) fn request_plan(&self, asset: AssetId, desired: MeshLod, current: Option<MeshLod>, priority: Priority) -> MeshRequestPlan {
        let Some(record) = self.records.get(&asset) else {
            return MeshRequestPlan { needs_header: true, requests: Vec::new(), retained: [None; 2] };
        };
        let Some(header) = record.header.as_ref() else {
            return MeshRequestPlan { needs_header: true, requests: Vec::new(), retained: [None; 2] };
        };
        let visible = priority == Priority::Visible;
        let selected = if visible {
            selected_lod(record, header, asset, desired)
        } else if header.lod(asset, MeshLod::Lowest).is_ok() && !matches!(record.lods[MeshLod::Lowest.index()], LodContent::Unavailable) {
            Some(MeshLod::Lowest)
        } else {
            None
        };
        let mut requests = Vec::with_capacity(2);
        let mut retained = [None; 2];
        if let Some(lod) = selected {
            retained[0] = Some(lod);
            if state_pending(record, lod)
                && let Ok(request) = header.lod(asset, lod)
            {
                requests.push(Request::MeshLod(request));
                if visible && lod == desired && !has_displayable_fallback(record, current, desired) {
                    let fallback = fallback_lod(record, header, asset, desired);
                    if let Some(fallback) = fallback
                        && state_pending(record, fallback)
                        && let Ok(request) = header.lod(asset, fallback)
                    {
                        retained[1] = Some(fallback);
                        requests.push(Request::MeshLod(request));
                    }
                }
            }
        }
        MeshRequestPlan { needs_header: false, requests, retained }
    }

    pub(super) fn resolve(&self, asset: AssetId, desired: MeshLod, current: Option<MeshLod>) -> MeshResolution {
        let Some(record) = self.records.get(&asset) else { return MeshResolution::Waiting };
        let Some(header) = record.header.as_ref() else { return MeshResolution::Waiting };
        let Some(selected) = selected_lod(record, header, asset, desired) else { return MeshResolution::Waiting };
        if let Some(resolution) = resolved_lod(record, asset, selected) {
            return resolution;
        }
        if let Some(current) = current
            && current != selected
            && let Some(LodContent::Ready(mesh)) = record.lods.get(current.index())
        {
            return MeshResolution::Ready(DrawnMeshKey::Asset { asset, lod: current }, mesh.clone());
        }
        for lod in desired.fallback_order().into_iter().skip(1) {
            if lod == selected {
                continue;
            }
            if let Some(resolution) = resolved_lod(record, asset, lod) {
                return resolution;
            }
        }
        MeshResolution::Waiting
    }
}

fn retained_lods(user: &MeshUser) -> [bool; 4] {
    let mut retained = [false; 4];
    for lod in user.requested.into_iter().flatten().chain(user.published) {
        if let Some(value) = retained.get_mut(lod.index()) {
            *value = true;
        }
    }
    retained
}

fn update_lod_users(record: &mut MeshRecord, previous: [bool; 4], next: [bool; 4]) {
    for index in 0..record.lod_users.len() {
        match (previous.get(index), next.get(index)) {
            (Some(true), Some(false)) => {
                if let Some(count) = record.lod_users.get_mut(index)
                    && let Some(next) = count.checked_sub(1)
                {
                    *count = next;
                }
            }
            (Some(false), Some(true)) => {
                if let Some(count) = record.lod_users.get_mut(index) {
                    *count = count.saturating_add(1);
                }
            }
            _ => {}
        }
    }
}

fn release_unretained_lods(record: &mut MeshRecord) {
    for (index, count) in record.lod_users.iter().enumerate() {
        if *count > 0 {
            continue;
        }
        if let Some(state @ (LodContent::PendingAsset(_) | LodContent::Ready(_))) = record.lods.get_mut(index) {
            *state = LodContent::Pending;
        }
    }
}

fn has_displayable_fallback(record: &MeshRecord, current: Option<MeshLod>, desired: MeshLod) -> bool {
    current.is_some_and(|lod| matches!(record.lods.get(lod.index()), Some(LodContent::Ready(_))))
        || desired
            .fallback_order()
            .into_iter()
            .skip(1)
            .any(|lod| matches!(record.lods.get(lod.index()), Some(LodContent::Ready(_) | LodContent::Empty)))
}

fn selected_lod(record: &MeshRecord, header: &MeshHeaderAsset, asset: AssetId, desired: MeshLod) -> Option<MeshLod> {
    desired
        .fallback_order()
        .into_iter()
        .find(|lod| header.lod(asset, *lod).is_ok() && !matches!(record.lods.get(lod.index()), Some(LodContent::Unavailable)))
}

fn fallback_lod(record: &MeshRecord, header: &MeshHeaderAsset, asset: AssetId, desired: MeshLod) -> Option<MeshLod> {
    if desired == MeshLod::Lowest {
        return None;
    }
    let available = |lod: &MeshLod| header.lod(asset, *lod).is_ok() && !matches!(record.lods[lod.index()], LodContent::Unavailable);
    if available(&MeshLod::Lowest) {
        return Some(MeshLod::Lowest);
    }
    desired.fallback_order().into_iter().skip(1).take_while(|lod| lod.index() < desired.index()).find(|lod| available(lod))
}

fn resolved_lod(record: &MeshRecord, asset: AssetId, lod: MeshLod) -> Option<MeshResolution> {
    match record.lods.get(lod.index()) {
        Some(LodContent::Ready(mesh)) => Some(MeshResolution::Ready(DrawnMeshKey::Asset { asset, lod }, mesh.clone())),
        Some(LodContent::Empty) => Some(MeshResolution::Empty),
        _ => None,
    }
}

fn state_pending(record: &MeshRecord, lod: MeshLod) -> bool {
    matches!(record.lods.get(lod.index()), Some(LodContent::Pending | LodContent::PendingAsset(_)))
}
