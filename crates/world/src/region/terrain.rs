use std::collections::{
    BTreeMap,
    Bound::{Excluded, Unbounded},
};

use crate::{ApplyError, Region, RegionApplied, RegionOutcome, UpdateSource, WorldReadError, WorldRevision, update::FieldStamp};
use crate::{World, WorldChange, WorldId, newer_field};

pub const TERRAIN_PATCH_SIZE: usize = 16;
pub const TERRAIN_PATCH_SAMPLES: usize = TERRAIN_PATCH_SIZE * TERRAIN_PATCH_SIZE;
pub const TERRAIN_REGION_SIZE: usize = 256;
pub const MAX_TERRAIN_PATCHES: usize = 65_536;
pub const MAX_TERRAIN_PAGE_PATCHES: usize = 8;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TerrainPatchId(u8);

impl TerrainPatchId {
    pub fn new(x: u8, y: u8) -> Option<Self> {
        if x < 16 && y < 16 { Some(Self(y * 16 + x)) } else { None }
    }
    pub fn coordinates(self) -> (u8, u8) {
        (self.0 % 16, self.0 / 16)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerrainPatchError {
    UnsupportedCoordinates,
    NonFinite,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerrainPatch {
    id: TerrainPatchId,
    heights: Box<[f32; TERRAIN_PATCH_SAMPLES]>,
}

impl TerrainPatch {
    pub fn new(x: u8, y: u8, heights: Box<[f32; TERRAIN_PATCH_SAMPLES]>) -> Result<Self, TerrainPatchError> {
        let id = TerrainPatchId::new(x, y).ok_or(TerrainPatchError::UnsupportedCoordinates)?;
        if !heights.iter().all(|height| height.is_finite()) {
            return Err(TerrainPatchError::NonFinite);
        }
        Ok(Self { id, heights })
    }
    pub fn id(&self) -> TerrainPatchId {
        self.id
    }
    pub fn heights(&self) -> &[f32; TERRAIN_PATCH_SAMPLES] {
        &self.heights
    }
}

struct ObservedPatch {
    patch: TerrainPatch,
    fence: FieldStamp,
}

#[derive(Default)]
pub(crate) struct Terrain {
    patches: BTreeMap<TerrainPatchId, ObservedPatch>,
    water: Option<(f32, FieldStamp)>,
}

impl Terrain {
    pub(crate) fn patch_count(&self) -> usize {
        self.patches.len()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerrainPageCursor {
    world: WorldId,
    region: Region,
    after: TerrainPatchId,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerrainPage {
    pub region: Region,
    pub hydration_revision: WorldRevision,
    pub dimensions: [u16; 2],
    pub known_patches: usize,
    pub water_height: Option<f32>,
    pub patches: Vec<TerrainPatch>,
    pub next: Option<TerrainPageCursor>,
}

impl World {
    pub fn observe_terrain_patch(&mut self, source: UpdateSource, patch: TerrainPatch) -> Result<RegionApplied, ApplyError> {
        let region = source.region();
        self.require_active_region(region)?;
        let Some(stamp) = source.stamp() else { return Ok(unchanged()) };
        let fence = FieldStamp { authority: source.authority(), stamp };
        let current = self.regions[&region].terrain.patches.get(&patch.id);
        if !newer_field(fence, current.map(|current| current.fence)) {
            return Ok(unchanged());
        }
        if current.is_some_and(|current| current.patch == patch) {
            self.regions.get_mut(&region).expect("active region").terrain.patches.get_mut(&patch.id).expect("known patch").fence = fence;
            return Ok(unchanged());
        }
        let new_patch = current.is_none();
        if new_patch && self.terrain_patches >= MAX_TERRAIN_PATCHES {
            return Err(ApplyError::TerrainBudgetExhausted);
        }
        self.ensure_revision_available()?;
        let id = patch.id;
        self.regions.get_mut(&region).expect("active region").terrain.patches.insert(id, ObservedPatch { patch, fence });
        self.terrain_patches += usize::from(new_patch);
        let revision = self.publish_changes(vec![WorldChange::TerrainPatchUpdated { region, patch: id }]).expect("changed patch");
        Ok(RegionApplied { outcome: RegionOutcome::Patched { revision }, recovery_cancellations: Vec::new() })
    }
    pub fn observe_water_height(&mut self, source: UpdateSource, height: f32) -> Result<RegionApplied, ApplyError> {
        let region = source.region();
        self.require_active_region(region)?;
        if !height.is_finite() {
            return Err(ApplyError::InvalidValue { field: "water_height" });
        }
        let Some(stamp) = source.stamp() else { return Ok(unchanged()) };
        let fence = FieldStamp { authority: source.authority(), stamp };
        let current = self.regions[&region].terrain.water;
        if !newer_field(fence, current.map(|(_, fence)| fence)) {
            return Ok(unchanged());
        }
        if current.is_some_and(|(current, _)| current == height) {
            self.regions.get_mut(&region).expect("active region").terrain.water = Some((height, fence));
            return Ok(unchanged());
        }
        self.ensure_revision_available()?;
        self.regions.get_mut(&region).expect("active region").terrain.water = Some((height, fence));
        let revision = self.publish_changes(vec![WorldChange::WaterHeightUpdated(region)]).expect("changed water height");
        Ok(RegionApplied { outcome: RegionOutcome::Patched { revision }, recovery_cancellations: Vec::new() })
    }
    pub fn terrain_page(
        &self,
        region: Region,
        cursor: Option<TerrainPageCursor>,
        limit: usize,
    ) -> Result<Option<TerrainPage>, WorldReadError> {
        if limit == 0 || limit > MAX_TERRAIN_PAGE_PATCHES {
            return Err(WorldReadError::InvalidLimit);
        }
        if let Some(cursor) = cursor {
            if cursor.world != self.id {
                return Err(WorldReadError::StaleSession);
            }
            if cursor.region != region {
                return Err(WorldReadError::InvalidCursor);
            }
        }
        let Some(state) = self.regions.get(&region) else { return Ok(None) };
        let start = cursor.map_or(Unbounded, |cursor| Excluded(cursor.after));
        let mut iter = state.terrain.patches.range((start, Unbounded));
        let mut patches = Vec::with_capacity(limit);
        for (_, observed) in iter.by_ref().take(limit) {
            patches.push(observed.patch.clone());
        }
        let next = if iter.next().is_some() {
            patches.last().map(|patch| TerrainPageCursor { world: self.id, region, after: patch.id })
        } else {
            None
        };
        Ok(Some(TerrainPage {
            region,
            hydration_revision: self.revision(),
            dimensions: [TERRAIN_REGION_SIZE as u16; 2],
            known_patches: state.terrain.patches.len(),
            water_height: state.terrain.water.map(|(height, _)| height),
            patches,
            next,
        }))
    }
}

fn unchanged() -> RegionApplied {
    RegionApplied { outcome: RegionOutcome::Unchanged, recovery_cancellations: Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OrderEvidence, RegionInfo, UpdateAuthority, WorldLimits};
    fn world() -> World {
        let mut world = World::new(WorldLimits {
            max_region_registrations: 3,
            max_entities: 8,
            max_entities_per_region: 8,
            max_tombstones_per_region: 8,
            max_tombstones: 8,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(8),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(8),
            max_journal_entries: 64,
            max_journal_changes: 128,
            max_region_name_bytes: 32,
            max_recovery_targets: 4,
        });
        let _ = world.adopt_region(Region::new(1)).unwrap();
        world
    }
    fn source(order: u32) -> UpdateSource {
        UpdateSource::new(Region::new(1), order, crate::OrderEvidence::Ordered(u64::from(order)), 0)
    }
    fn patch(x: u8, y: u8, height: f32) -> TerrainPatch {
        TerrainPatch::new(x, y, Box::new([height; TERRAIN_PATCH_SAMPLES])).unwrap()
    }
    fn page(world: &World) -> TerrainPage {
        world.terrain_page(Region::new(1), None, MAX_TERRAIN_PAGE_PATCHES).unwrap().unwrap()
    }
    #[test]
    fn unknown_coverage_and_water_remain_distinct_from_observed_zero() {
        let mut world = world();
        let before = page(&world);
        assert_eq!(before.dimensions, [256, 256]);
        assert_eq!(before.known_patches, 0);
        assert_eq!(before.water_height, None);
        assert!(before.patches.is_empty());
        let barrier = world.read_barrier();
        let _ = world.observe_terrain_patch(source(1), patch(3, 5, 0.0)).unwrap();
        let _ = world.observe_water_height(source(1), 0.0).unwrap();
        let after = page(&world);
        assert_eq!(after.known_patches, 1);
        assert_eq!(after.water_height, Some(0.0));
        assert_eq!(after.patches[0].id().coordinates(), (3, 5));
        assert_eq!(after.patches[0].heights(), &[0.0; TERRAIN_PATCH_SAMPLES]);
        let changes = world.change_page(barrier.changes, 8).unwrap();
        assert_eq!(
            changes.records.iter().map(|record| record.change).collect::<Vec<_>>(),
            [
                WorldChange::TerrainPatchUpdated { region: Region::new(1), patch: TerrainPatchId::new(3, 5).unwrap() },
                WorldChange::WaterHeightUpdated(Region::new(1)),
            ]
        );
        let _ = world.patch_region(Region::new(1), RegionInfo { name: Some("region".into()), ..RegionInfo::default() }).unwrap();
        assert_eq!(page(&world).patches, after.patches);
        assert_eq!(page(&world).water_height, after.water_height);
    }
    #[test]
    fn source_fences_handle_reordering_ordinals_authority_and_equal_values() {
        let mut world = world();
        let _ = world.observe_terrain_patch(source(10), patch(0, 0, 1.0)).unwrap();
        let _ = world.observe_water_height(source(10), 1.0).unwrap();
        let revision = world.revision();
        let _ = world.observe_terrain_patch(source(20), patch(0, 0, 1.0)).unwrap();
        let _ = world.observe_water_height(source(20), 1.0).unwrap();
        assert_eq!(world.revision(), revision);
        for source in [
            source(15),
            source(30).with_authority(UpdateAuthority::Untracked),
            UpdateSource::new(Region::new(1), 30, OrderEvidence::Ambiguous, 0),
            UpdateSource::new(Region::new(1), 30, OrderEvidence::Degraded, 0),
        ] {
            let _ = world.observe_terrain_patch(source, patch(0, 0, 2.0)).unwrap();
            let _ = world.observe_water_height(source, 2.0).unwrap();
        }
        assert_eq!(world.revision(), revision);
        let later_block = UpdateSource::new(Region::new(1), 20, OrderEvidence::Ordered(20), 1);
        let _ = world.observe_terrain_patch(later_block, patch(0, 0, 3.0)).unwrap();
        assert_eq!(page(&world).patches[0].heights()[0], 3.0);
        assert_eq!(world.terrain_patches, 1);
    }
    #[test]
    fn bounded_owned_pages_and_retirement_preserve_lifetime_authority() {
        let mut world = world();
        for x in 0..10 {
            let _ = world.observe_terrain_patch(source(u32::from(x) + 1), patch(x, 0, f32::from(x))).unwrap();
        }
        let first = page(&world);
        assert_eq!(first.patches.len(), 8);
        let next = world.terrain_page(Region::new(1), first.next, 8).unwrap().unwrap();
        assert_eq!(next.patches.len(), 2);
        assert_eq!(next.patches[0].id().coordinates(), (8, 0));
        assert!(next.next.is_none());
        assert_eq!(world.terrain_page(Region::new(2), first.next, 8), Err(WorldReadError::InvalidCursor));
        assert_eq!(self::world().terrain_page(Region::new(1), first.next, 8), Err(WorldReadError::StaleSession));
        assert_eq!(world.terrain_page(Region::new(1), None, 9), Err(WorldReadError::InvalidLimit));
        let _ = world.retire_region(Region::new(1)).unwrap();
        assert_eq!(world.terrain_patches, 0);
        assert_eq!(world.terrain_page(Region::new(1), first.next, 8), Ok(None));
        assert_eq!(world.observe_terrain_patch(source(100), patch(0, 0, 9.0)), Err(ApplyError::StaleRegion));
        assert_eq!(world.observe_water_height(source(100), 9.0), Err(ApplyError::StaleRegion));
        let _ = world.adopt_region(Region::new(2)).unwrap();
        assert_eq!(world.terrain_page(Region::new(2), None, 8).unwrap().unwrap().water_height, None);
        assert_eq!(first.patches[0].heights()[0], 0.0);
    }
    #[test]
    fn rejects_nonfinite_values_and_rolls_back_revision_exhaustion() {
        assert_eq!(TerrainPatch::new(16, 0, Box::new([0.0; TERRAIN_PATCH_SAMPLES])), Err(TerrainPatchError::UnsupportedCoordinates));
        assert_eq!(TerrainPatch::new(0, 0, Box::new([f32::NAN; TERRAIN_PATCH_SAMPLES])), Err(TerrainPatchError::NonFinite));
        let mut world = world();
        let _ = world.observe_terrain_patch(source(1), patch(0, 0, 1.0)).unwrap();
        assert!(world.observe_water_height(source(1), f32::INFINITY).is_err());
        world.journal.revision = WorldRevision(u64::MAX);
        assert_eq!(world.observe_terrain_patch(source(2), patch(0, 0, 2.0)), Err(ApplyError::RevisionExhausted));
        assert_eq!(world.observe_water_height(source(2), 2.0), Err(ApplyError::RevisionExhausted));
        assert_eq!(page(&world).patches[0].heights()[0], 1.0);
        assert_eq!(page(&world).water_height, None);
        assert_eq!(world.terrain_patches, 1);
    }
    #[test]
    fn global_capacity_allows_replacement_and_retirement_releases_admission() {
        let mut world = world();
        world.limits.max_region_registrations = 257;
        for region in 1..=256 {
            let region = Region::new(region);
            let _ = world.adopt_region(region).unwrap();
            let source = UpdateSource::new(region, 1, OrderEvidence::Ordered(1), 0);
            for y in 0..16 {
                for x in 0..16 {
                    let _ = world.observe_terrain_patch(source, patch(x, y, 0.0)).unwrap();
                }
            }
        }
        assert_eq!(world.terrain_patches, MAX_TERRAIN_PATCHES);
        let region = Region::new(257);
        let _ = world.adopt_region(region).unwrap();
        let source = UpdateSource::new(region, 1, OrderEvidence::Ordered(1), 0);
        assert_eq!(world.observe_terrain_patch(source, patch(0, 0, 1.0)), Err(ApplyError::TerrainBudgetExhausted));
        let _ = world.observe_terrain_patch(self::source(2), patch(0, 0, 2.0)).unwrap();
        assert_eq!(page(&world).patches[0].heights()[0], 2.0);
        let _ = world.retire_region(Region::new(1)).unwrap();
        let _ = world.observe_terrain_patch(source, patch(0, 0, 1.0)).unwrap();
        assert_eq!(world.terrain_patches, MAX_TERRAIN_PATCHES - 255);
    }
    proptest::proptest! {
        #[test]
        fn reordered_patch_sequences_match_newest_observation(
            updates in proptest::collection::vec((0u32..200, 0u8..16, -100i16..100), 0..100)
        ) {
            let mut world = world();
            let mut expected = BTreeMap::<u8, (u32, f32)>::new();
            for (order, x, height) in updates {
                let height = f32::from(height);
                let _ = world.observe_terrain_patch(source(order), patch(x, 0, height)).unwrap();
                if expected.get(&x).is_none_or(|(previous, _)| order > *previous) {
                    expected.insert(x, (order, height));
                }
                let mut cursor = None;
                let mut observed = BTreeMap::new();
                loop {
                    let page = world.terrain_page(Region::new(1), cursor, 8).unwrap().unwrap();
                    for patch in page.patches {
                        observed.insert(patch.id().coordinates().0, patch.heights()[0]);
                    }
                    cursor = page.next;
                    if cursor.is_none() { break; }
                }
                proptest::prop_assert_eq!(observed, expected.iter().map(|(x, (_, height))| (*x, *height)).collect());
                proptest::prop_assert_eq!(world.terrain_patches, expected.len());
            }
        }
    }
}
