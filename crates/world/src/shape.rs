//! Renderer-independent observed volume parameters and sculpt or mesh references.

use crate::AssetId;

/// Flags carried by sculpt and mesh volume parameters.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct SculptFlags {
    pub invert: bool,
    pub mirror: bool,
}

/// A recognized sculpt geometry subtype.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SculptType {
    Sphere,
    Torus,
    Plane,
    Cylinder,
}

/// Quantized path and profile parameters observed for a regular volume.
///
/// These are lossless simulator values, not validated geometry. Unknown curve codes and
/// out-of-range combinations remain representable; geometry consumers must constrain them
/// before generating meshes. Keeping the integers preserves exact equality and avoids
/// rounding observed values through floating-point conversions.
///
/// Cut beginnings and hollow use units of 1/50,000. Cut endings store the distance from
/// one in the same units. Path scale is `2 - value / 100`; shear, twist, radius offset,
/// taper, and skew use units of 1/100. Revolutions is `1 + value * 0.015`.
/// Twist is a curve-dependent fraction, not an angle in radians.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PrimShape {
    pub path_curve: u8,
    /// Low nibble selects the profile; high nibble selects the hollow profile.
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
    /// An uncut square profile extruded along a straight path, forming a box.
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

/// The object's shape and any verified sculpt or mesh asset data.
///
/// Sculpt and mesh variants carry asset data; matches should use struct patterns such as
/// `Shape::Sculpt { .. }` and `Shape::Mesh { .. }`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Shape {
    /// A regular volume without a sculpt or mesh asset.
    Prim(PrimShape),
    Sculpt {
        /// Sculpt texture asset; distinct from the object's identity.
        asset_id: AssetId,
        /// Recognized sculpt geometry subtype.
        sculpt_type: SculptType,
        /// Invert and mirror bits from the sculpt parameter.
        flags: SculptFlags,
    },
    Mesh {
        /// Mesh asset; distinct from the object's identity.
        asset_id: AssetId,
        /// Invert and mirror bits from the sculpt parameter.
        flags: SculptFlags,
    },
    Avatar,
}
