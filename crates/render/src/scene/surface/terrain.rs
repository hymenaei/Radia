use std::{
    array,
    sync::atomic::{AtomicBool, Ordering},
};

use assets::mesh::{Face, Mesh, Vertex};
use math::{DVec3, Vec2, Vec3};
use world::{Region, TerrainPatchId, TerrainSettings};

#[cfg(test)]
use uuid::Uuid;

use crate::scene::{MeshBuildError, Patch, model::Mirror};

const TERRAIN_REGION_SIZE: f32 = 256.0;
const TERRAIN_NORMAL_STRIDE: i32 = 2;
const NOISE_XY_SCALE_INV: f32 = 1.0 / 4.9215;
const NOISE_MAGNITUDE: f32 = 2.0;
const NOISE_SLOPE_SQUARED: f32 = 2.25;
const NOISE_LOW_FREQUENCY_SCALE: f32 = 0.222_222_22;

struct Tile {
    patch: Patch,
    x: Option<usize>,
    y: Option<usize>,
}

pub(super) struct Terrain {
    tiles: [Option<Tile>; 9],
    patch: TerrainPatchId,
    origin: DVec3,
    settings: Option<TerrainSettings>,
}

impl Terrain {
    pub(super) fn complete(&self) -> bool {
        self.tiles.iter().all(Option::is_some)
    }

    pub(super) fn capture(mirror: &Mirror, region: Region, patch: TerrainPatchId, origin: DVec3) -> Self {
        let (x, y) = patch.coordinates();
        let tiles = array::from_fn(|index| {
            let (region, patch, x, y) = locate(mirror, region, i32::from(x) + index as i32 % 3 - 1, i32::from(y) + index as i32 / 3 - 1)?;
            Some(Tile { patch: mirror.patches.get(&(region, patch))?.clone(), x, y })
        });
        let settings = mirror.regions.get(&region).and_then(|data| data.summary().terrain_settings);
        Self { tiles, patch, origin, settings }
    }

    pub(super) fn mesh(&self, cancelled: &AtomicBool) -> Result<(Mesh, Vec<f32>), MeshBuildError> {
        let tiles = &self.tiles;
        let (patch_x, patch_y) = self.patch.coordinates();
        let mut vertices = Vec::with_capacity(289);
        let mut composition_values = Vec::with_capacity(289);
        let mut indices = Vec::with_capacity(1536);
        let mut grid = [[None; 17]; 17];
        for y in 0..17 {
            if cancelled.load(Ordering::Acquire) {
                return Err(MeshBuildError::Cancelled);
            }
            for x in 0..17 {
                let Some(z) = sample(tiles, x, y) else { continue };
                let normal = classic_normal(x, y, |x, y| sample(tiles, x, y).unwrap_or(z));
                let local_x = f32::from(patch_x) * 16.0 + x as f32;
                let local_y = f32::from(patch_y) * 16.0 + y as f32;
                let composition = self.settings.map_or(0.0, |settings| {
                    composition_coordinate(
                        &settings,
                        local_x,
                        local_y,
                        z as f32,
                        (self.origin.x + f64::from(local_x)) as f32,
                        (self.origin.y + f64::from(local_y)) as f32,
                    )
                });
                grid[y as usize][x as usize] = Some(vertices.len() as u32);
                vertices.push(Vertex {
                    position: Vec3::new(x as f32, y as f32, z as f32),
                    normal,
                    uv: Vec2::new(x as f32, y as f32) / 16.0,
                });
                composition_values.push(composition);
            }
        }
        for y in 0..16 {
            for x in 0..16 {
                let [Some(a), Some(b), Some(c), Some(d)] = [grid[y][x], grid[y][x + 1], grid[y + 1][x], grid[y + 1][x + 1]] else {
                    continue;
                };
                indices.extend([a, b, d, a, d, c]);
            }
        }
        let count = indices.len() as u32;
        let mesh = Mesh::from_parts(vertices, indices, vec![Face { slot: 0, indices: 0..count }]).map_err(MeshBuildError::from)?;
        Ok((mesh, composition_values))
    }
}

fn classic_normal(x: i32, y: i32, sample: impl Fn(i32, i32) -> f64) -> Vec3 {
    let stride = TERRAIN_NORMAL_STRIDE;
    let point = |x, y| DVec3::new(f64::from(x), f64::from(y), sample(x, y));
    let p00 = point(x - stride, y - stride);
    let p01 = point(x - stride, y + stride);
    let p10 = point(x + stride, y - stride);
    let p11 = point(x + stride, y + stride);
    (p11 - p00).cross(p01 - p10).normalize().as_vec3()
}

