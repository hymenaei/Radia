use std::{
    f32::consts::{PI, TAU},
    sync::atomic::{AtomicBool, Ordering},
};

use assets::mesh::{Face, MAX_MESH_INDICES, MAX_MESH_VERTICES, Mesh, Vertex};
use math::{FloatExt, Quat, Vec2, Vec3};
use world::PrimShape;

use super::mesh::{Detail, MeshBuildError};

const MAX_PRIM_FACES: usize = 9;

#[derive(Default)]
struct Builder {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    faces: Vec<Face>,
    parents: Vec<usize>,
}

impl Builder {
    fn vertex(&mut self, position: Vec3, uv: Vec2) -> Result<usize, MeshBuildError> {
        if !position.is_finite() || !uv.is_finite() {
            return Err(MeshBuildError::InvalidParameters);
        }
        if self.vertices.len() == MAX_MESH_VERTICES {
            return Err(MeshBuildError::TooComplex);
        }
        let index = self.vertices.len();
        self.vertices.push(Vertex { position, normal: Vec3::ZERO, uv });
        self.parents.push(index);
        Ok(index)
    }

    fn triangle(&mut self, a: usize, b: usize, c: usize) -> Result<(), MeshBuildError> {
        if self.indices.len() + 3 > MAX_MESH_INDICES {
            return Err(MeshBuildError::TooComplex);
        }
        let cross = (self.vertices[b].position - self.vertices[a].position).cross(self.vertices[c].position - self.vertices[a].position);
        if cross.length_squared() > 1e-20 {
            self.indices.extend([a as u32, b as u32, c as u32]);
        }
        Ok(())
    }

    fn begin_face(&self) -> usize {
        self.indices.len()
    }

    fn end_face(&mut self, start: usize) -> Result<(), MeshBuildError> {
        if self.faces.len() == MAX_PRIM_FACES {
            return Err(MeshBuildError::TooComplex);
        }
        self.faces.push(Face { slot: self.faces.len() as u8, indices: start as u32..self.indices.len() as u32 });
        Ok(())
    }

    fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    fn seam(&mut self, a: usize, b: usize) {
        if self.vertices[a].position.distance_squared(self.vertices[b].position) <= 1e-12 {
            let a = root(&mut self.parents, a);
            let b = root(&mut self.parents, b);
            self.parents[b] = a;
        }
    }

    fn finish(mut self, cancelled: &AtomicBool) -> Result<Mesh, MeshBuildError> {
        if self.indices.is_empty() {
            return Err(MeshBuildError::Degenerate);
        }
        let mut normals = vec![Vec3::ZERO; self.vertices.len()];
        for (ordinal, triangle) in self.indices.as_chunks::<3>().0.iter().enumerate() {
            if ordinal % 256 == 0 && cancelled.load(Ordering::Acquire) {
                return Err(MeshBuildError::Cancelled);
            }
            let [a, b, c] = [triangle[0] as usize, triangle[1] as usize, triangle[2] as usize];
            let normal =
                (self.vertices[b].position - self.vertices[a].position).cross(self.vertices[c].position - self.vertices[a].position);
            for index in [a, b, c] {
                normals[root(&mut self.parents, index)] += normal;
            }
        }
        for (index, vertex) in self.vertices.iter_mut().enumerate() {
            vertex.normal = normals[root(&mut self.parents, index)].try_normalize().unwrap_or(Vec3::Z);
        }
        Mesh::from_parts(self.vertices, self.indices, self.faces).map_err(MeshBuildError::from)
    }
}

fn root(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        parents[index] = parents[parents[index]];
        index = parents[index];
    }
    index
}

#[derive(Clone, Copy)]
struct Point {
    position: Vec2,
    t: f32,
}

struct Profile {
    outer: Vec<Point>,
    inner: Vec<Point>,
    sides: Option<usize>,
    inner_flat: bool,
    open: bool,
    uv_scale: f32,
}

struct Ring {
    position: Vec3,
    rotation: Quat,
    scale: Vec2,
    t: f32,
}

impl Ring {
    fn point(&self, point: Point) -> Vec3 {
        self.position + self.rotation * (point.position * self.scale).extend(0.0)
    }
}

struct Params {
    profile: u8,
    hole: u8,
    path: u8,
    s: [f32; 2],
    t: [f32; 2],
    hollow: f32,
    scale: Vec2,
    shear: Vec2,
    twist: [f32; 2],
    taper: Vec2,
    revolutions: f32,
    radius: f32,
    skew: f32,
}

