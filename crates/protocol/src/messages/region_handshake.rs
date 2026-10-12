use base::UUID;

/// A simulator's initial region metadata message.
///
/// Legacy packets may contain only `RegionInfo1`; `RegionInfo2` and `RegionInfo3` appear together.
#[derive(Clone, Debug, PartialEq)]
pub struct RegionHandshake {
    pub region_info1: RegionInfo1,
    pub region_info2: Option<RegionInfo2>,
    pub region_info3: Option<RegionInfo3>,
    pub region_info4: Vec<RegionInfo4>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionInfo1 {
    pub region_flags: u32,
    pub sim_access: u8,
    pub sim_name: String,
    pub sim_owner: UUID,
    pub is_estate_manager: bool,
    pub water_height: f32,
    pub billable_factor: f32,
    pub cache_id: UUID,
    /// Terrain textures in `TerrainBase0` through `TerrainBase3` order.
    pub terrain_base: [UUID; 4],
    /// Detail textures in `TerrainDetail0` through `TerrainDetail3` order.
    pub terrain_detail: [UUID; 4],
    /// Terrain start heights in meters, ordered `00`, `01`, `10`, then `11`.
    pub terrain_start_heights: [f32; 4],
    /// Terrain height ranges in meters, ordered `00`, `01`, `10`, then `11`.
    pub terrain_height_ranges: [f32; 4],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionInfo2 {
    pub region_id: UUID,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionInfo3 {
    pub cpu_class_id: i32,
    pub cpu_ratio: i32,
    pub colo_name: String,
    pub product_sku: String,
    pub product_name: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RegionInfo4 {
    pub region_flags_extended: u64,
    pub region_protocols: u64,
}