pub(super) fn affected_patches(mirror: &Mirror, region: Region, patch: TerrainPatchId) -> impl Iterator<Item = (Region, TerrainPatchId)> {
    let (x, y) = patch.coordinates();
    array::from_fn::<_, 9, _>(|index| {
        let (region, patch, _, _) = locate(mirror, region, i32::from(x) + index as i32 % 3 - 1, i32::from(y) + index as i32 / 3 - 1)?;
        mirror.patches.contains_key(&(region, patch)).then_some((region, patch))
    })
    .into_iter()
    .flatten()
}

fn locate(mirror: &Mirror, region: Region, x: i32, y: i32) -> Option<(Region, TerrainPatchId, Option<usize>, Option<usize>)> {
    let origin = mirror.regions.get(&region)?.summary().info.resolved_origin()?;
    let dx = x.div_euclid(16);
    let dy = y.div_euclid(16);
    let find = |dx, dy| {
        let position = origin + DVec3::new(f64::from(dx) * 256.0, f64::from(dy) * 256.0, 0.0);
        let mut matches = mirror.regions.values().filter(|r| r.summary().info.resolved_origin() == Some(position));
        let region = matches.next().map(|region| region.summary().region);
        matches.next().is_none().then_some(region)
    };
    let found = if dx == 0 && dy == 0 { Some(region) } else { find(dx, dy)? };
    let (region, clamp_x, clamp_y) = if let Some(region) = found {
        (region, false, false)
    } else if dx != 0 && dy != 0 {
        match (find(dx, 0)?, find(0, dy)?) {
            (Some(region), None) => (region, false, true),
            (None, Some(region)) => (region, true, false),
            _ => (region, true, true),
        }
    } else {
        (region, dx != 0, dy != 0)
    };
    let patch_x = if clamp_x { x.clamp(0, 15) } else { x.rem_euclid(16) };
    let patch_y = if clamp_y { y.clamp(0, 15) } else { y.rem_euclid(16) };
    Some((
        region,
        TerrainPatchId::new(patch_x as u8, patch_y as u8)?,
        clamp_x.then_some(if dx < 0 { 0 } else { 15 }),
        clamp_y.then_some(if dy < 0 { 0 } else { 15 }),
    ))
}

fn sample(tiles: &[Option<Tile>; 9], x: i32, y: i32) -> Option<f64> {
    let tile = tiles[((y.div_euclid(16) + 1) * 3 + x.div_euclid(16) + 1) as usize].as_ref()?;
    let x = tile.x.unwrap_or(x.rem_euclid(16) as usize);
    let y = tile.y.unwrap_or(y.rem_euclid(16) as usize);
    Some(f64::from(tile.patch.source().heights()[y * 16 + x]))
}

fn composition_coordinate(settings: &TerrainSettings, local_x: f32, local_y: f32, height: f32, global_x: f32, global_y: f32) -> f32 {
    let x = (local_x / TERRAIN_REGION_SIZE).clamp(0.0, 1.0);
    let y = (local_y / TERRAIN_REGION_SIZE).clamp(0.0, 1.0);
    let start = bilinear(*settings.start_heights(), x, y);
    let range = bilinear(*settings.height_ranges(), x, y);
    let global_x = global_x * NOISE_XY_SCALE_INV;
    let global_y = global_y * NOISE_XY_SCALE_INV;
    let low = noise2([global_x * NOISE_LOW_FREQUENCY_SCALE, global_y * NOISE_LOW_FREQUENCY_SCALE]);
    let high = turbulence2([global_x, global_y], 2.0);
    let twiddle = (low * 6.5 + high * NOISE_SLOPE_SQUARED) * NOISE_MAGNITUDE;
    ((height + twiddle - start) * 4.0 / range).clamp(0.0, 3.0)
}

fn bilinear(corners: [f32; 4], x: f32, y: f32) -> f32 {
    let [southwest, southeast, northwest, northeast] = corners;
    (1.0 - x) * (1.0 - y) * southwest + x * (1.0 - y) * southeast + (1.0 - x) * y * northwest + x * y * northeast
}

#[derive(Clone, Copy)]
struct NoiseTable {
    permutation: [u8; 514],
    gradients: [[f32; 2]; 514],
}