impl Params {
    fn new(shape: PrimShape) -> Result<Self, MeshBuildError> {
        let profile = shape.profile_curve & 15;
        let hole = shape.profile_curve >> 4;
        let path = shape.path_curve & 0xf0;
        if profile > 5 || hole > 3 || shape.path_curve & 15 != 0 || ![0x10, 0x20, 0x30, 0x40, 0x80].contains(&path) {
            return Err(MeshBuildError::UnsupportedShape);
        }
        let s = [f32::from(shape.profile_begin) / 50000.0, 1.0 - f32::from(shape.profile_end) / 50000.0];
        let t = [f32::from(shape.path_begin) / 50000.0, 1.0 - f32::from(shape.path_end) / 50000.0];
        let hollow = f32::from(shape.profile_hollow) / 50000.0;
        let scale = Vec2::new(2.0 - f32::from(shape.path_scale_x) / 100.0, 2.0 - f32::from(shape.path_scale_y) / 100.0);
        let shear = Vec2::new(f32::from(shape.path_shear_x), f32::from(shape.path_shear_y)) / 100.0;
        let twist = [f32::from(shape.path_twist_begin) / 100.0, f32::from(shape.path_twist) / 100.0];
        let taper = Vec2::new(f32::from(shape.path_taper_x), f32::from(shape.path_taper_y)) / 100.0;
        let revolutions = 1.0 + f32::from(shape.path_revolutions) * 0.015;
        let mut radius = f32::from(shape.path_radius_offset) / 100.0;
        let mut skew = f32::from(shape.path_skew) / 100.0;
        let max_hollow = if hole == 2 && [0, 3, 5].contains(&profile) { 0.7 } else { 0.95 };
        if s[0] < 0.0
            || s[1] > 1.0
            || s[1] - s[0] < 0.01999
            || t[0] < 0.0
            || t[1] > 1.0
            || t[1] - t[0] < 0.01999
            || hollow > max_hollow
            || scale.min_element() < 0.0
            || scale.max_element() > 2.0
            || shear.abs().max_element() > 0.5
            || taper.abs().max_element() > 1.0
            || twist.into_iter().any(|v| v.abs() > 1.0)
            || revolutions > 4.0
            || skew.abs() > 0.95
        {
            return Err(MeshBuildError::InvalidParameters);
        }
        if path == 0x20 && profile != 5 && (scale.x < 0.05 || scale.x > 1.0 || scale.y < 0.05 || scale.y > 0.5) {
            return Err(MeshBuildError::InvalidParameters);
        }
        if path == 0x20 && profile != 5 {
            let taper_y = if radius * taper.y < 0.0 { 0.0 } else { taper.y.abs() };
            let max_radius = 1.0 - scale.y * (1.0 - taper_y) / (1.0 - scale.y);
            radius = radius.clamp(-max_radius, max_radius);
        } else {
            radius = 0.0;
        }
        if revolutions > 1.001 {
            let min_skew = 1.0 - 1.0 / (revolutions * scale.x + 1.0);
            if skew.abs() < min_skew {
                skew = if skew < 0.0 { -min_skew } else { min_skew };
            }
        }
        Ok(Self { profile, hole, path, s, t, hollow, scale, shear, twist, taper, revolutions, radius, skew })
    }
}

