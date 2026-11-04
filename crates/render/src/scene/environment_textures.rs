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
    ticket: Option<AssetTicket>,
    region: Region,
    priority: Priority,
    handle: Option<TextureHandle>,
    used: u64,
}

#[derive(Clone, Copy)]
struct Desired {
    region: Region,
    priority: Priority,
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
        desired.extend(active.iter().map(|(id, region)| (*id, Desired { region: *region, priority: Priority::Visible })));
        if let Some(viewer) = viewer {
            for setting in [viewer.day(), viewer.sky(), viewer.water()].into_iter().flatten() {
                extend_desired(&mut desired, description_textures(setting.environment()), setting.region(), Priority::Background);
            }
            if let Some(local) = viewer.local() {
                extend_desired(&mut desired, description_textures(local.environment()), local.region(), Priority::Background);
            }
            if let Some(sky) = viewer.sky_overrides()
                && let Some(region) = property_region(viewer.sky_properties())
            {
                extend_desired(&mut desired, override_sky_textures(sky), region, Priority::Background);
            }
            if let Some(water) = viewer.water_overrides()
                && let Some(region) = property_region(viewer.water_properties())
            {
                extend_desired(&mut desired, override_water_textures(water), region, Priority::Background);
            }
        }
        if desired.len() < MAX_TEXTURES {
            for (region, environment) in environments {
                extend_desired(&mut desired, description_textures(environment.description()), *region, Priority::Background);
                if desired.len() >= MAX_TEXTURES {
                    break;
                }
            }
        }

        for (id, entry) in &mut self.entries {
            if !active.contains_key(id)
                && entry.priority == Priority::Visible
                && desired.get(id).is_none_or(|demand: &Desired| demand.region == entry.region)
            {
                entry.priority = Priority::Background;
                if let Some(ticket) = entry.ticket {
                    queue_change(&mut self.events, AssetChange::UpdatePriority { ticket, priority: Priority::Background });
                }
            }
        }

        let protected = desired.keys().copied().collect::<BTreeSet<_>>();
        for (id, demand) in desired {
            if self.entries.contains_key(&id) {
                self.update_existing(id, demand);
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
            self.request(id, demand);
        }
    }

    fn update_existing(&mut self, id: AssetId, demand: Desired) {
        let (old_ticket, needs_request, priority_update) = {
            let entry = self.entries.get_mut(&id).expect("desired texture entry exists");
            entry.used = self.revision;
            if entry.region != demand.region {
                let old_ticket = entry.ticket.take();
                entry.region = demand.region;
                entry.priority = demand.priority;
                (old_ticket, entry.handle.is_none(), None)
            } else if entry.priority != demand.priority {
                entry.priority = demand.priority;
                (None, false, entry.ticket.map(|ticket| (ticket, demand.priority)))
            } else {
                (None, false, None)
            }
        };
        if let Some(ticket) = old_ticket {
            self.tickets.remove(&ticket);
            queue_change(&mut self.events, AssetChange::Release(ticket));
        }
        if needs_request {
            self.request(id, demand);
        } else if let Some((ticket, priority)) = priority_update {
            queue_change(&mut self.events, AssetChange::UpdatePriority { ticket, priority });
        }
    }

    fn request(&mut self, id: AssetId, demand: Desired) {
        let ticket = AssetTicket::new();
        let entry = self.entries.entry(id).or_insert(Entry {
            ticket: None,
            region: demand.region,
            priority: demand.priority,
            handle: None,
            used: self.revision,
        });
        entry.ticket = Some(ticket);
        entry.region = demand.region;
        entry.priority = demand.priority;
        self.tickets.insert(ticket, id);
        queue_change(
            &mut self.events,
            AssetChange::Request(AssetRequest {
                ticket,
                region: demand.region,
                request: Request::Texture { id, size: TextureSize::new(TEXTURE_SIZE) },
                priority: demand.priority,
                importance: 0,
            }),
        );
    }

