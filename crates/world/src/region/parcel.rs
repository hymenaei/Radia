use std::{fmt, mem::size_of, sync::Arc};

use math::Vec3;
use uuid::Uuid;

use super::environment::ParcelLocalId;
use crate::{
    ApplyError, Region, RegionApplied, RegionOutcome, UpdateSource, World, WorldChange, WorldRevision, newer_field, update::FieldStamp,
};

pub const PARCEL_GRID_EDGE: usize = 64;
pub const PARCEL_OVERLAY_CELLS: usize = PARCEL_GRID_EDGE * PARCEL_GRID_EDGE;
pub const PARCEL_OVERLAY_PARTS: usize = 4;
pub const PARCEL_OVERLAY_PART_BYTES: usize = PARCEL_OVERLAY_CELLS / PARCEL_OVERLAY_PARTS;
pub const PARCEL_BITMAP_BYTES: usize = PARCEL_OVERLAY_CELLS / 8;
pub const MAX_PARCEL_NAME_BYTES: usize = 1024;
pub const MAX_PARCEL_DESCRIPTION_BYTES: usize = 4096;
pub const MAX_PARCEL_URL_BYTES: usize = 2048;
pub const MAX_PARCEL_REGION_BYTES: usize = 24 * 1024;
pub const MAX_PARCEL_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_PARCEL_READ_BYTES: usize = size_of::<ParcelRead>()
    + size_of::<CurrentParcel>()
    + size_of::<ParcelPropertiesData>()
    + PARCEL_BITMAP_BYTES
    + MAX_PARCEL_NAME_BYTES
    + MAX_PARCEL_DESCRIPTION_BYTES
    + 2 * MAX_PARCEL_URL_BYTES
    + PARCEL_OVERLAY_CELLS;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ParcelFlags(u32);

impl ParcelFlags {
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn allow_fly(self) -> bool {
        self.0 & (1 << 0) != 0
    }

    pub const fn allow_other_scripts(self) -> bool {
        self.0 & (1 << 1) != 0
    }

    pub const fn allow_build(self) -> bool {
        self.0 & ((1 << 6) | (1 << 26)) != 0
    }

    pub const fn group_access(self) -> bool {
        self.0 & (1 << 8) != 0
    }

    pub const fn use_access_list(self) -> bool {
        self.0 & (1 << 9) != 0
    }

    pub const fn use_ban_list(self) -> bool {
        self.0 & (1 << 10) != 0
    }

    pub const fn use_pass_list(self) -> bool {
        self.0 & (1 << 11) != 0
    }

    pub const fn sound_local(self) -> bool {
        self.0 & (1 << 15) != 0
    }

    pub const fn restrict_push(self) -> bool {
        self.0 & (1 << 21) != 0
    }

    pub const fn deny_anonymous(self) -> bool {
        self.0 & (1 << 22) != 0
    }

    pub const fn deny_age_unverified(self) -> bool {
        self.0 & (1 << 31) != 0
    }

    pub const fn allow_group_scripts(self) -> bool {
        self.0 & (1 << 25) != 0
    }

    pub const fn allow_damage(self) -> bool {
        self.0 & (1 << 5) != 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParcelBitmap(Arc<[u8; PARCEL_BITMAP_BYTES]>);

impl ParcelBitmap {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ApplyError> {
        let bytes: [u8; PARCEL_BITMAP_BYTES] = bytes.try_into().map_err(|_| ApplyError::InvalidValue { field: "parcel_bitmap" })?;
        Ok(Self(Arc::new(bytes)))
    }

    pub fn contains_cell(&self, x: usize, y: usize) -> bool {
        if x >= PARCEL_GRID_EDGE || y >= PARCEL_GRID_EDGE {
            return false;
        }
        let bit = x + y * PARCEL_GRID_EDGE;
        self.0.get(bit / 8).is_some_and(|byte| byte & (1 << (bit % 8)) != 0)
    }
}

#[derive(Clone, PartialEq)]
pub struct ParcelPropertiesData {
    pub local_id: ParcelLocalId,
    pub name: Arc<str>,
    pub description: Arc<str>,
    pub owner_id: Option<Uuid>,
    pub group_id: Option<Uuid>,
    pub group_owned: bool,
    pub area: u32,
    pub flags: ParcelFlags,
    pub bitmap: ParcelBitmap,
    pub landing_point: Option<Vec3>,
    pub landing_look_at: Option<Vec3>,
    pub landing_type: Option<u8>,
    pub music_url: Arc<str>,
    pub media_url: Arc<str>,
    pub access: ParcelAccessSettings,
}

impl fmt::Debug for ParcelPropertiesData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParcelPropertiesData")
            .field("local_id", &self.local_id)
            .field("name", &self.name)
            .field("description", &self.description)
            .field("owner_id", &self.owner_id)
            .field("group_id", &self.group_id)
            .field("group_owned", &self.group_owned)
            .field("area", &self.area)
            .field("flags", &self.flags)
            .field("landing_point", &self.landing_point)
            .field("landing_look_at", &self.landing_look_at)
            .field("landing_type", &self.landing_type)
            .field("music_url", &"<redacted>")
            .field("media_url", &"<redacted>")
            .field("access", &self.access)
            .finish()
    }
}

