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

    pub(super) fn mesh(&self, cancelled: &AtomicBool) -> Result<Mesh, MeshBuildError> {
        let tiles = &self.tiles;
        let (patch_x, patch_y) = self.patch.coordinates();
        let mut vertices = Vec::with_capacity(289);
        let mut indices = Vec::with_capacity(1536);
        let mut grid = [[None; 17]; 17];
        for y in 0..17 {
            if cancelled.load(Ordering::Acquire) {
                return Err(MeshBuildError::Cancelled);
            }
            for x in 0..17 {
                let Some(z) = sample(tiles, x, y) else { continue };
                let slope = |a: Option<f64>, b: Option<f64>| match (a, b) {
                    (Some(a), Some(b)) => (b - a) * 0.5,
                    (Some(a), None) => z - a,
                    (None, Some(b)) => b - z,
                    _ => 0.0,
                };
                let dx = slope(sample(tiles, x - 1, y), sample(tiles, x + 1, y));
                let dy = slope(sample(tiles, x, y - 1), sample(tiles, x, y + 1));
                let normal = DVec3::new(-dx, -dy, 1.0).normalize().as_vec3();
                let local_x = f32::from(patch_x) * 16.0 + x as f32;
                let local_y = f32::from(patch_y) * 16.0 + y as f32;
                let weights = self.settings.map_or([1.0, 0.0, 0.0, 0.0], |settings| {
                    blend_weights(
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
                    weights,
                });
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
        Mesh::from_parts(vertices, indices, vec![Face { slot: 0, indices: 0..count }]).map_err(MeshBuildError::from)
    }
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

fn blend_weights(settings: &TerrainSettings, local_x: f32, local_y: f32, height: f32, global_x: f32, global_y: f32) -> [f32; 4] {
    let x = (local_x / TERRAIN_REGION_SIZE).clamp(0.0, 1.0);
    let y = (local_y / TERRAIN_REGION_SIZE).clamp(0.0, 1.0);
    let start = bilinear(*settings.start_heights(), x, y);
    let range = bilinear(*settings.height_ranges(), x, y);
    let global_x = global_x * NOISE_XY_SCALE_INV;
    let global_y = global_y * NOISE_XY_SCALE_INV;
    let low = noise2([global_x * NOISE_LOW_FREQUENCY_SCALE, global_y * NOISE_LOW_FREQUENCY_SCALE]);
    let high = turbulence2([global_x, global_y], 2.0);
    let twiddle = (low * 6.5 + high * NOISE_SLOPE_SQUARED) * NOISE_MAGNITUDE;
    layer_weights((height + twiddle - start) * 4.0 / range)
}

fn bilinear(corners: [f32; 4], x: f32, y: f32) -> f32 {
    let [southwest, southeast, northwest, northeast] = corners;
    (1.0 - x) * (1.0 - y) * southwest + x * (1.0 - y) * southeast + (1.0 - x) * y * northwest + x * y * northeast
}

fn layer_weights(value: f32) -> [f32; 4] {
    let value = value.clamp(0.0, 3.0);
    let lower = value.floor() as usize;
    let upper = (lower + 1).min(3);
    let upper_weight = value - lower as f32;
    let mut weights = [0.0; 4];
    weights[lower] = 1.0 - upper_weight;
    weights[upper] += upper_weight;
    weights
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

    fn settings(textures: [Uuid; 4], start_heights: [f32; 4], height_ranges: [f32; 4]) -> TerrainSettings {
        TerrainSettings::new(textures, start_heights, height_ranges).unwrap()
    }

    #[test]
    fn hand_derived_boundaries_and_corner_ranges_match_the_composition_scale() {
        let starts = [10.0, 20.0, 30.0, 40.0];
        let ranges = [40.0, 80.0, 120.0, 160.0];
        let settings = settings([Uuid::nil(); 4], starts, ranges);
        let corners = [(0.0, 0.0), (256.0, 0.0), (0.0, 256.0), (256.0, 256.0)];
        let boundaries = [
            (0.0, [1.0, 0.0, 0.0, 0.0]),
            (0.25, [0.75, 0.25, 0.0, 0.0]),
            (1.0, [0.0, 1.0, 0.0, 0.0]),
            (1.5, [0.0, 0.5, 0.5, 0.0]),
            (2.0, [0.0, 0.0, 1.0, 0.0]),
            (3.0, [0.0, 0.0, 0.0, 1.0]),
            (4.0, [0.0, 0.0, 0.0, 1.0]),
        ];
        for (index, (start, range)) in starts.into_iter().zip(ranges).enumerate() {
            let (x, y) = corners[index];
            for (scaled, expected) in boundaries {
                let height = start + range * scaled / 4.0;
                assert_eq!(blend_weights(&settings, x, y, height, 0.0, 0.0), expected, "corner {index}, scaled height {scaled}");
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
        let weight_sum = blend_weights(&settings, 128.0, 128.0, 50.0, 2048.0, 4096.0).iter().sum::<f32>();
        assert!((weight_sum - 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn neighbor_edge_weights_match_when_edge_height_ranges_and_terrain_heights_match() {
        let west = settings([Uuid::from_u128(1); 4], [10.0, 20.0, 30.0, 40.0], [40.0, 80.0, 120.0, 160.0]);
        let east = settings([Uuid::from_u128(2); 4], [20.0, 22.0, 40.0, 42.0], [80.0, 81.0, 160.0, 161.0]);
        let west_weights = blend_weights(&west, 256.0, 64.0, 34.5, 256.0, 64.0);
        let east_weights = blend_weights(&east, 0.0, 64.0, 34.5, 256.0, 64.0);
        assert_eq!(west_weights, east_weights);
        assert_ne!(west.detail_textures(), east.detail_textures());
    }
}
