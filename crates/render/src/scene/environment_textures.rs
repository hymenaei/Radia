use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use assets::{
    Content, EnvironmentDescription, EnvironmentSettings, Priority, Request, Sky, Status, TextureHandle, TextureRef, TextureSize, Water,
};
use utils::AssetId;
use world::Region;

use crate::{
    environment::{
        DEFAULT_BLOOM_TEXTURE, DEFAULT_CLOUD_TEXTURE, DEFAULT_HALO_TEXTURE, DEFAULT_MOON_TEXTURE, DEFAULT_RAINBOW_TEXTURE,
        DEFAULT_SUN_TEXTURE, DEFAULT_WATER_NORMAL_MAP,
    },
    scene::{AssetChange, AssetRequest, AssetTicket, ViewerEnvironment, region_environment::RegionEnvironment},
};

const MAX_PER_ENVIRONMENT: usize = 16;
const MAX_TEXTURES: usize = 64;
const TEXTURE_SIZE: u32 = 128;

pub(in crate::scene) struct EnvironmentTextures {
    entries: BTreeMap<AssetId, Entry>,
    tickets: HashMap<AssetTicket, AssetId>,
    events: VecDeque<AssetChange>,
    dirty: BTreeSet<AssetId>,
    revision: u64,
}

struct Entry {
    ticket: AssetTicket,
    handle: Option<TextureHandle>,
    used: u64,
}

impl EnvironmentTextures {
    pub(in crate::scene) fn new() -> Self {
        Self { entries: BTreeMap::new(), tickets: HashMap::new(), events: VecDeque::new(), dirty: BTreeSet::new(), revision: 0 }
    }

    pub(in crate::scene) fn update(
        &mut self,
        environments: &BTreeMap<Region, RegionEnvironment>,
        viewer: Option<&ViewerEnvironment>,
        active: &BTreeMap<AssetId, Region>,
    ) {
        self.revision = self.revision.checked_add(1).expect("environment texture revision space exhausted");
        let mut desired = BTreeMap::new();
        desired.extend(active.iter().map(|(id, region)| (*id, *region)));
        if let Some(viewer) = viewer {
            for setting in [viewer.day(), viewer.sky(), viewer.water()].into_iter().flatten() {
                extend_desired(&mut desired, description_textures(setting.environment()), setting.region());
            }
            if let Some(local) = viewer.local() {
                extend_desired(&mut desired, description_textures(local.environment()), local.region());
            }
            if let Some(sky) = viewer.sky_overrides()
                && let Some(region) = property_region(viewer.sky_properties())
            {
                extend_desired(&mut desired, override_sky_textures(sky), region);
            }
            if let Some(water) = viewer.water_overrides()
                && let Some(region) = property_region(viewer.water_properties())
            {
                extend_desired(&mut desired, override_water_textures(water), region);
            }
        }
        if desired.len() < MAX_TEXTURES {
            for (region, environment) in environments {
                extend_desired(&mut desired, description_textures(environment.description()), *region);
                if desired.len() >= MAX_TEXTURES {
                    break;
                }
            }
        }

        for id in desired.keys() {
            if let Some(entry) = self.entries.get_mut(id) {
                entry.used = self.revision;
            }
        }

        let protected = desired.keys().copied().collect::<BTreeSet<_>>();
        for (id, region) in desired {
            if self.entries.contains_key(&id) {
                continue;
            }
            if self.entries.len() == MAX_TEXTURES {
                let victim = self
                    .entries
                    .iter()
                    .filter(|(candidate, _)| !protected.contains(*candidate))
                    .min_by_key(|(candidate, entry)| (entry.used, **candidate))
                    .map(|(candidate, _)| *candidate);
                let Some(victim) = victim else { continue };
                self.release(victim);
            }
            let ticket = AssetTicket::new();
            self.entries.insert(id, Entry { ticket, handle: None, used: self.revision });
            self.tickets.insert(ticket, id);
            self.events.push_back(AssetChange::Request(AssetRequest {
                ticket,
                region,
                request: Request::Texture { id, size: TextureSize::new(TEXTURE_SIZE) },
                priority: Priority::Background,
            }));
        }
    }

    fn release(&mut self, id: AssetId) {
        if let Some(entry) = self.entries.remove(&id) {
            if self.tickets.remove(&entry.ticket).is_some() {
                self.events.push_back(AssetChange::Release(entry.ticket));
            }
            self.dirty.insert(id);
        }
    }

    pub(in crate::scene) fn event(&mut self) -> Option<AssetChange> {
        self.events.pop_front()
    }

    pub(in crate::scene) fn complete(&mut self, ticket: AssetTicket, status: &Status) -> Option<bool> {
        let id = self.tickets.get(&ticket).copied()?;
        let entry = self.entries.get_mut(&id)?;
        let handle = match status {
            Status::Ready(Content::Texture(handle))
            | Status::Refining(Content::Texture(handle))
            | Status::RefinementFailed { content: Content::Texture(handle), .. }
                if handle.asset() == id =>
            {
                Some(handle.clone())
            }
            Status::Failed { .. } | Status::StaleSession => None,
            Status::Ready(_) | Status::Refining(_) | Status::RefinementFailed { .. } => None,
            Status::Queued | Status::Fetching | Status::Decoding => return Some(false),
        };
        let changed = entry.handle != handle;
        if changed {
            entry.handle = handle;
            self.dirty.insert(id);
        }
        Some(changed)
    }