    fn release(&mut self, id: AssetId) {
        if let Some(entry) = self.entries.remove(&id) {
            if let Some(ticket) = entry.ticket
                && self.tickets.remove(&ticket).is_some()
            {
                queue_change(&mut self.events, AssetChange::Release(ticket));
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

fn queue_change(events: &mut VecDeque<AssetChange>, change: AssetChange) {
    match change {
        AssetChange::Release(ticket) => {
            let unissued = events.iter().any(|event| matches!(event, AssetChange::Request(request) if request.ticket == ticket));
            events.retain(|event| match event {
                AssetChange::Request(request) => request.ticket != ticket,
                AssetChange::UpdatePriority { ticket: pending, .. } => *pending != ticket,
                _ => true,
            });
            if !unissued {
                events.push_back(AssetChange::Release(ticket));
            }
        }
        AssetChange::UpdatePriority { ticket, priority } => {
            for event in events.iter_mut() {
                match event {
                    AssetChange::Request(request) if request.ticket == ticket => {
                        request.priority = priority;
                        return;
                    }
                    AssetChange::UpdatePriority { ticket: pending, priority: previous } if *pending == ticket => {
                        *previous = priority;
                        return;
                    }
                    _ => {}
                }
            }
            events.push_back(AssetChange::UpdatePriority { ticket, priority });
        }
        change => events.push_back(change),
    }
}

fn extend_desired(desired: &mut BTreeMap<AssetId, Desired>, ids: Vec<AssetId>, region: Region, priority: Priority) {
    for id in ids {
        if desired.len() == MAX_TEXTURES {
            break;
        }
        desired.entry(id).or_insert(Desired { region, priority });
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
    add_resolved_with_clear(ids, sky.bloom, DEFAULT_BLOOM_TEXTURE);
    add_resolved(ids, sky.rainbow, DEFAULT_RAINBOW_TEXTURE);
    add_resolved(ids, sky.halo, DEFAULT_HALO_TEXTURE);
}

fn water_textures(water: &Water, ids: &mut Vec<AssetId>) {
    add_resolved_with_clear(ids, water.normal_map, DEFAULT_WATER_NORMAL_MAP);
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

fn add_resolved_with_clear(ids: &mut Vec<AssetId>, texture: TextureRef, default: Option<AssetId>) {
    let id = match texture {
        TextureRef::Absent | TextureRef::Clear => default,
        TextureRef::Asset(id) => Some(id),
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
    add_resolved_with_clear(&mut ids, sky.bloom, DEFAULT_BLOOM_TEXTURE);
    add_resolved(&mut ids, sky.rainbow, DEFAULT_RAINBOW_TEXTURE);
    add_resolved(&mut ids, sky.halo, DEFAULT_HALO_TEXTURE);
    ids
}

fn override_water_textures(water: &Water) -> Vec<AssetId> {
    let mut ids = Vec::with_capacity(1);
    add_resolved_with_clear(&mut ids, water.normal_map, DEFAULT_WATER_NORMAL_MAP);
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
                AssetChange::Release(_)
                | AssetChange::UpdateImportance { .. }
                | AssetChange::UpdatePriority { .. }
                | AssetChange::UpdateSize { .. } => None,
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
    fn pending_demand_stays_bounded() {
        let mut manager = EnvironmentTextures::new();
        for value in 1..4096 {
            manager.update(&BTreeMap::new(), None, &BTreeMap::from([(asset(value), Region::new(1))]));
            assert!(manager.events.len() <= 2 * MAX_TEXTURES, "pending events: {}", manager.events.len());
        }
        let requests = requested(&mut manager);
        assert_eq!(requests.len(), MAX_TEXTURES);
        assert!(requests.iter().all(|id| manager.entries.contains_key(id)));
    }

    #[test]
    fn slow_delivery_preserves_leases() {
        let mut manager = EnvironmentTextures::new();
        let mut issued = std::collections::HashSet::new();
        let apply = |event, issued: &mut std::collections::HashSet<_>| match event {
            AssetChange::Request(request) => {
                assert!(issued.insert(request.ticket));
            }
            AssetChange::Release(ticket) => {
                assert!(issued.remove(&ticket));
            }
            AssetChange::UpdatePriority { ticket, .. } => {
                assert!(issued.contains(&ticket));
            }
            AssetChange::UpdateImportance { .. } | AssetChange::UpdateSize { .. } => unreachable!(),
        };
        for value in 1..4096 {
            manager.update(&BTreeMap::new(), None, &BTreeMap::from([(asset(value), Region::new(1))]));
            if value % 4 == 0
                && let Some(event) = manager.event()
            {
                apply(event, &mut issued);
            }
            assert!(manager.events.len() <= 3 * MAX_TEXTURES);
            assert!(issued.len() <= 2 * MAX_TEXTURES);
        }
        manager.reset();
        while let Some(event) = manager.event() {
            apply(event, &mut issued);
        }
        assert!(issued.is_empty());
    }

    #[test]
    fn nil_bloom_and_water_request_defaults() {
        let sky = EnvironmentDescription::decode(
            b"{'type':'sky','sun_id':u00000000-0000-0000-0000-000000000000,'moon_id':u00000000-0000-0000-0000-000000000000,'cloud_id':u00000000-0000-0000-0000-000000000000,'bloom_id':u00000000-0000-0000-0000-000000000000,'rainbow_id':u00000000-0000-0000-0000-000000000000,'halo_id':u00000000-0000-0000-0000-000000000000}",
        )
        .expect("nil bloom sky");
        let water = EnvironmentDescription::decode(b"{'type':'water','normal_map':u00000000-0000-0000-0000-000000000000}")
            .expect("nil normal water");
        assert_eq!(description_textures(&sky), [DEFAULT_BLOOM_TEXTURE.expect("default bloom texture")]);
        assert_eq!(description_textures(&water), [DEFAULT_WATER_NORMAL_MAP.expect("default water normal")]);
    }

    #[test]
    fn stale_textures_stay_resident_until_capacity_is_needed() {
        let mut manager = EnvironmentTextures::new();
        let region = Region::new(1);
        let first = BTreeMap::from([(asset(1), region)]);
        manager.update(&BTreeMap::new(), None, &first);
        let old_ticket = manager.entries[&asset(1)].ticket.expect("active request ticket");
        requested(&mut manager);
        manager.update(&BTreeMap::new(), None, &BTreeMap::from([(asset(2), region)]));

        assert!(manager.entries.contains_key(&asset(1)));
        assert!(
            matches!(manager.event(), Some(AssetChange::UpdatePriority { ticket, priority: Priority::Background }) if ticket == old_ticket)
        );
        assert!(
            matches!(manager.event(), Some(AssetChange::Request(AssetRequest { request: Request::Texture { id, .. }, .. })) if id == asset(2))
        );
        assert!(manager.events.iter().all(|event| !matches!(event, AssetChange::Release(ticket) if *ticket == old_ticket)));
    }

    #[test]
    fn priority_change_keeps_texture_ticket() {
        let mut manager = EnvironmentTextures::new();
        let id = asset(3);
        let region = Region::new(1);
        manager.request(id, Desired { region, priority: Priority::Background });
        let Some(AssetChange::Request(request)) = manager.event() else { panic!("background texture request") };
        manager.update_existing(id, Desired { region, priority: Priority::Visible });
        assert!(
            matches!(manager.event(), Some(AssetChange::UpdatePriority { ticket, priority: Priority::Visible }) if ticket == request.ticket)
        );
        manager.update_existing(id, Desired { region, priority: Priority::Background });
        assert!(
            matches!(manager.event(), Some(AssetChange::UpdatePriority { ticket, priority: Priority::Background }) if ticket == request.ticket)
        );
        assert!(manager.event().is_none());
    }

    #[test]
    fn stale_active_demand_downgrades() {
        let mut manager = EnvironmentTextures::new();
        let id = asset(6);
        let region = Region::new(1);
        manager.request(id, Desired { region, priority: Priority::Visible });
        let Some(AssetChange::Request(request)) = manager.event() else { panic!("active texture request") };
        manager.update(&BTreeMap::new(), None, &BTreeMap::new());
        assert!(
            matches!(manager.event(), Some(AssetChange::UpdatePriority { ticket, priority: Priority::Background }) if ticket == request.ticket)
        );
    }

    #[test]
    fn route_change_replaces_incomplete_demand() {
        let mut manager = EnvironmentTextures::new();
        let id = asset(4);
        let first_region = Region::new(1);
        let second_region = Region::new(2);
        manager.request(id, Desired { region: first_region, priority: Priority::Background });
        let Some(AssetChange::Request(first)) = manager.event() else { panic!("first texture request") };
        manager.update_existing(id, Desired { region: second_region, priority: Priority::Background });
        assert!(matches!(manager.event(), Some(AssetChange::Release(ticket)) if ticket == first.ticket));
        let Some(AssetChange::Request(second)) = manager.event() else { panic!("route texture request") };
        assert_eq!(second.region, second_region);
        assert_ne!(first.ticket, second.ticket);
        assert!(manager.complete(first.ticket, &Status::Fetching).is_none());
    }

    #[test]
    fn route_change_keeps_ready_handle() {
        let mut manager = EnvironmentTextures::new();
        let id = asset(5);
        let first_region = Region::new(1);
        let second_region = Region::new(2);
        manager.request(id, Desired { region: first_region, priority: Priority::Background });
        let Some(AssetChange::Request(request)) = manager.event() else { panic!("texture request") };
        let handle = loaded_texture(id);
        assert_eq!(manager.complete(request.ticket, &Status::Ready(Content::Texture(handle.clone()))), Some(true));
        manager.update_existing(id, Desired { region: second_region, priority: Priority::Background });
        assert!(matches!(manager.event(), Some(AssetChange::Release(ticket)) if ticket == request.ticket));
        assert!(manager.event().is_none());
        let entry = manager.entries.get(&id).expect("ready entry is retained");
        assert_eq!(entry.region, second_region);
        assert_eq!(entry.handle.as_ref(), Some(&handle));
        assert!(manager.complete(request.ticket, &Status::Fetching).is_none());
    }

    fn loaded_texture(id: AssetId) -> TextureHandle {
        use std::time::Instant;

        use assets::{Budget, LoadJob, Priority, Resource, Store};

        let mut store = Store::new(Budget::default());
        let route = store.register_route().expect("texture test route");
        let now = Instant::now();
        let request = Request::new(Resource { id, kind: assets::Kind::Texture });
        let demand = store.request(route, request, Priority::Visible, now).expect("texture test demand");
        let LoadJob::Fetch(fetch) = store.next_job(now).expect("texture fetch") else { panic!("fresh texture request") };
        let input = fetch.complete(Ok(test_support::texture::GRAY_ALPHA_J2C.to_vec())).expect("texture response");
        let decode = store.start_decode(input).expect("texture decode");
        store.complete(decode.run(), now);
        let Status::Ready(Content::Texture(handle)) = demand.status() else { panic!("decoded texture") };
        handle
    }
}