pub(super) fn tessellate(shape: PrimShape, detail: Detail, cancelled: &AtomicBool) -> Result<Mesh, MeshBuildError> {
    check(cancelled)?;
    let params = Params::new(shape)?;
    let profile = profile(&params, detail);
    let (path, closed) = path(&params, detail);
    let mut mesh = Builder::default();
    if !closed {
        cap(&mut mesh, &profile, path.last().expect("nonempty bounded path"), true)?;
    }
    check(cancelled)?;
    if let Some(sides) = profile.sides {
        let mut begin = 0;
        while begin + 1 < profile.outer.len() {
            let side = ((profile.outer[begin].t + profile.outer[begin + 1].t) * 0.5 * sides as f32).floor() as usize;
            let mut end = begin + 1;
            while end + 1 < profile.outer.len()
                && ((profile.outer[end].t + profile.outer[end + 1].t) * 0.5 * sides as f32).floor() as usize == side
            {
                end += 1;
            }
            let start = mesh.begin_face();
            side_grid(&mut mesh, &profile.outer[begin..=end], &path, profile.uv_scale, false, false, closed, cancelled)?;
            mesh.end_face(start)?;
            begin = end;
        }
    } else {
        let start = mesh.begin_face();
        side_grid(&mut mesh, &profile.outer, &path, 1.0, false, !profile.open, closed, cancelled)?;
        mesh.end_face(start)?;
    }
    if !profile.inner.is_empty() {
        let points: Vec<_> = profile.inner.iter().rev().copied().collect();
        let start = mesh.begin_face();
        let uv_scale = params.hollow * if matches!(params.profile, 2..=4) { 0.5 } else { 1.0 };
        if profile.inner_flat {
            for edge in points.windows(2) {
                side_grid(&mut mesh, edge, &path, uv_scale, false, false, closed, cancelled)?;
            }
        } else {
            side_grid(&mut mesh, &points, &path, uv_scale, false, !profile.open, closed, cancelled)?;
        }
        mesh.end_face(start)?;
    }
    if !closed {
        cap(&mut mesh, &profile, &path[0], false)?;
    }
    if profile.open {
        let outer_begin = profile.outer[0];
        let outer_end = *profile.outer.last().expect("profile endpoints");
        let center = Point { position: Vec2::ZERO, t: 0.0 };
        let inner_begin = profile.inner.first().copied().unwrap_or(center);
        let inner_end = profile.inner.last().copied().unwrap_or(center);
        for edge in [[inner_begin, outer_begin], [outer_end, inner_end]] {
            let start = mesh.begin_face();
            side_grid(&mut mesh, &edge, &path, 1.0, true, false, closed, cancelled)?;
            mesh.end_face(start)?;
        }
    }
    check(cancelled)?;
    mesh.finish(cancelled)
}

fn check(cancelled: &AtomicBool) -> Result<(), MeshBuildError> {
    if cancelled.load(Ordering::Acquire) { Err(MeshBuildError::Cancelled) } else { Ok(()) }
}

fn polygon(sides: usize, offset: f32, span: f32, cut: [f32; 2], split: usize, factor: f32) -> Vec<Point> {
    let full_sides = (sides as f32 / span).round() as usize;
    let radius = match full_sides {
        3 => 0.5,
        4 => std::f32::consts::FRAC_1_SQRT_2,
        5 => 0.53,
        6 => 0.525,
        7 => 0.5,
        _ => 0.5,
    } * factor;
    let point = |t: f32| {
        let base = (t * sides as f32).floor();
        let fraction = t * sides as f32 - base;
        let angle = TAU * (base / sides as f32 * span + offset);
        let a = Vec2::new(angle.cos(), angle.sin()) * radius;
        let end_angle = angle + TAU * span / sides as f32;
        let b = Vec2::new(end_angle.cos(), end_angle.sin()) * radius;
        Point { position: a.lerp(b, fraction), t }
    };
    let mut points = vec![point(cut[0])];
    let first = (cut[0] * sides as f32).floor() as usize + 1;
    let last = (cut[1] * sides as f32).ceil() as usize;
    for sample in first..=last {
        let t = (sample as f32 / sides as f32).min(cut[1]);
        let previous = points.last().expect("first profile point").t;
        if t <= previous + 1e-7 {
            continue;
        }
        for i in 1..=split {
            points.push(point(previous + (t - previous) * i as f32 / (split + 1) as f32));
        }
        points.push(point(t));
    }
    points
}

fn profile(params: &Params, detail: Detail) -> Profile {
    let mut split = (detail.scale() * 0.66).floor() as usize;
    let sides = match params.profile {
        1 => Some(4),
        2..=4 => Some(3),
        _ => None,
    };
    if [0x10, 0x80].contains(&params.path) && params.scale != Vec2::ONE && sides.is_some() {
        split = 0;
    }
    let half = params.profile == 5;
    let mut count = sides.unwrap_or_else(|| (6.0 * detail.scale() * if half { 0.5 } else { 1.0 }).floor() as usize);
    if params.hollow > 0.0 && params.hole == 2 && sides.is_none() {
        let divisor = if half { 2 } else { 4 };
        count = count.div_ceil(divisor) * divisor;
    }
    let offset = if half {
        0.5
    } else if params.profile == 1 {
        -0.375
    } else {
        0.0
    };
    let span = if half { 0.5 } else { 1.0 };
    let mut outer = polygon(count, offset, span, params.s, if sides.is_some() { split } else { 0 }, 1.0);
    if half && params.hollow == 0.0 && params.s == [0.0, 1.0] {
        outer.push(outer[0]);
    }
    let open = if half { params.s != [0.0, 1.0] || params.hollow > 0.0 } else { params.s[1] - params.s[0] < 0.99 };
    let inner_flat = match params.hole {
        0 => sides.is_some(),
        2 | 3 => true,
        _ => false,
    };
    let inner = if params.hollow > 0.0 {
        let inner_count = match params.hole {
            1 => (6.0 * detail.scale() * span).floor() as usize,
            2 => {
                if half {
                    2
                } else {
                    4
                }
            }
            3 => 3,
            _ => count,
        };
        let factor = params.hollow * if matches!(params.profile, 2..=4) { 0.5 } else { 1.0 };
        polygon(inner_count, offset, span, params.s, if inner_flat { split } else { 0 }, factor)
    } else {
        Vec::new()
    };
    Profile { outer, inner, sides, inner_flat, open, uv_scale: sides.map_or(1.0, |sides| sides as f32) }
}