impl NoiseTable {
    fn new() -> Self {
        let mut random = 42u32;
        let mut permutation = [0u8; 514];
        let mut gradients = [[0.0; 2]; 514];
        for index in 0..256 {
            permutation[index] = index as u8;
            let _ = random_mod(&mut random, 512);
            let mut gradient = [0.0; 2];
            for component in &mut gradient {
                *component = random_mod(&mut random, 512) as f32 / 256.0 - 1.0;
            }
            let length = (gradient[0] * gradient[0] + gradient[1] * gradient[1]).sqrt();
            gradient[0] /= length;
            gradient[1] /= length;
            gradients[index] = gradient;
            for _ in 0..3 {
                let _ = random_mod(&mut random, 512);
            }
        }
        for index in (1..256).rev() {
            let other = random_mod(&mut random, 256) as usize;
            permutation.swap(index, other);
        }
        for index in 0..258 {
            permutation[256 + index] = permutation[index];
            gradients[256 + index] = gradients[index];
        }
        Self { permutation, gradients }
    }
}

fn random_mod(state: &mut u32, modulus: u32) -> u32 {
    *state = state.wrapping_mul(214_013).wrapping_add(2_531_011);
    ((*state >> 16) & 0x7fff) % modulus
}

fn noise2(point: [f32; 2]) -> f32 {
    static TABLE: std::sync::OnceLock<NoiseTable> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(NoiseTable::new);
    let setup = |value: f32| {
        let shifted = value + 4096.0;
        let integer = shifted.trunc() as i32;
        let base = integer as u8;
        let fraction = shifted - integer as f32;
        (usize::from(base), usize::from(base.wrapping_add(1)), fraction, fraction - 1.0)
    };
    let (x0, x1, rx0, rx1) = setup(point[0]);
    let (y0, y1, ry0, ry1) = setup(point[1]);
    let i = usize::from(table.permutation[x0]);
    let j = usize::from(table.permutation[x1]);
    let b00 = usize::from(table.permutation[i + y0]);
    let b10 = usize::from(table.permutation[j + y0]);
    let b01 = usize::from(table.permutation[i + y1]);
    let b11 = usize::from(table.permutation[j + y1]);
    let at = |x, y, gradient: [f32; 2]| x * gradient[0] + y * gradient[1];
    let curve = |value: f32| value * value * (3.0 - 2.0 * value);
    let lerp = |t: f32, a: f32, b: f32| a + t * (b - a);
    let sx = curve(rx0);
    let sy = curve(ry0);
    let a = lerp(sx, at(rx0, ry0, table.gradients[b00]), at(rx1, ry0, table.gradients[b10]));
    let b = lerp(sx, at(rx0, ry1, table.gradients[b01]), at(rx1, ry1, table.gradients[b11]));
    lerp(sy, a, b)
}

