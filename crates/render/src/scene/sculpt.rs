use std::{f32::consts::PI, sync::atomic::AtomicBool};

use assets::{MAX_SOURCE_EDGE, mesh::Mesh};
use math::{Vec2, Vec3};
use world::{SculptFlags, SculptType};

use super::{Detail, MeshBuildError, prim::Builder};

const MIN_SCULPT_MAP_CELLS: usize = 16;
const MIN_SURFACE_AREA: f32 = 0.002;
const MAX_SURFACE_AREA: f32 = 384.0;
const PLACEHOLDER_RADIUS: f32 = 0.3;

pub fn tessellate_sculpt(
    size: [u32; 2],
    rgba: &[u8],
    sculpt_type: SculptType,
    flags: SculptFlags,
    detail: Detail,
) -> Result<Mesh, MeshBuildError> {
    let [width, height] = size;
    if width == 0 || height == 0 || width > MAX_SOURCE_EDGE || height > MAX_SOURCE_EDGE {
        return Err(MeshBuildError::TooComplex);
    }
    let pixel_count = usize::try_from(width)
        .ok()
        .and_then(|width| usize::try_from(height).ok().and_then(|height| width.checked_mul(height)))
        .ok_or(MeshBuildError::TooComplex)?;
    let required_bytes = pixel_count.checked_mul(4).ok_or(MeshBuildError::TooComplex)?;
    if rgba.len() != required_bytes {
        return Err(MeshBuildError::InvalidParameters);
    }

    let map_too_small = pixel_count / 4 < MIN_SCULPT_MAP_CELLS;
    let (sides_s, sides_t) = if map_too_small { (4, 4) } else { mesh_resolution(width, height, detail, pixel_count) };
    let rows = sides_s + 1;
    let columns = sides_t + 1;
    let vertex_count = rows.checked_mul(columns).ok_or(MeshBuildError::TooComplex)?;
    let reverse_horizontal = flags.invert ^ flags.mirror;
    let mut positions = if map_too_small {
        placeholder(rows, columns)
    } else {
        map_positions(size, rgba, rows, columns, sculpt_type, flags, reverse_horizontal)?
    };

    if !map_too_small && detail != Detail::Lowest {
        let area = surface_area(&positions, rows, columns);
        if !area.is_finite() || !(MIN_SURFACE_AREA..=MAX_SURFACE_AREA).contains(&area) {
            positions = placeholder(rows, columns);
        }
    }

    let mut mesh = Builder::default();
    let start = mesh.begin_face();
    for row in 0..rows {
        let v = row as f32 / (rows - 1) as f32;
        for column in 0..columns {
            let mut u = column as f32 / (columns - 1) as f32;
            if reverse_horizontal {
                u = 1.0 - u;
            }
            let index = row * columns + column;
            mesh.vertex(positions[index], Vec2::new(u, v))?;
        }
    }
    for row in 0..rows - 1 {
        for column in 0..columns - 1 {
            let a = row * columns + column;
            mesh.triangle(a, a + columns + 1, a + columns)?;
            mesh.triangle(a, a + 1, a + columns + 1)?;
        }
    }
    mesh.end_face(start)?;

    if sculpt_type != SculptType::Plane {
        for row in 0..rows {
            let first = row * columns;
            mesh.seam(first, first + columns - 1);
        }
    }
    match sculpt_type {
        SculptType::Sphere => {
            for column in 1..columns {
                mesh.seam(0, column);
                let last_row = (rows - 1) * columns;
                mesh.seam(last_row, last_row + column);
            }
        }
        SculptType::Torus => {
            let last_row = (rows - 1) * columns;
            for column in 0..columns {
                mesh.seam(column, last_row + column);
            }
        }
        SculptType::Plane | SculptType::Cylinder => {}
    }

    let cancelled = AtomicBool::new(false);
    let result = mesh.finish(&cancelled)?;
    debug_assert_eq!(result.vertices().len(), vertex_count);
    Ok(result)
}