fn path(params: &Params, detail: Detail) -> (Vec<Ring>, bool) {
    let twist = (params.twist[1] - params.twist[0]).abs();
    if [0x10, 0x80].contains(&params.path) {
        let split =
            if params.scale != Vec2::ONE && (1..=4).contains(&params.profile) { 0 } else { (detail.scale() * 0.66).floor() as usize };
        let count = ((twist * 3.5 * (detail.scale() - 0.5)).floor() as usize + 2).max(split + 2);
        let begin = (Vec2::splat(2.0) - params.scale).min(Vec2::ONE);
        let end = params.scale.min(Vec2::ONE);
        return (
            (0..count)
                .map(|i| {
                    let t = params.t[0] + (params.t[1] - params.t[0]) * i as f32 / (count - 1) as f32;
                    Ring {
                        position: (params.shear * t).extend(t - 0.5),
                        rotation: Quat::from_rotation_z(PI * params.twist[0].lerp_monotonic(params.twist[1], t)),
                        scale: begin.lerp(end, t),
                        t,
                    }
                })
                .collect(),
            false,
        );
    }
    if params.path == 0x40 {
        return (
            (0..5)
                .map(|i| {
                    let t = i as f32 / 4.0;
                    let angle = PI * params.twist[1] * t;
                    Ring {
                        position: Vec3::new(0.0, -angle.sin() * 0.5 * t, -0.5 + (angle.cos() * 0.5 + 0.5) * t),
                        rotation: Quat::from_rotation_x(angle),
                        scale: Vec2::ONE.lerp(params.scale, t),
                        t,
                    }
                })
                .collect(),
            false,
        );
    }
    let sides = if params.path == 0x30 {
        (6.0 * detail.scale()).floor() as usize
    } else {
        (((6.0 * detail.scale() + twist * 3.5 * (detail.scale() - 0.5)).floor()) * params.revolutions).floor() as usize
    };
    let radius = match sides {
        3 => 0.5,
        4 => std::f32::consts::FRAC_1_SQRT_2,
        5 => 0.53,
        6 => 0.525,
        _ => 0.5,
    } * (1.0 - params.scale.y);
    let radius_offset = if params.path == 0x20 && params.profile != 5 { params.radius } else { 0.0 };
    let radii = if radius_offset < 0.0 { [radius * (1.0 + radius_offset), radius] } else { [radius, radius * (1.0 - radius_offset)] };
    let begin = (Vec2::ONE + params.taper).min(Vec2::ONE);
    let end = (Vec2::ONE - params.taper).min(Vec2::ONE);
    let closed = params.t == [0.0, 1.0]
        && params.skew.abs() <= 0.001
        && (end - begin).abs().max_element() <= 0.001
        && (radii[1] - radii[0]).abs() <= 0.001
        && params.twist[0] == params.twist[1];
    let mut samples = vec![params.t[0]];
    for i in (params.t[0] * sides as f32).floor() as usize + 1..=(params.t[1] * sides as f32).ceil() as usize {
        let t = (i as f32 / sides as f32).min(params.t[1]);
        if t > *samples.last().expect("path beginning") + 1e-7 {
            samples.push(t);
        }
    }
    let rings = samples
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            let angle = TAU * params.revolutions * t;
            let radius = radii[0].lerp_monotonic(radii[1], t);
            let s = angle.sin() * radius;
            let c = angle.cos() * radius;
            let x = if params.path == 0x30 {
                if i % 2 == 0 { 0.5 } else { -0.5 }
            } else {
                params.shear.x * s + (-params.skew + 2.0 * params.skew * t) * 0.5
            };
            Ring {
                position: Vec3::new(x, c + params.shear.y * s, s),
                rotation: Quat::from_rotation_x(angle)
                    * Quat::from_rotation_z(TAU * params.twist[0].lerp_monotonic(params.twist[1], t) - PI),
                scale: params.scale * Vec2::new(1.0 - params.skew.abs(), 1.0) * begin.lerp(end, t),
                t,
            }
        })
        .collect();
    (rings, closed)
}