impl ParcelPropertiesData {
    fn validate(&self) -> Result<(), ApplyError> {
        if self.name.len() > MAX_PARCEL_NAME_BYTES {
            return Err(ApplyError::InvalidValue { field: "parcel_name" });
        }
        if self.description.len() > MAX_PARCEL_DESCRIPTION_BYTES {
            return Err(ApplyError::InvalidValue { field: "parcel_description" });
        }
        if self.music_url.len() > MAX_PARCEL_URL_BYTES {
            return Err(ApplyError::InvalidValue { field: "parcel_music_url" });
        }
        if self.media_url.len() > MAX_PARCEL_URL_BYTES {
            return Err(ApplyError::InvalidValue { field: "parcel_media_url" });
        }
        if self.area > 65_536 {
            return Err(ApplyError::InvalidValue { field: "parcel_area" });
        }
        if self.landing_point.is_some_and(|point| !point.is_finite()) {
            return Err(ApplyError::InvalidValue { field: "parcel_landing_point" });
        }
        if self.landing_look_at.is_some_and(|point| !point.is_finite()) {
            return Err(ApplyError::InvalidValue { field: "parcel_landing_look_at" });
        }
        if self.access.pass_hours.is_some_and(|hours| !hours.is_finite() || hours < 0.0) {
            return Err(ApplyError::InvalidValue { field: "parcel_pass_hours" });
        }
        Ok(())
    }