    pub(in crate::scene) fn take_dirty(&mut self) -> impl Iterator<Item = (AssetId, Option<TextureHandle>)> + '_ {
        let dirty = std::mem::take(&mut self.dirty);
        dirty.into_iter().map(|id| (id, self.entries.get(&id).and_then(|entry| entry.handle.clone())))
    }

    pub(in crate::scene) fn clear_dirty(&mut self) {
        self.dirty.clear();
    }

    pub(in crate::scene) fn reset(&mut self) {
        let ids = self.entries.keys().copied().collect::<Vec<_>>();
        for id in ids {
            self.release(id);
        }
        self.tickets.clear();
    }
}

fn extend_desired(desired: &mut BTreeMap<AssetId, Region>, ids: Vec<AssetId>, region: Region) {
    for id in ids {
        if desired.len() == MAX_TEXTURES {
            break;
        }
        desired.entry(id).or_insert(region);
    }
}

fn description_textures(description: &EnvironmentDescription) -> Vec<AssetId> {
    let mut ids = Vec::with_capacity(MAX_PER_ENVIRONMENT);
    match &description.settings {
        EnvironmentSettings::Sky(sky) => sky_textures(sky, &mut ids),
        EnvironmentSettings::Water(water) => water_textures(water, &mut ids),
        EnvironmentSettings::DayCycle(day) => {
            for track in day.skies() {
                for frame in track {
                    sky_textures(frame.settings(), &mut ids);
                }
            }
            for frame in day.water() {
                water_textures(frame.settings(), &mut ids);
            }
        }
    }
    ids
}

fn sky_textures(sky: &Sky, ids: &mut Vec<AssetId>) {
    add_resolved(ids, sky.sun.texture, DEFAULT_SUN_TEXTURE);
    add_resolved(ids, sky.moon.texture, DEFAULT_MOON_TEXTURE);
    add_resolved(ids, sky.clouds.texture, DEFAULT_CLOUD_TEXTURE);
    add_resolved(ids, sky.bloom, DEFAULT_BLOOM_TEXTURE);
    add_resolved(ids, sky.rainbow, DEFAULT_RAINBOW_TEXTURE);
    add_resolved(ids, sky.halo, DEFAULT_HALO_TEXTURE);
}

fn water_textures(water: &Water, ids: &mut Vec<AssetId>) {
    add_resolved(ids, water.normal_map, DEFAULT_WATER_NORMAL_MAP);
}

fn add_resolved(ids: &mut Vec<AssetId>, texture: TextureRef, default: Option<AssetId>) {
    let id = match texture {
        TextureRef::Absent => default,
        TextureRef::Asset(id) => Some(id),
        TextureRef::Clear => None,
    };
    if let Some(id) = id
        && ids.len() < MAX_PER_ENVIRONMENT
        && !ids.contains(&id)
    {
        ids.push(id);
    }
}

fn override_sky_textures(sky: &Sky) -> Vec<AssetId> {
    let mut ids = Vec::with_capacity(6);
    add_resolved(&mut ids, sky.sun.texture, DEFAULT_SUN_TEXTURE);
    add_resolved(&mut ids, sky.moon.texture, DEFAULT_MOON_TEXTURE);
    add_resolved(&mut ids, sky.clouds.texture, DEFAULT_CLOUD_TEXTURE);
    add_resolved(&mut ids, sky.bloom, DEFAULT_BLOOM_TEXTURE);
    add_resolved(&mut ids, sky.rainbow, DEFAULT_RAINBOW_TEXTURE);
    add_resolved(&mut ids, sky.halo, DEFAULT_HALO_TEXTURE);
    ids
}

fn override_water_textures(water: &Water) -> Vec<AssetId> {
    let mut ids = Vec::with_capacity(1);
    add_resolved(&mut ids, water.normal_map, DEFAULT_WATER_NORMAL_MAP);
    ids
}

fn property_region(properties: &BTreeMap<String, crate::scene::ExperienceProperty>) -> Option<Region> {
    properties.values().next().map(|property| property.region())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn asset(value: u128) -> AssetId {
        AssetId::new(Uuid::from_u128(value))
    }

    fn requested(manager: &mut EnvironmentTextures) -> Vec<AssetId> {
        std::iter::from_fn(|| manager.event())
            .filter_map(|event| match event {
                AssetChange::Request(request) => match request.request {
                    Request::Texture { id, .. } => Some(id),
                    Request::PbrMaterial(_)
                    | Request::LegacyMaterial(_)
                    | Request::Environment(_)
                    | Request::MeshHeader(_)
                    | Request::MeshLod(_) => None,
                },
                AssetChange::Release(_) => None,
            })
            .collect()
    }

    #[test]
    fn active_cycle_textures_are_requested_before_background_descriptions() {
        let mut manager = EnvironmentTextures::new();
        let active = BTreeMap::from([(asset(1), Region::new(1)), (asset(2), Region::new(1))]);

        manager.update(&BTreeMap::new(), None, &active);

        assert_eq!(requested(&mut manager), [asset(1), asset(2)]);
    }

    #[test]
    fn stale_textures_stay_resident_until_capacity_is_needed() {
        let mut manager = EnvironmentTextures::new();
        let region = Region::new(1);
        let first = BTreeMap::from([(asset(1), region)]);
        manager.update(&BTreeMap::new(), None, &first);
        let old_ticket = manager.entries[&asset(1)].ticket;
        requested(&mut manager);
        manager.update(&BTreeMap::new(), None, &BTreeMap::from([(asset(2), region)]));

        assert!(manager.entries.contains_key(&asset(1)));
        assert!(
            matches!(manager.event(), Some(AssetChange::Request(AssetRequest { request: Request::Texture { id, .. }, .. })) if id == asset(2))
        );
        assert!(manager.events.iter().all(|event| !matches!(event, AssetChange::Release(ticket) if *ticket == old_ticket)));
    }
}
