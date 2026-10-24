use utils::AssetId;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct SculptFlags {
    pub invert: bool,
    pub mirror: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SculptType {
    Sphere,
    Torus,
    Plane,
    Cylinder,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PrimShape {
    pub path_curve: u8,
    pub profile_curve: u8,
    pub path_begin: u16,
    pub path_end: u16,
    pub path_scale_x: u8,
    pub path_scale_y: u8,
    pub path_shear_x: i8,
    pub path_shear_y: i8,
    pub path_twist: i8,
    pub path_twist_begin: i8,
    pub path_radius_offset: i8,
    pub path_taper_x: i8,
    pub path_taper_y: i8,
    pub path_revolutions: u8,
    pub path_skew: i8,
    pub profile_begin: u16,
    pub profile_end: u16,
    pub profile_hollow: u16,
}

impl Default for PrimShape {
    fn default() -> Self {
        Self {
            path_curve: 0x10,
            profile_curve: 0x01,
            path_begin: 0,
            path_end: 0,
            path_scale_x: 100,
            path_scale_y: 100,
            path_shear_x: 0,
            path_shear_y: 0,
            path_twist: 0,
            path_twist_begin: 0,
            path_radius_offset: 0,
            path_taper_x: 0,
            path_taper_y: 0,
            path_revolutions: 0,
            path_skew: 0,
            profile_begin: 0,
            profile_end: 0,
            profile_hollow: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Shape {
    Prim(PrimShape),
    Sculpt { asset_id: AssetId, sculpt_type: SculptType, flags: SculptFlags },
    Mesh { asset_id: AssetId, flags: SculptFlags },
    Avatar,
}
