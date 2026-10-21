use std::collections::{
    BTreeMap,
    Bound::{Excluded, Unbounded},
};

use super::{World, WorldChange, WorldId, newer_field};
use crate::{ApplyError, Region, RegionApplied, RegionOutcome, UpdateSource, WorldReadError, WorldRevision, update::FieldStamp};

#[cfg(test)]
#[path = "terrain_tests.rs"]
mod tests;

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
pub(super) struct Terrain {
    patches: BTreeMap<TerrainPatchId, ObservedPatch>,
    water: Option<(f32, FieldStamp)>,
}

impl Terrain {
    pub(super) fn patch_count(&self) -> usize {
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

fn unchanged() -> RegionApplied {
    RegionApplied { outcome: RegionOutcome::Unchanged, recovery_cancellations: Vec::new() }
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