fn side_grid(
    mesh: &mut Builder,
    points: &[Point],
    path: &[Ring],
    uv_scale: f32,
    cut: bool,
    wrap_s: bool,
    wrap_t: bool,
    cancelled: &AtomicBool,
) -> Result<(), MeshBuildError> {
    let base = mesh.vertex_count();
    let width = points.len();
    let begin_uv = if uv_scale > 1.0 { (points[0].t * uv_scale).floor() } else { 0.0 };
    for ring in path {
        check(cancelled)?;
        for (i, point) in points.iter().enumerate() {
            let s = if cut { i as f32 } else { point.t * uv_scale - begin_uv };
            mesh.vertex(ring.point(*point), Vec2::new(s, ring.t))?;
        }
    }
    for row in 0..path.len() - 1 {
        check(cancelled)?;
        for column in 0..width - 1 {
            let a = base + row * width + column;
            mesh.triangle(a, a + width + 1, a + width)?;
            mesh.triangle(a, a + 1, a + width + 1)?;
        }
    }
    if wrap_s {
        for row in 0..path.len() {
            mesh.seam(base + row * width, base + row * width + width - 1);
        }
    }
    if wrap_t {
        for column in 0..width {
            mesh.seam(base + column, base + (path.len() - 1) * width + column);
        }
    }
    for (column, point) in points.iter().enumerate() {
        if path.iter().all(|ring| ring.point(*point).distance_squared(path[0].point(*point)) <= 1e-12) {
            for row in 1..path.len() {
                mesh.seam(base + column, base + row * width + column);
            }
        }
    }
    Ok(())
}