    fn retained_bytes(&self) -> usize {
        size_of::<Self>() + PARCEL_BITMAP_BYTES + self.name.len() + self.description.len() + self.music_url.len() + self.media_url.len()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParcelAccessSettings {
    pub region_push_override: Option<bool>,
    pub region_deny_anonymous: Option<bool>,
    pub region_deny_identified: Option<bool>,
    pub region_deny_transacted: Option<bool>,
    pub region_deny_age_unverified: Option<bool>,
    pub region_allow_access_override: Option<bool>,
    pub pass_price: Option<u32>,
    pub pass_hours: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParcelPropertiesObservation {
    pub request_result: i32,
    pub sequence_id: i32,
    pub properties: Option<ParcelPropertiesData>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CurrentParcel {
    pub sequence_id: u32,
    pub properties: ParcelPropertiesData,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParcelOwnership {
    Public,
    Owned,
    Group,
    SelfOwned,
    ForSale,
    Auction,
    Unknown(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParcelOverlayCell {
    pub ownership: ParcelOwnership,
    pub hidden_avatars: bool,
    pub sound_local: bool,
    pub west_boundary: bool,
    pub south_boundary: bool,
    pub reserved_bit: bool,
}

impl ParcelOverlayCell {
    fn decode(raw: u8) -> Self {
        let ownership = match raw & 0x07 {
            0 => ParcelOwnership::Public,
            1 => ParcelOwnership::Owned,
            2 => ParcelOwnership::Group,
            3 => ParcelOwnership::SelfOwned,
            4 => ParcelOwnership::ForSale,
            5 => ParcelOwnership::Auction,
            code => ParcelOwnership::Unknown(code),
        };
        Self {
            ownership,
            hidden_avatars: raw & 0x10 != 0,
            sound_local: raw & 0x20 != 0,
            west_boundary: raw & 0x40 != 0,
            south_boundary: raw & 0x80 != 0,
            reserved_bit: raw & 0x08 != 0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParcelOverlayGrid(Arc<[u8; PARCEL_OVERLAY_CELLS]>);

impl ParcelOverlayGrid {
    pub fn cell(&self, x: usize, y: usize) -> Option<ParcelOverlayCell> {
        let index = x.checked_add(y.checked_mul(PARCEL_GRID_EDGE)?)?;
        self.0.get(index).copied().filter(|_| x < PARCEL_GRID_EDGE && y < PARCEL_GRID_EDGE).map(ParcelOverlayCell::decode)
    }

    pub fn raw_cell(&self, x: usize, y: usize) -> Option<u8> {
        let index = x.checked_add(y.checked_mul(PARCEL_GRID_EDGE)?)?;
        self.0.get(index).copied().filter(|_| x < PARCEL_GRID_EDGE && y < PARCEL_GRID_EDGE)
    }

    fn retained_bytes(&self) -> usize {
        PARCEL_OVERLAY_CELLS
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParcelOverlayObservation {
    pub sequence_id: i32,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RegionFlags {
    pub low: Option<u32>,
    pub extended: Option<u64>,
}

impl RegionFlags {
    fn bits(self) -> Option<u64> {
        match (self.low, self.extended) {
            (Some(low), Some(extended)) => Some(u64::from(low) | extended),
            (Some(low), None) => Some(u64::from(low)),
            (None, Some(extended)) => Some(extended),
            (None, None) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EffectiveRestrictions {
    pub allow_fly: Option<bool>,
    pub allow_build: Option<bool>,
    pub allow_scripts: Option<bool>,
    pub allow_group_scripts: Option<bool>,
    pub restrict_push: Option<bool>,
    pub allow_damage: Option<bool>,
    pub sound_local: Option<bool>,
    pub avatars_hidden_outside: Option<bool>,
    pub allow_direct_teleport: Option<bool>,
    pub deny_anonymous: Option<bool>,
    pub deny_identified: Option<bool>,
    pub deny_transacted: Option<bool>,
    pub deny_age_unverified: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParcelRead {
    pub region: Region,
    pub hydration_revision: WorldRevision,
    pub current: Option<CurrentParcel>,
    pub overlay: Option<ParcelOverlayGrid>,
    pub region_flags: RegionFlags,
    pub effective_restrictions: EffectiveRestrictions,
}

impl ParcelRead {
    pub fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.current.as_ref().map_or(0, |current| size_of::<CurrentParcel>() + current.properties.retained_bytes())
            + self.overlay.as_ref().map_or(0, ParcelOverlayGrid::retained_bytes)
    }
}

#[derive(Default)]
pub(crate) struct ParcelState {
    current: Option<CurrentParcel>,
    last_agent_sequence: Option<u32>,
    agent_fence: Option<FieldStamp>,
    overlay: OverlayAssembly,
    region_flags: RegionFlags,
    region_low_fence: Option<FieldStamp>,
    region_extended_fence: Option<FieldStamp>,
}

impl ParcelState {
    pub(crate) fn retained_bytes(&self) -> usize {
        self.current.as_ref().map_or(0, |current| current.properties.retained_bytes()) + self.overlay.retained_bytes()
    }

    #[cfg(test)]
    pub(crate) fn invariants_hold(&self) -> bool {
        let present = self.overlay.present;
        let staged_parts =
            self.overlay.staging_fences.iter().enumerate().fold(0_u8, |bits, (index, fence)| bits | (u8::from(fence.is_some()) << index));
        let current_sequence_valid =
            self.current.as_ref().is_none_or(|current| self.last_agent_sequence.is_some_and(|sequence| sequence >= current.sequence_id));
        (present == 0) == self.overlay.staging.is_none()
            && present == staged_parts
            && current_sequence_valid
            && self.retained_bytes() <= MAX_PARCEL_REGION_BYTES
    }

    fn effective_restrictions(&self) -> EffectiveRestrictions {
        let Some(current) = self.current.as_ref() else { return EffectiveRestrictions::default() };
        let parcel = &current.properties;
        let region = self.region_flags;
        let bits = region.bits();
        let mut restrictions = EffectiveRestrictions {
            allow_fly: bits.map(|flags| flags & (1 << 19) == 0 && parcel.flags.allow_fly()),
            allow_build: Some(parcel.flags.allow_build()),
            allow_scripts: bits.map(|flags| flags & ((1 << 13) | (1 << 21)) == 0 && parcel.flags.allow_other_scripts()),
            allow_group_scripts: bits.map(|flags| flags & ((1 << 13) | (1 << 21)) == 0 && parcel.flags.allow_group_scripts()),
            restrict_push: bits.map(|flags| {
                let region_restricts = flags & (1 << 22) != 0;
                region_restricts && parcel.access.region_push_override != Some(true) || parcel.flags.restrict_push()
            }),
            allow_damage: combine_or(bits.map(|flags| flags & 1 != 0), Some(parcel.flags.allow_damage())),
            sound_local: Some(parcel.flags.sound_local()),
            avatars_hidden_outside: None,
            allow_direct_teleport: bits.map(|flags| flags & (1 << 20) != 0),
            deny_anonymous: combine_or(
                combine_or(bits.map(|flags| flags & (1 << 23) != 0), Some(parcel.flags.deny_anonymous())),
                parcel.access.region_deny_anonymous,
            ),
            deny_identified: parcel.access.region_deny_identified,
            deny_transacted: parcel.access.region_deny_transacted,
            deny_age_unverified: combine_or(
                combine_or(bits.map(|flags| flags & (1 << 30) != 0), Some(parcel.flags.deny_age_unverified())),
                parcel.access.region_deny_age_unverified,
            ),
        };
        if let Some(overlay) = self.overlay.complete.as_ref() {
            let mut hidden = false;
            let mut local_sound = false;
            for y in 0..PARCEL_GRID_EDGE {
                for x in 0..PARCEL_GRID_EDGE {
                    if !parcel.bitmap.contains_cell(x, y) {
                        continue;
                    }
                    if let Some(cell) = overlay.cell(x, y) {
                        hidden |= cell.hidden_avatars;
                        local_sound |= cell.sound_local;
                    }
                }
            }
            restrictions.avatars_hidden_outside = Some(hidden);
            restrictions.sound_local = Some(restrictions.sound_local == Some(true) || local_sound);
        }
        restrictions
    }
}

#[derive(Default)]
struct OverlayAssembly {
    complete: Option<ParcelOverlayGrid>,
    staging: Option<Box<[u8; PARCEL_OVERLAY_CELLS]>>,
    present: u8,
    staging_fences: [Option<FieldStamp>; PARCEL_OVERLAY_PARTS],
    complete_fences: [Option<FieldStamp>; PARCEL_OVERLAY_PARTS],
}

impl OverlayAssembly {
    fn retained_bytes(&self) -> usize {
        self.complete.as_ref().map_or(0, ParcelOverlayGrid::retained_bytes) + self.staging.as_ref().map_or(0, |_| PARCEL_OVERLAY_CELLS)
    }
}

impl World {
    pub fn parcel_state(&self, region: Region) -> Option<ParcelRead> {
        let parcels = &self.regions.get(&region)?.parcels;
        Some(ParcelRead {
            region,
            hydration_revision: self.revision(),
            current: parcels.current.clone(),
            overlay: parcels.overlay.complete.clone(),
            region_flags: parcels.region_flags,
            effective_restrictions: parcels.effective_restrictions(),
        })
    }

    pub fn observe_parcel_overlay(
        &mut self,
        source: UpdateSource,
        observation: ParcelOverlayObservation,
    ) -> Result<RegionApplied, ApplyError> {
        let region = source.region();
        self.require_active_region(region)?;
        let index = usize::try_from(observation.sequence_id)
            .ok()
            .filter(|index| *index < PARCEL_OVERLAY_PARTS)
            .ok_or(ApplyError::InvalidValue { field: "parcel_overlay_index" })?;
        if observation.data.len() != PARCEL_OVERLAY_PART_BYTES {
            return Err(ApplyError::InvalidValue { field: "parcel_overlay_size" });
        }
        let Some(stamp) = source.stamp() else { return Ok(RegionApplied::unchanged()) };
        let fence = FieldStamp { authority: source.authority(), stamp };
        let parcels = &self.regions[&region].parcels;
        let assembly = &parcels.overlay;
        if !newer_field(fence, assembly.complete_fences[index]) {
            return Ok(RegionApplied::unchanged());
        }
        if assembly.present & (1 << index) != 0 && !newer_field(fence, assembly.staging_fences[index]) {
            return Ok(RegionApplied::unchanged());
        }

        let restart = assembly.present & (1 << index) != 0;
        let old_bytes = parcels.retained_bytes();
        let next_bytes = if assembly.staging.is_none() { old_bytes.saturating_add(PARCEL_OVERLAY_CELLS) } else { old_bytes };
        if next_bytes > MAX_PARCEL_REGION_BYTES || self.parcel_bytes.saturating_sub(old_bytes).saturating_add(next_bytes) > MAX_PARCEL_BYTES
        {
            return Err(ApplyError::ParcelBudgetExhausted);
        }
        let mut preserved = 0_u8;
        if restart {
            for part in 0..PARCEL_OVERLAY_PARTS {
                let bit = 1 << part;
                if part != index
                    && assembly.present & bit != 0
                    && assembly.staging_fences[part].is_some_and(|staged| newer_field(staged, Some(fence)))
                {
                    preserved |= bit;
                }
            }
        }
        let present = (if restart { preserved } else { assembly.present }) | (1 << index);
        let will_publish = present == (1 << PARCEL_OVERLAY_PARTS) - 1;
        if will_publish {
            self.ensure_revision_available()?;
        }
        let publish = {
            let assembly = &mut self.regions.get_mut(&region).expect("active region").parcels.overlay;
            let mut staging = match assembly.staging.take() {
                Some(staging) => staging,
                None => {
                    assembly.present = 0;
                    assembly.staging_fences = [None; PARCEL_OVERLAY_PARTS];
                    Box::new([0; PARCEL_OVERLAY_CELLS])
                }
            };
            if restart {
                for part in 0..PARCEL_OVERLAY_PARTS {
                    let bit = 1 << part;
                    if part != index && assembly.present & bit != 0 && preserved & bit == 0 {
                        let start = part * PARCEL_OVERLAY_PART_BYTES;
                        staging[start..start + PARCEL_OVERLAY_PART_BYTES].fill(0);
                        assembly.present &= !bit;
                        assembly.staging_fences[part] = None;
                    }
                }
            }
            let offset = index * PARCEL_OVERLAY_PART_BYTES;
            staging[offset..offset + PARCEL_OVERLAY_PART_BYTES].copy_from_slice(&observation.data);
            assembly.staging_fences[index] = Some(fence);
            assembly.present |= 1 << index;
            if assembly.present == (1 << PARCEL_OVERLAY_PARTS) - 1 {
                assembly.complete = Some(ParcelOverlayGrid(Arc::from(staging)));
                assembly.complete_fences = assembly.staging_fences;
                assembly.staging_fences = [None; PARCEL_OVERLAY_PARTS];
                assembly.present = 0;
                true
            } else {
                assembly.staging = Some(staging);
                false
            }
        };
        let new_bytes = self.regions[&region].parcels.retained_bytes();
        self.parcel_bytes = self.parcel_bytes - old_bytes + new_bytes;
        if !publish {
            return Ok(RegionApplied::unchanged());
        }
        Ok(self.publish_region_patch(WorldChange::ParcelUpdated(region)))
    }

    pub fn observe_parcel_properties(
        &mut self,
        source: UpdateSource,
        observation: ParcelPropertiesObservation,
    ) -> Result<RegionApplied, ApplyError> {
        let region = source.region();
        self.require_active_region(region)?;
        if observation.request_result == -1 {
            return Ok(RegionApplied::unchanged());
        }
        if !(0..=1).contains(&observation.request_result) {
            return Err(ApplyError::InvalidValue { field: "parcel_request_result" });
        }
        if observation.sequence_id < 0 {
            return Ok(RegionApplied::unchanged());
        }
        let sequence_id = u32::try_from(observation.sequence_id).map_err(|_| ApplyError::InvalidValue { field: "parcel_sequence_id" })?;
        let Some(stamp) = source.stamp() else { return Ok(RegionApplied::unchanged()) };
        let fence = FieldStamp { authority: source.authority(), stamp };
        let current = &self.regions[&region].parcels;
        let newer_sequence = sequence_id > 0 && current.last_agent_sequence.is_some_and(|last| sequence_id > last);
        let stale_sequence = sequence_id > 0 && current.last_agent_sequence.is_some_and(|last| sequence_id <= last);
        if stale_sequence || (!newer_sequence && !newer_field(fence, current.agent_fence)) {
            return Ok(RegionApplied::unchanged());
        }
        let properties = observation.properties.ok_or(ApplyError::InvalidValue { field: "parcel_properties" })?;
        properties.validate()?;
        let old_bytes = current.retained_bytes();
        let new_property_bytes = properties.retained_bytes();
        let next_bytes = old_bytes - current.current.as_ref().map_or(0, |current| current.properties.retained_bytes()) + new_property_bytes;
        if next_bytes > MAX_PARCEL_REGION_BYTES || self.parcel_bytes.saturating_sub(old_bytes).saturating_add(next_bytes) > MAX_PARCEL_BYTES
        {
            return Err(ApplyError::ParcelBudgetExhausted);
        }
        let selection_changed = self.regions[&region].environment.selected_parcel() != Some(properties.local_id);
        let environment_changed =
            self.agent_parcel_selection_changes(region, properties.local_id, fence) || (newer_sequence && selection_changed);
        let properties_changed =
            current.current.as_ref().is_none_or(|current| current.sequence_id != sequence_id || current.properties != properties);
        if !environment_changed && !properties_changed {
            let current = &mut self.regions.get_mut(&region).expect("active region").parcels;
            current.agent_fence = Some(fence);
            current.last_agent_sequence = Some(current.last_agent_sequence.map_or(sequence_id, |last| last.max(sequence_id)));
            return Ok(RegionApplied::unchanged());
        }
        self.ensure_revision_available()?;
        let revision = WorldRevision(self.revision().get() + 1);
        let environment_changed =
            self.select_agent_parcel_at_revision(region, Some(properties.local_id), Some(fence), newer_sequence, revision);
        let parcels = &mut self.regions.get_mut(&region).expect("active region").parcels;
        parcels.current = Some(CurrentParcel { sequence_id, properties });
        parcels.agent_fence = Some(fence);
        parcels.last_agent_sequence = Some(parcels.last_agent_sequence.map_or(sequence_id, |last| last.max(sequence_id)));
        self.parcel_bytes = self.parcel_bytes - old_bytes + next_bytes;
        let mut changes = Vec::with_capacity(2);
        changes.push(WorldChange::ParcelUpdated(region));
        if environment_changed {
            changes.push(WorldChange::EnvironmentUpdated(region));
        }
        let _ = self.publish_changes(changes);
        Ok(RegionApplied::new(RegionOutcome::Patched { revision }))
    }

    pub fn observe_region_flags(
        &mut self,
        source: UpdateSource,
        low: Option<u32>,
        extended: Option<u64>,
    ) -> Result<RegionApplied, ApplyError> {
        let region = source.region();
        self.require_active_region(region)?;
        if low.is_none() && extended.is_none() {
            return Ok(RegionApplied::unchanged());
        }
        let Some(stamp) = source.stamp() else { return Ok(RegionApplied::unchanged()) };
        let fence = FieldStamp { authority: source.authority(), stamp };
        let current = &self.regions[&region].parcels;
        let low_changed = low.is_some_and(|flags| newer_field(fence, current.region_low_fence) && current.region_flags.low != Some(flags));
        let extended_changed =
            extended.is_some_and(|flags| newer_field(fence, current.region_extended_fence) && current.region_flags.extended != Some(flags));
        if !low_changed && !extended_changed {
            let current = &mut self.regions.get_mut(&region).expect("active region").parcels;
            if low.is_some_and(|_| newer_field(fence, current.region_low_fence)) {
                current.region_low_fence = Some(fence);
            }
            if extended.is_some_and(|_| newer_field(fence, current.region_extended_fence)) {
                current.region_extended_fence = Some(fence);
            }
            return Ok(RegionApplied::unchanged());
        }
        self.ensure_revision_available()?;
        let current = &mut self.regions.get_mut(&region).expect("active region").parcels;
        if low.is_some_and(|_| newer_field(fence, current.region_low_fence)) {
            current.region_flags.low = low;
            current.region_low_fence = Some(fence);
        }
        if extended.is_some_and(|_| newer_field(fence, current.region_extended_fence)) {
            current.region_flags.extended = extended;
            current.region_extended_fence = Some(fence);
        }
        Ok(self.publish_region_patch(WorldChange::ParcelUpdated(region)))
    }

    pub fn clear_agent_parcel(&mut self, region: Region) -> Result<RegionApplied, ApplyError> {
        self.require_active_region(region)?;
        let parcels = &self.regions[&region].parcels;
        let had_current = parcels.current.is_some();
        let had_selection = self.regions[&region].environment.selected_parcel().is_some();
        if !had_current && !had_selection {
            return Ok(RegionApplied::unchanged());
        }
        self.ensure_revision_available()?;
        let revision = WorldRevision(self.revision().get() + 1);
        let old_bytes = parcels.retained_bytes();
        let selected_changed = self.select_agent_parcel_at_revision(region, None, None, false, revision);
        let parcels = &mut self.regions.get_mut(&region).expect("active region").parcels;
        parcels.current = None;
        let new_bytes = parcels.retained_bytes();
        self.parcel_bytes = self.parcel_bytes - old_bytes + new_bytes;
        let mut changes = Vec::with_capacity(2);
        if had_current {
            changes.push(WorldChange::ParcelUpdated(region));
        }
        if selected_changed {
            changes.push(WorldChange::EnvironmentUpdated(region));
        }
        let _ = self.publish_changes(changes);
        Ok(RegionApplied::new(RegionOutcome::Patched { revision }))
    }

    fn agent_parcel_selection_changes(&self, region: Region, parcel: ParcelLocalId, fence: FieldStamp) -> bool {
        let environment = &self.regions[&region].environment;
        newer_field(fence, environment.parcel_fence()) && environment.selected_parcel() != Some(parcel)
    }

    fn select_agent_parcel_at_revision(
        &mut self,
        region: Region,
        parcel: Option<ParcelLocalId>,
        fence: Option<FieldStamp>,
        supersedes_fence: bool,
        revision: WorldRevision,
    ) -> bool {
        self.update_agent_parcel_selection_at_revision(region, parcel, fence, supersedes_fence, revision)
    }
}

fn combine_or(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{fmt::Debug, sync::Arc};

    use math::Vec3;

    use super::*;
    use crate::{OrderEvidence::Ordered, WorldLimits};

    fn limits() -> WorldLimits {
        WorldLimits {
            max_region_registrations: 8,
            max_entities: 8,
            max_entities_per_region: 8,
            max_tombstones_per_region: 8,
            max_tombstones: 8,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(8),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(8),
            max_journal_entries: 128,
            max_journal_changes: 256,
            max_region_name_bytes: 128,
            max_recovery_targets: 4,
        }
    }

    fn source(region: Region, order: u64) -> UpdateSource {
        UpdateSource::new(region, order as u32, Ordered(order), 0)
    }

    fn bitmap(cells: &[(usize, usize)]) -> ParcelBitmap {
        let mut bytes = [0_u8; PARCEL_BITMAP_BYTES];
        for &(x, y) in cells {
            let bit = x + y * PARCEL_GRID_EDGE;
            bytes[bit / 8] |= 1 << (bit % 8);
        }
        ParcelBitmap::from_bytes(&bytes).unwrap()
    }

    fn properties(local_id: i32, flags: u32, bitmap: ParcelBitmap) -> ParcelPropertiesData {
        ParcelPropertiesData {
            local_id: ParcelLocalId::new(local_id).unwrap(),
            name: Arc::from("A home"),
            description: Arc::from("Parcel description"),
            owner_id: Some(Uuid::from_u128(1)),
            group_id: Some(Uuid::from_u128(2)),
            group_owned: false,
            area: 1024,
            flags: ParcelFlags::from_bits(flags),
            bitmap,
            landing_point: Some(Vec3::new(8.0, 12.0, 24.0)),
            landing_look_at: Some(Vec3::Z),
            landing_type: Some(1),
            music_url: Arc::from("https://music.invalid/stream"),
            media_url: Arc::from("https://media.invalid/page"),
            access: ParcelAccessSettings::default(),
        }
    }

    fn observe_properties(world: &mut World, region: Region, order: u64, sequence_id: i32, local_id: i32, flags: u32) {
        let observation = ParcelPropertiesObservation {
            request_result: 0,
            sequence_id,
            properties: Some(properties(local_id, flags, bitmap(&[(0, 0)]))),
        };
        let _ = world.observe_parcel_properties(source(region, order), observation).unwrap();
    }

    fn observe_overlay(world: &mut World, region: Region, order: u64, part: i32, data: Vec<u8>) {
        let _ = world.observe_parcel_overlay(source(region, order), ParcelOverlayObservation { sequence_id: part, data }).unwrap();
    }

    fn overlay_part(value: u8) -> Vec<u8> {
        vec![value; PARCEL_OVERLAY_PART_BYTES]
    }

    fn assert_invalid_field<T: Debug>(result: Result<T, ApplyError>, expected: &'static str) {
        match result {
            Err(ApplyError::InvalidValue { field }) => assert_eq!(field, expected),
            other => panic!("expected invalid {expected}, got {other:?}"),
        }
    }

    fn complete_overlay(world: &mut World, region: Region, start_order: u64, first: u8) {
        for (offset, part) in [2, 0, 3, 1].into_iter().enumerate() {
            let mut data = overlay_part(first.wrapping_add(part as u8));
            if part == 0 {
                data[0] = 0xfd;
            }
            if part == 2 {
                data[0] = 0x2e;
            }
            observe_overlay(world, region, start_order + offset as u64, part, data);
        }
    }

    #[test]
    fn overlay_publishes_only_after_out_of_order_parts_and_decodes_flags() {
        let region = Region::new(1);
        let mut world = World::new(limits());
        let _ = world.adopt_region(region).unwrap();
        let mut first = overlay_part(0);
        first[0] = 0xfd;
        observe_overlay(&mut world, region, 1, 0, first);
        let mut third = overlay_part(0);
        third[0] = 0x2e;
        observe_overlay(&mut world, region, 2, 2, third);

        assert!(world.parcel_state(region).unwrap().overlay.is_none());
        assert_eq!(world.check_invariants(), Ok(()));

        observe_overlay(&mut world, region, 3, 3, overlay_part(0));
        assert!(world.parcel_state(region).unwrap().overlay.is_none());
        observe_overlay(&mut world, region, 4, 1, overlay_part(0));

        let grid = world.parcel_state(region).unwrap().overlay.unwrap();
        assert_eq!(grid.raw_cell(0, 0), Some(0xfd));
        assert_eq!(
            grid.cell(0, 0),
            Some(ParcelOverlayCell {
                ownership: ParcelOwnership::Auction,
                hidden_avatars: true,
                sound_local: true,
                west_boundary: true,
                south_boundary: true,
                reserved_bit: true,
            })
        );
        assert_eq!(
            grid.cell(0, 32),
            Some(ParcelOverlayCell {
                ownership: ParcelOwnership::Unknown(6),
                hidden_avatars: false,
                sound_local: true,
                west_boundary: false,
                south_boundary: false,
                reserved_bit: true,
            })
        );
        assert_eq!(grid.cell(PARCEL_GRID_EDGE, 0), None);
        assert_eq!(world.check_invariants(), Ok(()));
    }

    #[test]
    fn incomplete_new_overlay_keeps_last_complete_grid() {
        let region = Region::new(2);
        let mut world = World::new(limits());
        let _ = world.adopt_region(region).unwrap();
        complete_overlay(&mut world, region, 1, 0);
        assert_eq!(world.parcel_state(region).unwrap().overlay.as_ref().unwrap().raw_cell(0, 0), Some(0xfd));

        observe_overlay(&mut world, region, 5, 0, overlay_part(7));

        let state = world.parcel_state(region).unwrap();
        assert_eq!(state.overlay.as_ref().unwrap().raw_cell(0, 0), Some(0xfd));
        assert_eq!(world.check_invariants(), Ok(()));
    }

    #[test]
    fn overlay_restart_preserves_parts_newer_than_its_boundary() {
        let region = Region::new(3);
        let mut world = World::new(limits());
        let _ = world.adopt_region(region).unwrap();

        observe_overlay(&mut world, region, 1, 0, overlay_part(0x10));
        observe_overlay(&mut world, region, 21, 1, overlay_part(0x21));
        observe_overlay(&mut world, region, 20, 0, overlay_part(0x20));
        observe_overlay(&mut world, region, 22, 2, overlay_part(0x22));
        observe_overlay(&mut world, region, 23, 3, overlay_part(0x23));

        let grid = world.parcel_state(region).unwrap().overlay.unwrap();
        assert_eq!(grid.raw_cell(0, 0), Some(0x20));
        assert_eq!(grid.raw_cell(0, 16), Some(0x21));
        assert_eq!(grid.raw_cell(0, 32), Some(0x22));
        assert_eq!(grid.raw_cell(0, 48), Some(0x23));
        assert_eq!(world.check_invariants(), Ok(()));
    }

    #[test]
    fn malformed_overlay_parts_do_not_mutate_assembly() {
        let region = Region::new(3);
        let mut world = World::new(limits());
        let _ = world.adopt_region(region).unwrap();

        assert_invalid_field(
            world.observe_parcel_overlay(source(region, 1), ParcelOverlayObservation { sequence_id: 4, data: overlay_part(0) }),
            "parcel_overlay_index",
        );
        assert_invalid_field(
            world.observe_parcel_overlay(
                source(region, 2),
                ParcelOverlayObservation { sequence_id: 0, data: vec![0; PARCEL_OVERLAY_PART_BYTES - 1] },
            ),
            "parcel_overlay_size",
        );
        assert!(world.parcel_state(region).unwrap().overlay.is_none());
        assert_eq!(world.parcel_bytes, 0);
        assert_eq!(world.check_invariants(), Ok(()));
    }

    #[test]
    fn parcel_flag_bits_match_official_wire_values() {
        let flags = ParcelFlags::from_bits(
            (1 << 0)
                | (1 << 1)
                | (1 << 5)
                | (1 << 6)
                | (1 << 8)
                | (1 << 9)
                | (1 << 10)
                | (1 << 11)
                | (1 << 15)
                | (1 << 21)
                | (1 << 22)
                | (1 << 25)
                | (1 << 26)
                | (1 << 31),
        );
        assert!(flags.allow_fly());
        assert!(flags.allow_other_scripts());
        assert!(flags.allow_damage());
        assert!(flags.allow_build());
        assert!(flags.group_access());
        assert!(flags.use_access_list());
        assert!(flags.use_ban_list());
        assert!(flags.use_pass_list());
        assert!(flags.sound_local());
        assert!(flags.restrict_push());
        assert!(flags.deny_anonymous());
        assert!(flags.allow_group_scripts());
        assert!(flags.deny_age_unverified());
    }

    #[test]
    fn walking_updates_only_the_newer_agent_parcel_and_excludes_selected_and_collision_replies() {
        let region = Region::new(4);
        let mut world = World::new(limits());
        let _ = world.adopt_region(region).unwrap();
        observe_properties(&mut world, region, 1, 20, 41, 0);
        observe_properties(&mut world, region, 2, 21, 42, 0);

        let state = world.parcel_state(region).unwrap();
        assert_eq!(state.current.as_ref().unwrap().sequence_id, 21);
        assert_eq!(state.current.as_ref().unwrap().properties.local_id, ParcelLocalId::new(42).unwrap());
        assert_eq!(world.environment(region).unwrap().parcel.unwrap().parcel, ParcelLocalId::new(42).unwrap());

        for sequence_id in [20_i32, -10_000, -20_000, -30_000, -40_000, -50_000] {
            let _ = world
                .observe_parcel_properties(
                    source(region, 3 + sequence_id.unsigned_abs() as u64),
                    ParcelPropertiesObservation { request_result: 0, sequence_id, properties: None },
                )
                .unwrap();
        }
        let _ = world
            .observe_parcel_properties(
                source(region, 60_000),
                ParcelPropertiesObservation { request_result: -1, sequence_id: 22, properties: None },
            )
            .unwrap();
        let state = world.parcel_state(region).unwrap();
        assert_eq!(state.current.as_ref().unwrap().sequence_id, 21);
        assert_eq!(state.current.as_ref().unwrap().properties.local_id, ParcelLocalId::new(42).unwrap());

        let _ = world.clear_agent_parcel(region).unwrap();
        assert!(world.parcel_state(region).unwrap().current.is_none());
        assert!(world.environment(region).unwrap().parcel.is_none());
        observe_properties(&mut world, region, 60_001, 21, 43, 0);
        assert!(world.parcel_state(region).unwrap().current.is_none());
        observe_properties(&mut world, region, 60_002, 22, 43, 0);
        assert_eq!(world.parcel_state(region).unwrap().current.as_ref().unwrap().properties.local_id, ParcelLocalId::new(43).unwrap());
        assert_eq!(world.check_invariants(), Ok(()));
    }

    #[test]
    fn effective_restrictions_combine_region_parcel_and_overlay_flags() {
        let region = Region::new(5);
        let mut world = World::new(limits());
        let _ = world.adopt_region(region).unwrap();
        let parcel_flags = (1 << 0) | (1 << 1) | (1 << 5) | (1 << 6);
        observe_properties(&mut world, region, 1, 1, 5, parcel_flags);
        let region_flags = (1 << 19) | (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23) | (1 << 13);
        let _ = world.observe_region_flags(source(region, 2), Some(region_flags), Some(0)).unwrap();
        let state = world.parcel_state(region).unwrap();
        assert_eq!(state.effective_restrictions.allow_fly, Some(false));
        assert_eq!(state.effective_restrictions.allow_build, Some(true));
        assert_eq!(state.effective_restrictions.allow_scripts, Some(false));
        assert_eq!(state.effective_restrictions.restrict_push, Some(true));
        assert_eq!(state.effective_restrictions.allow_damage, Some(true));
        assert_eq!(state.effective_restrictions.allow_direct_teleport, Some(true));
        assert_eq!(state.effective_restrictions.deny_anonymous, Some(true));

        complete_overlay(&mut world, region, 3, 0);
        let state = world.parcel_state(region).unwrap();
        assert_eq!(state.effective_restrictions.avatars_hidden_outside, Some(true));
        assert_eq!(state.effective_restrictions.sound_local, Some(true));
        assert_eq!(world.check_invariants(), Ok(()));
    }

    #[test]
    fn property_validation_bounds_urls_and_debug_redacts_them() {
        let region = Region::new(6);
        let mut world = World::new(limits());
        let _ = world.adopt_region(region).unwrap();
        let mut data = properties(1, 0, bitmap(&[]));
        data.music_url = Arc::from("https://secret.invalid/music");
        data.media_url = Arc::from("https://secret.invalid/media");
        let debug = format!("{data:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("secret.invalid"));
        data.media_url = Arc::from("x".repeat(MAX_PARCEL_URL_BYTES + 1));
        assert_invalid_field(
            world.observe_parcel_properties(
                source(region, 1),
                ParcelPropertiesObservation { request_result: 0, sequence_id: 1, properties: Some(data) },
            ),
            "parcel_media_url",
        );
        assert!(world.parcel_state(region).unwrap().current.is_none());
        assert_eq!(world.check_invariants(), Ok(()));
    }

    #[test]
    fn region_crossing_and_interleaved_retirement_preserve_world_invariants() {
        let first = Region::new(7);
        let second = Region::new(8);
        let mut world = World::new(limits());
        let _ = world.adopt_region(first).unwrap();
        let _ = world.adopt_region(second).unwrap();

        observe_overlay(&mut world, first, 1, 0, overlay_part(1));
        observe_properties(&mut world, first, 2, 7, 70, 0);
        observe_overlay(&mut world, second, 3, 2, overlay_part(2));
        observe_properties(&mut world, second, 4, 1, 80, 0);
        assert_eq!(world.check_invariants(), Ok(()));

        let _ = world.clear_agent_parcel(first).unwrap();
        assert!(world.parcel_state(first).unwrap().current.is_none());
        assert!(world.environment(first).unwrap().parcel.is_none());
        assert_eq!(world.parcel_state(second).unwrap().current.as_ref().unwrap().properties.local_id, ParcelLocalId::new(80).unwrap());
        observe_overlay(&mut world, second, 5, 0, overlay_part(3));
        assert_eq!(world.check_invariants(), Ok(()));

        let _ = world.retire_region(first).unwrap();
        assert!(world.parcel_state(first).is_none());
        assert!(world.parcel_state(second).is_some());
        assert_eq!(
            world.observe_parcel_properties(
                source(first, 6),
                ParcelPropertiesObservation { request_result: 0, sequence_id: 8, properties: None },
            ),
            Err(ApplyError::StaleRegion)
        );
        assert_eq!(world.check_invariants(), Ok(()));
        let _ = world.retire_region(second).unwrap();
        assert_eq!(world.parcel_bytes, 0);
        assert_eq!(world.check_invariants(), Ok(()));
    }
}
