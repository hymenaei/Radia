use std::collections::{BTreeSet, HashMap, HashSet};

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
    users: HashSet<Entity>,
}

#[derive(Default)]
enum LodContent {
    #[default]
    Pending,
    PendingAsset(MeshAsset),
    Empty,
    Ready(SharedMesh),
}

pub(super) struct MeshRequestPlan {
    pub(super) needs_header: bool,
    pub(super) requests: Vec<Request>,
}

pub(super) enum MeshResolution {
    Waiting,
    Empty,
    Failed,
    Ready(DrawnMeshKey, SharedMesh),
}

#[derive(Clone)]
pub(in crate::scene) enum MeshPublication {
    Keep,
    Clear,
    Set(DrawnMeshKey, SharedMesh),
}

pub(super) fn publication(
    object: &Object,
    asset: Option<AssetId>,
    resolution: Option<MeshResolution>,
) -> (MeshPublication, Option<SharedMesh>) {
    match (asset, resolution) {
        (Some(_), Some(MeshResolution::Ready(key, mesh))) => {
            let current = object.mesh().is_some_and(|(current_key, current)| current_key == key && current.id() == mesh.id());
            if current { (MeshPublication::Keep, Some(mesh)) } else { (MeshPublication::Set(key, mesh.clone()), Some(mesh)) }
        }
        (Some(_), Some(MeshResolution::Empty | MeshResolution::Failed)) | (Some(_), None) => (MeshPublication::Clear, None),
        (Some(_), Some(MeshResolution::Waiting)) => {
            let mesh = object.mesh_matches_geometry().then(|| object.mesh().map(|(_, mesh)| mesh.clone())).flatten();
            (MeshPublication::Keep, mesh)
        }
        (None, _) if object.mesh_matches_geometry() => (MeshPublication::Keep, object.mesh().map(|(_, mesh)| mesh.clone())),
        _ => (MeshPublication::Keep, None),
    }
}

pub(super) fn material_slots(object: &Object, asset: bool, mesh: Option<&SharedMesh>, priority: Priority) -> BTreeSet<u8> {
    if asset || priority == Priority::Visible && mesh.is_some() {
        return mesh.map_or_else(BTreeSet::new, |mesh| mesh.mesh().faces().iter().map(|face| face.slot).collect());
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
        self.records.entry(asset).or_default().users.insert(entity);
        true
    }

    pub(super) fn remove_user(&mut self, asset: AssetId, entity: Entity) {
        let empty = self.records.get_mut(&asset).is_some_and(|record| {
            record.users.remove(&entity);
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
        self.records.get(&asset).map_or_else(Vec::new, |record| record.users.iter().copied().collect())
    }

    pub(super) fn set_header(&mut self, asset: AssetId, header: MeshHeaderAsset) -> bool {
        let Some(record) = self.records.get_mut(&asset) else { return false };
        if record.header.is_some() {
            return false;
        }
        record.header = Some(header);
        true
    }

    pub(super) fn set_lod(&mut self, asset: AssetId, content: MeshAsset, budget: &Budget) -> (bool, bool) {
        let lod = content.lod();
        let Some(record) = self.records.get_mut(&asset) else { return (false, false) };
        let Some(state) = record.lods.get_mut(lod_index(lod)) else { return (false, false) };
        if !matches!(state, LodContent::Pending | LodContent::PendingAsset(_)) {
            return (false, false);
        }
        match content.geometry() {
            LodGeometry::Empty { .. } => {
                *state = LodContent::Empty;
                (true, false)
            }
            LodGeometry::Geometry(_) => {
                let Some(lease) = budget.reserve(asset_mesh_bytes()) else {
                    *state = LodContent::PendingAsset(content);
                    return (false, true);
                };
                let Some(mesh) = SharedMesh::from_asset(content, lease) else { return (false, false) };
                *state = LodContent::Ready(mesh);
                (true, false)
            }
        }
    }

    pub(super) fn retry_pending(&mut self, asset: AssetId, budget: &Budget) -> bool {
        let mut deferred = false;
        for index in 0..4 {
            let content = self.records.get(&asset).and_then(|record| match record.lods.get(index) {
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
                && let Some(state @ LodContent::PendingAsset(_)) = record.lods.get_mut(index)
            {
                *state = LodContent::Ready(mesh);
            }
        }
        deferred
    }

    pub(super) fn request_plan(&self, asset: AssetId, desired: MeshLod, priority: Priority) -> MeshRequestPlan {
        let Some(record) = self.records.get(&asset) else {
            return MeshRequestPlan { needs_header: true, requests: Vec::new() };
        };
        let Some(header) = record.header.as_ref() else {
            return MeshRequestPlan { needs_header: true, requests: Vec::new() };
        };
        let visible = priority == Priority::Visible;
        let selected = if visible {
            available_lod(header, asset, desired)
        } else if header.lod(asset, MeshLod::Lowest).is_ok() {
            Some(MeshLod::Lowest)
        } else {
            None
        };
        let mut requests = Vec::with_capacity(2);
        if let Some(lod) = selected
            && state_pending(record, lod)
            && let Ok(request) = header.lod(asset, lod)
        {
            requests.push(Request::MeshLod(request));
            if visible
                && lod == desired
                && let Some(fallback) = nearest_available(header, asset, desired)
                && state_pending(record, fallback)
                && let Ok(request) = header.lod(asset, fallback)
            {
                requests.push(Request::MeshLod(request));
            }
        }
        MeshRequestPlan { needs_header: false, requests }
    }

    pub(super) fn resolve(&self, asset: AssetId, desired: MeshLod) -> MeshResolution {
        let Some(record) = self.records.get(&asset) else { return MeshResolution::Waiting };
        if matches!(record.lods.get(lod_index(desired)), Some(LodContent::Empty)) {
            return MeshResolution::Empty;
        }
        let mut lods = [MeshLod::Lowest, MeshLod::Low, MeshLod::Medium, MeshLod::High];
        lods.sort_by_key(|lod| {
            let index = lod_index(*lod);
            (index.abs_diff(lod_index(desired)), usize::from(index > lod_index(desired)))
        });
        for lod in lods {
            if let Some(LodContent::Ready(mesh)) = record.lods.get(lod_index(lod)) {
                return MeshResolution::Ready(DrawnMeshKey::Asset { asset, lod }, mesh.clone());
            }
        }
        MeshResolution::Waiting
    }
}

fn state_pending(record: &MeshRecord, lod: MeshLod) -> bool {
    matches!(record.lods.get(lod_index(lod)), Some(LodContent::Pending | LodContent::PendingAsset(_)))
}

fn available_lod(header: &MeshHeaderAsset, asset: AssetId, desired: MeshLod) -> Option<MeshLod> {
    if header.lod(asset, desired).is_ok() { Some(desired) } else { nearest_available(header, asset, desired) }
}

fn nearest_available(header: &MeshHeaderAsset, asset: AssetId, desired: MeshLod) -> Option<MeshLod> {
    let mut lods = [MeshLod::Lowest, MeshLod::Low, MeshLod::Medium, MeshLod::High]
        .into_iter()
        .filter(|lod| *lod != desired && header.lod(asset, *lod).is_ok())
        .collect::<Vec<_>>();
    lods.sort_by_key(|lod| {
        let index = lod_index(*lod);
        (index.abs_diff(lod_index(desired)), usize::from(index > lod_index(desired)))
    });
    lods.into_iter().next()
}

fn lod_index(lod: MeshLod) -> usize {
    match lod {
        MeshLod::Lowest => 0,
        MeshLod::Low => 1,
        MeshLod::Medium => 2,
        MeshLod::High => 3,
    }
}