fn cap(mesh: &mut Builder, profile: &Profile, ring: &Ring, top: bool) -> Result<(), MeshBuildError> {
    let start = mesh.begin_face();
    let vertex = |mesh: &mut Builder, point: Point| {
        mesh.vertex(ring.point(point), Vec2::new(point.position.x + 0.5, if top { point.position.y + 0.5 } else { 0.5 - point.position.y }))
    };
    let outer: Vec<_> = profile.outer.iter().copied().map(|point| vertex(mesh, point)).collect::<Result<_, _>>()?;
    let triangle = |mesh: &mut Builder, a, b, c| {
        if top { mesh.triangle(a, b, c) } else { mesh.triangle(a, c, b) }
    };
    if profile.inner.is_empty() {
        let center = vertex(mesh, Point { position: Vec2::ZERO, t: 0.0 })?;
        for edge in outer.windows(2) {
            triangle(mesh, center, edge[0], edge[1])?;
        }
    } else {
        let inner: Vec<_> = profile.inner.iter().copied().map(|point| vertex(mesh, point)).collect::<Result<_, _>>()?;
        let mut a = 0;
        let mut b = 0;
        while a + 1 < outer.len() || b + 1 < inner.len() {
            if b + 1 == inner.len() || (a + 1 < outer.len() && profile.outer[a + 1].t <= profile.inner[b + 1].t) {
                triangle(mesh, outer[a], outer[a + 1], inner[b])?;
                a += 1;
            } else {
                triangle(mesh, outer[a], inner[b + 1], inner[b])?;
                b += 1;
            }
        }
    }
    mesh.end_face(start)
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::{collections::HashMap, f32::consts::PI, sync::atomic::AtomicBool};

    use assets::mesh::{MAX_MESH_INDICES, MAX_MESH_VERTICES, Mesh};
    use math::{Vec2, Vec3};
    use proptest::prelude::*;
    use world::PrimShape;

    use super::{Detail, MeshBuildError, tessellate};

    fn generate(shape: PrimShape) -> Mesh {
        tessellate(shape, Detail::High, &AtomicBool::new(false)).unwrap()
    }

    fn volume(mesh: &Mesh) -> f32 {
        mesh.indices()
            .as_chunks::<3>()
            .0
            .iter()
            .map(|tri| {
                let a = mesh.vertices()[tri[0] as usize].position;
                let b = mesh.vertices()[tri[1] as usize].position;
                let c = mesh.vertices()[tri[2] as usize].position;
                a.dot(b.cross(c)) / 6.0
            })
            .sum()
    }

    fn close(a: Vec3, b: Vec3) {
        assert!(a.distance(b) < 2e-5, "{a:?} != {b:?}");
    }

    fn position_key(position: Vec3) -> [i32; 3] {
        position.to_array().map(|component| (component * 100_000.0).round() as i32)
    }

    fn assert_closed(mesh: &Mesh, shape: &str) {
        let mut edges: HashMap<([i32; 3], [i32; 3]), (usize, i32)> = HashMap::new();
        for triangle in mesh.indices().as_chunks::<3>().0 {
            let points = triangle.map(|index| position_key(mesh.vertices()[index as usize].position));
            for (start, end) in [(points[0], points[1]), (points[1], points[2]), (points[2], points[0])] {
                if start == end {
                    continue;
                }
                let (edge, direction) = if start < end { ((start, end), 1) } else { ((end, start), -1) };
                let entry = edges.entry(edge).or_default();
                entry.0 += 1;
                entry.1 += direction;
            }
        }
        assert!(!edges.is_empty(), "{shape} must contain triangles");
        for (edge, (count, direction)) in edges {
            assert!(count >= 2 && count.is_multiple_of(2), "{shape} edge {edge:?} has a boundary");
            assert_eq!(direction, 0, "{shape} edge {edge:?} must have opposite winding");
        }
    }

    fn assert_lod_invariants(mesh: &Mesh, detail: Detail) -> prop::test_runner::TestCaseResult {
        prop_assert!(mesh.vertices().len() <= MAX_MESH_VERTICES, "{detail:?} vertex budget");
        prop_assert!(
            !mesh.indices().is_empty() && mesh.indices().len().is_multiple_of(3) && mesh.indices().len() / 3 <= MAX_MESH_INDICES / 3,
            "{detail:?} triangle budget"
        );
        prop_assert!(mesh.faces().len() <= 9, "{detail:?} face budget");
        prop_assert!(mesh.bounds().min.cmplt(mesh.bounds().max).all(), "{detail:?} bounds have nonzero extent on every axis");
        prop_assert!(mesh.indices().iter().all(|index| (*index as usize) < mesh.vertices().len()), "{detail:?} index is in range");
        for vertex in mesh.vertices() {
            prop_assert!(vertex.position.is_finite() && vertex.uv.is_finite() && vertex.normal.is_finite());
            prop_assert!(vertex.position.length() <= 3.0, "normalized bound used by camera planning");
            prop_assert!((vertex.normal.length() - 1.0).abs() < 1e-5);
            prop_assert!(vertex.position.cmpge(mesh.bounds().min).all() && vertex.position.cmple(mesh.bounds().max).all());
        }
        for (slot, face) in mesh.faces().iter().enumerate() {
            prop_assert_eq!(usize::from(face.slot), slot);
            prop_assert!(face.indices.start <= face.indices.end && face.indices.end as usize <= mesh.indices().len());
        }
        Ok(())
    }

    #[test]
    fn box_face_layout() {
        let mesh = generate(PrimShape::default());
        assert_eq!(mesh.faces().len(), 6);
        close(mesh.bounds().min, Vec3::splat(-0.5));
        close(mesh.bounds().max, Vec3::splat(0.5));
        assert!((volume(&mesh) - 1.0).abs() < 1e-5);
        for (slot, (face, expected)) in mesh.faces().iter().zip([Vec3::Z, -Vec3::Y, Vec3::X, Vec3::Y, -Vec3::X, -Vec3::Z]).enumerate() {
            assert_eq!(face.slot, slot as u8, "box face slots follow Second Life material order");
            for index in &mesh.indices()[face.indices.start as usize..face.indices.end as usize] {
                let vertex = mesh.vertices()[*index as usize];
                close(vertex.normal, expected);
                if face.slot == 0 {
                    assert!(vertex.uv.distance(Vec2::new(vertex.position.x + 0.5, vertex.position.y + 0.5)) < 1e-5);
                }
                if face.slot == 5 {
                    assert!(vertex.uv.distance(Vec2::new(vertex.position.x + 0.5, 0.5 - vertex.position.y)) < 1e-5);
                }
            }
        }
    }

    #[test]
    fn modified_prim_volumes() {
        let cases = [
            (PrimShape { profile_hollow: 25000, ..PrimShape::default() }, 0.75),
            (PrimShape { profile_begin: 6250, profile_end: 6250, ..PrimShape::default() }, 0.75),
            (PrimShape { profile_begin: 6250, profile_end: 6250, profile_hollow: 12500, ..PrimShape::default() }, 0.703125),
            (PrimShape { path_shear_x: 25, path_shear_y: -20, ..PrimShape::default() }, 1.0),
            (PrimShape { path_scale_x: 150, path_scale_y: 150, ..PrimShape::default() }, 7.0 / 12.0),
            (PrimShape { path_begin: 12500, path_end: 12500, ..PrimShape::default() }, 0.5),
        ];
        for (shape, expected) in cases {
            let mesh = generate(shape);
            assert!((volume(&mesh) - expected).abs() < 2e-5, "shape {shape:?}: {} != {expected}", volume(&mesh));
        }
        let hollow = generate(PrimShape { profile_hollow: 25000, ..PrimShape::default() });
        assert_eq!(hollow.faces().len(), 7);
        for face in [hollow.faces().first().unwrap(), hollow.faces().last().unwrap()] {
            for triangle in hollow.indices()[face.indices.start as usize..face.indices.end as usize].as_chunks::<3>().0 {
                let [a, b, c] = [triangle[0], triangle[1], triangle[2]].map(|i| hollow.vertices()[i as usize].position.truncate());
                let signs = [(b - a).perp_dot(-a), (c - b).perp_dot(-b), (a - c).perp_dot(-c)];
                assert!(!(signs.iter().all(|s| *s > 1e-7) || signs.iter().all(|s| *s < -1e-7)), "cap must leave the hollow center empty");
            }
        }
    }

    #[test]
    fn base_prim_geometry() {
        let cylinder = generate(PrimShape { profile_curve: 0, ..PrimShape::default() });
        assert_eq!(cylinder.faces().len(), 3);
        close(cylinder.bounds().min, Vec3::splat(-0.5));
        close(cylinder.bounds().max, Vec3::splat(0.5));
        assert!((volume(&cylinder) - PI * 0.25).abs() < 0.02);
        let sphere = generate(PrimShape { path_curve: 0x20, profile_curve: 5, ..PrimShape::default() });
        assert_eq!(sphere.faces().len(), 1);
        close(sphere.bounds().min, Vec3::splat(-0.5));
        close(sphere.bounds().max, Vec3::splat(0.5));
        assert!((volume(&sphere) - PI / 6.0).abs() < 0.03, "sphere volume {}", volume(&sphere));
        let torus =
            generate(PrimShape { path_curve: 0x20, profile_curve: 0, path_scale_x: 150, path_scale_y: 150, ..PrimShape::default() });
        assert_eq!(torus.faces().len(), 1);
        close(torus.bounds().min, Vec3::new(-0.25, -0.5, -0.5));
        close(torus.bounds().max, Vec3::new(0.25, 0.5, 0.5));
        assert!((volume(&torus) - PI * PI / 32.0).abs() < 0.02);
        for profile in 2..=4 {
            let triangle = generate(PrimShape { profile_curve: profile, ..PrimShape::default() });
            assert_eq!(triangle.faces().len(), 5);
            assert!((volume(&triangle) - 3.0_f32.sqrt() * 3.0 / 16.0).abs() < 1e-5);
        }
    }

    #[test]
    fn closed_prims_have_no_boundary_edges() {
        for (name, shape) in [
            ("box", PrimShape::default()),
            ("cylinder", PrimShape { profile_curve: 0, ..PrimShape::default() }),
            ("sphere", PrimShape { path_curve: 0x20, profile_curve: 5, ..PrimShape::default() }),
            ("torus", PrimShape { path_curve: 0x20, profile_curve: 0, path_scale_x: 150, path_scale_y: 150, ..PrimShape::default() }),
        ] {
            assert_closed(&generate(shape), name);
        }
    }

    #[test]
    fn collapsed_path_keeps_face() {
        let mesh = generate(PrimShape { path_curve: 0x10, path_scale_x: 200, path_scale_y: 200, ..PrimShape::default() });

        assert!(mesh.faces()[0].indices.is_empty());
        assert!(mesh.faces().iter().any(|face| !face.indices.is_empty()));
    }

    #[test]
    fn flexible_shape_and_uv_seams() {
        let line = generate(PrimShape { profile_curve: 0, ..PrimShape::default() });
        let flexible = generate(PrimShape { path_curve: 0x80, profile_curve: 0, ..PrimShape::default() });
        assert_eq!(line.vertices(), flexible.vertices());
        assert_eq!(line.indices(), flexible.indices());
        let a = line.vertices().iter().find(|v| v.uv == Vec2::ZERO).unwrap();
        let b = line.vertices().iter().find(|v| v.uv == Vec2::X).unwrap();
        close(a.position, b.position);
        close(a.normal, b.normal);
        assert_ne!(a.uv, b.uv);
    }

    #[test]
    fn circular_path_constraints() {
        let shape = PrimShape {
            path_curve: 0x20,
            path_scale_x: 100,
            path_scale_y: 150,
            path_revolutions: 100,
            path_radius_offset: 80,
            ..PrimShape::default()
        };
        let mesh = generate(shape);
        let centers: Vec<_> = mesh
            .faces()
            .iter()
            .filter_map(|face| {
                let vertices = &mesh.indices()[face.indices.start as usize..face.indices.end as usize];
                vertices.iter().map(|i| mesh.vertices()[*i as usize]).find(|v| v.uv == Vec2::splat(0.5)).map(|v| v.position)
            })
            .collect();
        assert!(centers.iter().any(|p| p.distance(Vec3::new(-5.0 / 14.0, 0.25, 0.0)) < 2e-5));
        assert!(centers.iter().any(|p| p.distance(Vec3::new(5.0 / 14.0, -0.25, 0.0)) < 2e-5));
        let clamped = generate(PrimShape { path_radius_offset: 0, ..shape });
        assert_eq!(mesh.vertices(), clamped.vertices());
        assert_eq!(mesh.indices(), clamped.indices());

        let tapered = generate(PrimShape {
            path_curve: 0x20,
            path_scale_x: 150,
            path_scale_y: 150,
            path_taper_y: 50,
            path_radius_offset: 80,
            ..PrimShape::default()
        });
        let cap = &tapered.faces()[0];
        assert!(tapered.indices()[cap.indices.start as usize..cap.indices.end as usize].iter().any(|i| {
            let v = tapered.vertices()[*i as usize];
            v.uv == Vec2::splat(0.5) && v.position.distance(Vec3::new(0.0, 0.125, 0.0)) < 2e-5
        }));
    }

    #[test]
    fn generation_errors() {
        let cancel = AtomicBool::new(false);
        for (shape, expected) in [
            (PrimShape { profile_curve: 15, ..PrimShape::default() }, MeshBuildError::UnsupportedShape),
            (PrimShape { path_curve: 0x11, ..PrimShape::default() }, MeshBuildError::UnsupportedShape),
            (PrimShape { profile_begin: 50000, ..PrimShape::default() }, MeshBuildError::InvalidParameters),
        ] {
            assert_matches!(tessellate(shape, Detail::Low, &cancel).err(), Some(error) if error == expected, "shape {shape:?}");
        }
        assert_matches!(tessellate(PrimShape::default(), Detail::Low, &AtomicBool::new(true)).err(), Some(MeshBuildError::Cancelled));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]
        #[test]
        fn valid_prim_lods_stay_bounded(
            profile in 0u8..6,
            hole in 0u8..4,
            path in prop::sample::select(vec![0x10u8, 0x20, 0x30, 0x40, 0x80]),
            cuts in prop::array::uniform4(0u16..20000),
            hollow in 0u16..=30000,
            scales in prop::array::uniform2(25u8..176),
            shear in prop::array::uniform2(-50i8..=50),
            twist in prop::array::uniform2(-100i8..=100),
            taper in prop::array::uniform2(-80i8..=80),
            radius in -80i8..=80,
            revolutions in 0u8..=200,
            skew in -95i8..=95,
        ) {
            let shape = PrimShape {
                profile_curve: profile | (hole << 4), path_curve: path,
                profile_begin: cuts[0], profile_end: cuts[1], path_begin: cuts[2], path_end: cuts[3], profile_hollow: hollow,
                path_scale_x: if path == 0x20 && profile != 5 { 100 + scales[0] % 76 } else { scales[0] },
                path_scale_y: if path == 0x20 && profile != 5 { 150 + scales[1] % 26 } else { scales[1] },
                path_shear_x: shear[0], path_shear_y: shear[1], path_twist_begin: twist[0], path_twist: twist[1],
                path_taper_x: taper[0], path_taper_y: taper[1], path_radius_offset: radius, path_revolutions: revolutions, path_skew: skew,
            };
            for detail in [Detail::Lowest, Detail::Low, Detail::Medium, Detail::High] {
                let mesh = tessellate(shape, detail, &AtomicBool::new(false))
                    .map_err(|error| prop::test_runner::TestCaseError::fail(format!("valid shape {shape:?} failed at {detail:?}: {error:?}")))?;
                assert_lod_invariants(&mesh, detail)?;
            }
        }
    }
}
