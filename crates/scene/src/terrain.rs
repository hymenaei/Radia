use std::{
    array,
    sync::atomic::{AtomicBool, Ordering},
};

use math::{DVec3, Vec2, Vec3};
use world::{Region, TerrainPatchId};

use crate::{Face, Mesh, MeshError, Patch, Vertex, model::Mirror};

struct Tile {
    patch: Patch,
    x: Option<usize>,
    y: Option<usize>,
}

pub(crate) struct Terrain([Option<Tile>; 9]);

impl Terrain {
    pub(crate) fn complete(&self) -> bool {
        self.0.iter().all(Option::is_some)
    }

    pub(crate) fn capture(mirror: &Mirror, region: Region, patch: TerrainPatchId) -> Self {
        let (x, y) = patch.coordinates();
        Self(array::from_fn(|index| {
            let (region, patch, x, y) = locate(mirror, region, i32::from(x) + index as i32 % 3 - 1, i32::from(y) + index as i32 / 3 - 1)?;
            Some(Tile { patch: mirror.patches.get(&(region, patch))?.clone(), x, y })
        }))
    }

    pub(crate) fn mesh(&self, cancelled: &AtomicBool) -> Result<Mesh, MeshError> {
        ground(&self.0, cancelled)
    }
}

pub(crate) fn affected_patches(mirror: &Mirror, region: Region, patch: TerrainPatchId) -> impl Iterator<Item = (Region, TerrainPatchId)> {
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

fn ground(tiles: &[Option<Tile>; 9], cancelled: &AtomicBool) -> Result<Mesh, MeshError> {
    let mut vertices = Vec::with_capacity(289);
    let mut indices = Vec::with_capacity(1536);
    let mut grid = [[None; 17]; 17];
    for y in 0..17 {
        if cancelled.load(Ordering::Acquire) {
            return Err(MeshError::Cancelled);
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
            grid[y as usize][x as usize] = Some(vertices.len() as u32);
            vertices.push(Vertex { position: Vec3::new(x as f32, y as f32, z as f32), normal, uv: Vec2::new(x as f32, y as f32) / 16.0 });
        }
    }
    for y in 0..16 {
        for x in 0..16 {
            let [Some(a), Some(b), Some(c), Some(d)] = [grid[y][x], grid[y][x + 1], grid[y + 1][x], grid[y + 1][x + 1]] else { continue };
            indices.extend([a, b, d, a, d, c]);
        }
    }
    let count = indices.len() as u32;
    Mesh::from_parts(vertices, indices, vec![Face { slot: 0, indices: 0..count }])
}