fn turbulence2(point: [f32; 2], mut frequency: f32) -> f32 {
    let mut value = 0.0;
    while frequency >= 1.0 {
        value += noise2([frequency * point[0], frequency * point[1]]) / frequency;
        frequency *= 0.5;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture::TERRAIN_ALPHA_RAMP;

    fn settings(textures: [Uuid; 4], start_heights: [f32; 4], height_ranges: [f32; 4]) -> TerrainSettings {
        TerrainSettings::new(textures, start_heights, height_ranges).unwrap()
    }

    #[test]
    fn hand_derived_boundaries_and_corner_ranges_match_the_composition_scale() {
        let starts = [10.0, 20.0, 30.0, 40.0];
        let ranges = [40.0, 80.0, 120.0, 160.0];
        let settings = settings([Uuid::nil(); 4], starts, ranges);
        let corners = [(0.0, 0.0), (256.0, 0.0), (0.0, 256.0), (256.0, 256.0)];
        let boundaries = [(0.0, 0.0), (0.25, 0.25), (1.0, 1.0), (1.5, 1.5), (2.0, 2.0), (3.0, 3.0), (4.0, 3.0)];
        for (index, (start, range)) in starts.into_iter().zip(ranges).enumerate() {
            let (x, y) = corners[index];
            for (scaled, expected) in boundaries {
                let height = start + range * scaled / 4.0;
                assert_eq!(composition_coordinate(&settings, x, y, height, 0.0, 0.0), expected, "corner {index}, scaled height {scaled}");
            }
        }
        assert_eq!(bilinear(starts, 0.0, 0.0), 10.0);
        assert_eq!(bilinear(starts, 1.0, 0.0), 20.0);
        assert_eq!(bilinear(starts, 0.0, 1.0), 30.0);
        assert_eq!(bilinear(starts, 1.0, 1.0), 40.0);
        assert_eq!(bilinear(ranges, 0.0, 0.0), 40.0);
        assert_eq!(bilinear(ranges, 1.0, 0.0), 80.0);
        assert_eq!(bilinear(ranges, 0.0, 1.0), 120.0);
        assert_eq!(bilinear(ranges, 1.0, 1.0), 160.0);
        assert_eq!(bilinear(starts, 0.5, 0.5), 25.0);
        assert_eq!(bilinear(ranges, 0.5, 0.5), 100.0);
        assert_eq!(composition_coordinate(&settings, 128.0, 128.0, 50.0, 0.0, 0.0), 1.0);
    }

    #[test]
    fn neighbor_edge_coordinates_match_for_matching_settings() {
        let west = settings([Uuid::from_u128(1); 4], [10.0, 20.0, 30.0, 40.0], [40.0, 80.0, 120.0, 160.0]);
        let east = settings([Uuid::from_u128(2); 4], [20.0, 22.0, 40.0, 42.0], [80.0, 81.0, 160.0, 161.0]);
        let west_coordinate = composition_coordinate(&west, 256.0, 64.0, 34.5, 256.0, 64.0);
        let east_coordinate = composition_coordinate(&east, 0.0, 64.0, 34.5, 256.0, 64.0);
        assert_eq!(west_coordinate, east_coordinate);
        assert_ne!(west.detail_textures(), east.detail_textures());
    }

    #[test]
    fn official_noise_fixture_at_nonzero_region_coordinates() {
        let settings = settings([Uuid::nil(); 4], [10.0, 20.0, 30.0, 40.0], [40.0, 80.0, 120.0, 160.0]);
        let coordinate = composition_coordinate(&settings, 128.0, 128.0, 50.0, 2048.0, 4096.0);
        assert!((coordinate - 0.960_022).abs() < 1.0e-6, "composition coordinate {coordinate}");
    }

    fn ramp_alpha(coordinate: f32) -> f32 {
        let position = coordinate.clamp(0.0, 1.0) * TERRAIN_ALPHA_RAMP.len() as f32 - 0.5;
        let lower = position.floor();
        let fraction = position - lower;
        let sample = |index: f32| {
            let index = index.clamp(0.0, TERRAIN_ALPHA_RAMP.len() as f32 - 1.0) as usize;
            f32::from(TERRAIN_ALPHA_RAMP[index]) / 255.0
        };
        let first = sample(lower);
        first + (sample(lower + 1.0) - first) * fraction
    }

    fn ramp_weights(coordinate: f32) -> [f32; 4] {
        let alpha1 = ramp_alpha(coordinate);
        let alpha2 = ramp_alpha(coordinate - 2.0);
        let alpha_final = ramp_alpha(coordinate - 1.0);
        [alpha_final * alpha1, alpha_final * (1.0 - alpha1), (1.0 - alpha_final) * alpha2, (1.0 - alpha_final) * (1.0 - alpha2)]
    }

    fn mix(first: [f32; 3], second: [f32; 3], alpha: f32) -> [f32; 3] {
        std::array::from_fn(|index| first[index] * (1.0 - alpha) + second[index] * alpha)
    }

    #[test]
    fn ramp_layer_boundaries_match_reference() {
        let cases = [
            (0.0, [0.992_172_24, 0.003_906_19, 0.003_906_19, 0.000_015_38]),
            (1.0, [0.007_812_38, 0.988_266_05, 0.003_906_19, 0.000_015_38]),
            (2.0, [0.000_061_51, 0.007_781_62, 0.988_266_05, 0.003_890_81]),
            (3.0, [0.000_061_51, 0.007_781_62, 0.007_781_62, 0.984_375_24]),
        ];
        for (coordinate, expected) in cases {
            for (actual, expected) in ramp_weights(coordinate).into_iter().zip(expected) {
                assert!((actual - expected).abs() < 1.0e-6, "coordinate {coordinate}: {actual} != {expected}");
            }
        }
    }

    #[test]
    fn mid_triangle_matches_official_ramp() {
        let vertex_composition = [1.25, 1.75, 1.5];
        let composition = vertex_composition[0] * 0.25 + vertex_composition[1] * 0.25 + vertex_composition[2] * 0.5;
        assert_eq!(composition, 1.5);

        let alpha1 = ramp_alpha(composition);
        let alpha2 = ramp_alpha(composition - 2.0);
        let alpha_final = ramp_alpha(composition - 1.0);
        assert!((alpha1 - 2.0 / 255.0).abs() < 1.0e-6);
        assert!((alpha2 - 254.0 / 255.0).abs() < 1.0e-6);
        assert!((alpha_final - 379.0 / 510.0).abs() < 1.0e-6);

        let c3 = [1.0, 1.0, 1.0];
        let c2 = [0.0, 0.0, 1.0];
        let c1 = [0.0, 1.0, 0.0];
        let c0 = [1.0, 0.0, 0.0];
        let actual = mix(mix(c3, c2, alpha2), mix(c1, c0, alpha1), alpha_final);
        let expected = [0.006_835_83, 0.738_316_03, 0.256_862_75];
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 1.0e-6, "{actual} != {expected}");
        }
    }

    fn terrain(center_patch_x: i32, origin_x: f64, height: impl Fn(f64, f64) -> f64) -> Terrain {
        use crate::scene::Budget;
        use world::TerrainPatch;

        let budget = Budget::default();
        let tiles = array::from_fn(|index| {
            let dx = index as i32 % 3 - 1;
            let dy = index as i32 / 3 - 1;
            let patch_x = center_patch_x + dx;
            let patch_y = 8 + dy;
            let heights = Box::new(array::from_fn(|sample| {
                let x = origin_x + f64::from(patch_x * 16 + (sample % 16) as i32);
                let y = f64::from(patch_y * 16 + (sample / 16) as i32);
                height(x, y) as f32
            }));
            let patch = TerrainPatch::new(patch_x.rem_euclid(16) as u8, patch_y as u8, heights).unwrap();
            Some(Tile { patch: Patch::new(patch, budget.reserve(512).unwrap()), x: None, y: None })
        });
        Terrain {
            tiles,
            patch: TerrainPatchId::new(center_patch_x.rem_euclid(16) as u8, 8).unwrap(),
            origin: DVec3::new(origin_x, 0.0, 0.0),
            settings: None,
        }
    }

    fn assert_edge_normals_match(west: Terrain, east: Terrain, seam_x: f64) {
        let cancelled = AtomicBool::new(false);
        let (west_mesh, west_composition) = west.mesh(&cancelled).unwrap();
        let (east_mesh, east_composition) = east.mesh(&cancelled).unwrap();
        assert_eq!(west_composition.len(), west_mesh.vertices().len());
        assert_eq!(east_composition.len(), east_mesh.vertices().len());
        let west_normal = west_mesh.vertices()[8 * 17 + 16].normal;
        let east_normal = east_mesh.vertices()[8 * 17].normal;
        let expected = Vec3::new(-1.0 / 5.0_f32.sqrt(), 0.0, 2.0 / 5.0_f32.sqrt());
        assert!(west_normal.distance(expected) < 1.0e-6, "west side of edge at {seam_x}");
        assert!(east_normal.distance(expected) < 1.0e-6, "east side of edge at {seam_x}");
    }

    #[test]
    fn patch_edge_normals_use_neighbor_heights() {
        let seam = 9.0 * 16.0;
        let height = |x: f64, _y: f64| (x - seam).max(0.0);
        assert_edge_normals_match(terrain(8, 0.0, height), terrain(9, 0.0, height), seam);
    }

    #[test]
    fn region_edge_normals_stitch_neighbor_heights() {
        let seam = 256.0;
        let height = |x: f64, _y: f64| (x - seam).max(0.0);
        assert_edge_normals_match(terrain(15, 0.0, height), terrain(0, 256.0, height), seam);
    }

    #[test]
    fn small_heightfield_normal_matches_classic_diagonal_formula() {
        let heights = [
            [0.0, 2.0, 4.0, 6.0, 8.0],
            [1.0, 3.0, 5.0, 7.0, 9.0],
            [2.0, 4.0, 6.0, 8.0, 10.0],
            [3.0, 5.0, 7.0, 9.0, 11.0],
            [4.0, 6.0, 8.0, 10.0, 12.0],
        ];
        let normal = classic_normal(2, 2, |x, y| heights[y as usize][x as usize]);
        let expected = Vec3::new(-2.0, -1.0, 1.0).normalize();
        assert!(normal.distance(expected) < 1.0e-6);
    }
}