fn mesh_resolution(width: u32, height: u32, detail: Detail, pixel_count: usize) -> (usize, usize) {
    let detail_scale = detail.scale();
    let sides = if detail_scale <= 1.0 {
        6
    } else if detail_scale <= 2.0 {
        8
    } else if detail_scale <= 3.0 {
        16
    } else {
        32
    };
    let map_cells = pixel_count / 4;
    let cells = (sides * sides).min(map_cells);
    let ratio = width as f32 / height as f32;
    let mut sides_s = ((cells as f32 / ratio).sqrt() as usize).max(4);
    let sides_t = (cells / sides_s).max(4);
    sides_s = cells / sides_t;
    (sides_s, sides_t)
}

fn map_positions(
    size: [u32; 2],
    rgba: &[u8],
    rows: usize,
    columns: usize,
    sculpt_type: SculptType,
    flags: SculptFlags,
    reverse_horizontal: bool,
) -> Result<Vec<Vec3>, MeshBuildError> {
    let [width, height] = size;
    let mut positions = Vec::with_capacity(rows * columns);
    for row in 0..rows {
        for column in 0..columns {
            let mut y = ((row as f32 / (rows - 1) as f32) * height as f32) as u32;
            let reversed_column = if reverse_horizontal { columns - column - 1 } else { column };
            let mut x = ((reversed_column as f32 / (columns - 1) as f32) * width as f32) as u32;
            if y == 0 && sculpt_type == SculptType::Sphere {
                x = width / 2;
            }
            if y == height {
                y = if sculpt_type == SculptType::Torus { 0 } else { height - 1 };
                if sculpt_type == SculptType::Sphere {
                    x = width / 2;
                }
            }
            if x == width {
                x = if sculpt_type == SculptType::Plane { width - 1 } else { 0 };
            }
            let pixel_index = usize::try_from(y)
                .ok()
                .and_then(|y| y.checked_mul(width as usize))
                .and_then(|row| row.checked_add(x as usize))
                .and_then(|pixel| pixel.checked_mul(4))
                .ok_or(MeshBuildError::InvalidParameters)?;
            let end = pixel_index.checked_add(4).ok_or(MeshBuildError::InvalidParameters)?;
            let pixel: [u8; 4] = rgba
                .get(pixel_index..end)
                .ok_or(MeshBuildError::InvalidParameters)?
                .try_into()
                .map_err(|_| MeshBuildError::InvalidParameters)?;
            let mut position = Vec3::new(
                pixel[0] as f32 * (1.0 / 255.0) - 0.5,
                pixel[1] as f32 * (1.0 / 255.0) - 0.5,
                pixel[2] as f32 * (1.0 / 255.0) - 0.5,
            );
            if flags.mirror {
                position.x = -position.x;
            }
            positions.push(position);
        }
    }
    Ok(positions)
}

fn placeholder(rows: usize, columns: usize) -> Vec<Vec3> {
    let mut positions = Vec::with_capacity(rows * columns);
    for row in 0..rows {
        let u = row as f32 / (rows - 1) as f32;
        for column in 0..columns {
            let v = column as f32 / (columns - 1) as f32;
            let latitude = PI * v;
            let longitude = 2.0 * PI * u;
            positions.push(Vec3::new(
                latitude.sin() * longitude.cos() * PLACEHOLDER_RADIUS,
                latitude.sin() * longitude.sin() * PLACEHOLDER_RADIUS,
                latitude.cos() * PLACEHOLDER_RADIUS,
            ));
        }
    }
    positions
}

fn surface_area(positions: &[Vec3], rows: usize, columns: usize) -> f32 {
    let mut area = 0.0;
    for row in 0..rows - 1 {
        for column in 0..columns - 1 {
            let p1 = positions[row * columns + column];
            let p2 = positions[(row + 1) * columns + column];
            let p3 = positions[row * columns + column + 1];
            let p4 = positions[(row + 1) * columns + column + 1];
            let cross1 = (p1 - p2).cross(p1 - p3);
            let cross2 = (p4 - p2).cross(p4 - p3);
            area += (cross1.length() + cross2.length()) * 0.5;
        }
    }
    area
}

#[cfg(test)]
#[path = "sculpt_tests.rs"]
mod tests;
