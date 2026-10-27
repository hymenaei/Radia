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
pub(super) struct Builder {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    faces: Vec<Face>,
    parents: Vec<usize>,
    normal_overrides: Vec<(usize, Vec3)>,
}

impl Builder {
    pub(super) fn vertex(&mut self, position: Vec3, uv: Vec2) -> Result<usize, MeshBuildError> {
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

    pub(super) fn triangle(&mut self, a: usize, b: usize, c: usize) -> Result<(), MeshBuildError> {
        if self.indices.len() + 3 > MAX_MESH_INDICES {
            return Err(MeshBuildError::TooComplex);
        }
        self.indices.extend([a as u32, b as u32, c as u32]);
        Ok(())
    }

    pub(super) fn begin_face(&self) -> usize {
        self.indices.len()
    }

    pub(super) fn end_face(&mut self, start: usize) -> Result<(), MeshBuildError> {
        if self.faces.len() == MAX_PRIM_FACES {
            return Err(MeshBuildError::TooComplex);
        }
        self.faces.push(Face { slot: self.faces.len() as u8, indices: start as u32..self.indices.len() as u32 });
        Ok(())
    }

    fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    pub(super) fn seam(&mut self, a: usize, b: usize) {
        let a = root(&mut self.parents, a);
        let b = root(&mut self.parents, b);
        self.parents[b] = a;
    }

    fn normal_override(&mut self, index: usize, normal: Vec3) {
        self.normal_overrides.push((index, normal));
    }

    pub(super) fn finish(mut self, cancelled: &AtomicBool) -> Result<Mesh, MeshBuildError> {
        if self.indices.is_empty() {
            return Err(MeshBuildError::Degenerate);
        }
        let mut normals = vec![Vec3::ZERO; self.vertices.len()];
        for face in &self.faces {
            let indices = &self.indices[face.indices.start as usize..face.indices.end as usize];
            for (ordinal, triangle) in indices.as_chunks::<3>().0.iter().enumerate() {
                if ordinal % 256 == 0 && cancelled.load(Ordering::Acquire) {
                    return Err(MeshBuildError::Cancelled);
                }
                let [a, b, c] = [triangle[0] as usize, triangle[1] as usize, triangle[2] as usize];
                let normal =
                    (self.vertices[b].position - self.vertices[a].position).cross(self.vertices[c].position - self.vertices[a].position);
                for index in [a, b, c] {
                    normals[root(&mut self.parents, index)] += normal;
                }
                let extra = if ordinal % 2 == 0 { b } else { c };
                normals[root(&mut self.parents, extra)] += normal;
            }
        }
        for (index, vertex) in self.vertices.iter_mut().enumerate() {
            vertex.normal = normals[root(&mut self.parents, index)].try_normalize().unwrap_or(Vec3::Z);
        }
        for (index, normal) in self.normal_overrides {
            self.vertices[index].normal = normal;
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
    cap_begin: Vec<Point>,
    cap_end: Vec<Point>,
    sides: Option<usize>,
    inner_flat: bool,
    open: bool,
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
        let mut s = [f32::from(shape.profile_begin) / 50000.0, 1.0 - f32::from(shape.profile_end) / 50000.0];
        let mut t = [f32::from(shape.path_begin) / 50000.0, 1.0 - f32::from(shape.path_end) / 50000.0];
        if s[1] >= 0.0149 && s[1] < 0.02 {
            s[1] = 0.02;
        }
        for cut in [&mut s, &mut t] {
            if cut[0] < 0.0 || cut[0] > 0.98 || cut[1] < 0.02 || cut[1] > 1.0 {
                return Err(MeshBuildError::InvalidParameters);
            }
            if cut[0] > cut[1] - 0.02 {
                if cut[0] - (cut[1] - 0.02) > 0.01 {
                    return Err(MeshBuildError::InvalidParameters);
                }
                cut[0] = cut[1] - 0.02;
            }
        }
        let hollow = f32::from(shape.profile_hollow) / 50000.0;
        let scale = Vec2::new(2.0 - f32::from(shape.path_scale_x) / 100.0, 2.0 - f32::from(shape.path_scale_y) / 100.0);
        let shear = Vec2::new(f32::from(shape.path_shear_x), f32::from(shape.path_shear_y)) / 100.0;
        let twist = [f32::from(shape.path_twist_begin) / 100.0, f32::from(shape.path_twist) / 100.0];
        let taper = Vec2::new(f32::from(shape.path_taper_x), f32::from(shape.path_taper_y)) / 100.0;
        let revolutions = 1.0 + f32::from(shape.path_revolutions) * 0.015;
        let mut radius = f32::from(shape.path_radius_offset) / 100.0;
        let mut skew = f32::from(shape.path_skew) / 100.0;
        let max_hollow = if hole == 2 && [0, 3, 5].contains(&profile) { 0.7 } else { 0.95 };
        if hollow > max_hollow
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
    let uncut_cube =
        params.profile == 1 && params.hole == 0 && params.hollow == 0.0 && !profile.open && params.path == 0x10 && params.t == [0.0, 1.0];
    let pole_normals = params.path == 0x20 && params.profile == 5;
    let mut mesh = Builder::default();
    if !closed {
        if uncut_cube {
            cube_cap(&mut mesh, &profile, path.last().expect("nonempty bounded path"), true)?;
        } else {
            cap(&mut mesh, &profile, &profile.cap_begin, path.last().expect("nonempty bounded path"), true)?;
        }
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
            let points = &profile.outer[begin..=end];
            side_grid(
                &mut mesh,
                points,
                &path,
                sides as f32,
                (points[0].t * sides as f32).floor(),
                false,
                false,
                closed,
                pole_normals,
                cancelled,
            )?;
            mesh.end_face(start)?;
            begin = end;
        }
    } else {
        let start = mesh.begin_face();
        side_grid(&mut mesh, &profile.outer, &path, 1.0, 0.0, false, !profile.open, closed, pole_normals, cancelled)?;
        mesh.end_face(start)?;
    }
    if !profile.inner.is_empty() {
        let points: Vec<_> = profile.inner.iter().rev().copied().collect();
        let start = mesh.begin_face();
        let uv_origin = if profile.inner_flat { points[0].t.floor() } else { 0.0 };
        if profile.inner_flat {
            let mut flat_points = vec![points[0]];
            for point in points.iter().skip(1) {
                flat_points.extend([*point, *point]);
            }
            flat_points.push(if profile.open { *points.last().expect("inner profile endpoints") } else { points[0] });
            side_grid(&mut mesh, &flat_points, &path, 1.0, uv_origin, false, !profile.open, closed, pole_normals, cancelled)?;
        } else {
            side_grid(&mut mesh, &points, &path, 1.0, uv_origin, false, !profile.open, closed, pole_normals, cancelled)?;
        }
        mesh.end_face(start)?;
    }
    if !closed {
        if uncut_cube {
            cube_cap(&mut mesh, &profile, &path[0], false)?;
        } else {
            cap(&mut mesh, &profile, &profile.cap_end, &path[0], false)?;
        }
    }
    if profile.open {
        let outer_begin = profile.outer[0];
        let outer_end = *profile.outer.last().expect("profile endpoints");
        let center = Point { position: Vec2::ZERO, t: 0.0 };
        let inner_begin = profile.inner.first().copied().unwrap_or(center);
        let inner_end = profile.inner.last().copied().unwrap_or(center);
        for edge in [[inner_begin, outer_begin], [outer_end, inner_end]] {
            let start = mesh.begin_face();
            side_grid(&mut mesh, &edge, &path, 1.0, 0.0, true, false, closed, pole_normals, cancelled)?;
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
    let first_side = (cut[0] * sides as f32).floor() as usize;
    let first_t = first_side as f32 / sides as f32;
    let mut points = Vec::new();
    if (cut[0] - first_t) * (sides as f32) < 0.9999 {
        points.push(point(cut[0]));
    }
    let mut sample = first_side + 1;
    let mut t = sample as f32 / sides as f32;
    while t < cut[1] {
        if let Some(previous) = points.last().map(|point| point.t) {
            for i in 1..=split {
                points.push(point(previous + (t - previous) * i as f32 / (split + 1) as f32));
            }
        }
        points.push(point(t));
        sample += 1;
        t = sample as f32 / sides as f32;
    }
    let end_fraction = (cut[1] - (t - 1.0 / sides as f32)) * sides as f32;
    if end_fraction > 0.0001 {
        if let Some(previous) = points.last().map(|point| point.t) {
            for i in 1..=split {
                points.push(point(previous + (cut[1] - previous) * i as f32 / (split + 1) as f32));
            }
        }
        points.push(point(cut[1]));
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
    if params.path == 0x10 && params.scale != Vec2::ONE && sides.is_some() {
        split = 0;
    }
    let half = params.profile == 5;
    let circle_detail = 6.0 * detail.scale() * if half { 0.5 } else { 1.0 };
    let circle_detail = if params.hollow > 0.0 && params.hole == 2 && sides.is_none() {
        let divisor = if half { 2.0 } else { 4.0 };
        (circle_detail / divisor).ceil() * divisor
    } else {
        circle_detail
    };
    let count = sides.unwrap_or(circle_detail.floor() as usize);
    let offset = if half {
        0.5
    } else if params.profile == 1 {
        -0.375
    } else {
        0.0
    };
    let span = if half { 0.5 } else { 1.0 };
    let outer = polygon(count, offset, span, params.s, if sides.is_some() { split } else { 0 }, 1.0);
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
    let mut cap_begin = outer.clone();
    let mut cap_end = outer.clone();
    if params.hollow == 0.0 && open {
        let center = Point { position: Vec2::ZERO, t: 0.0 };
        cap_begin.push(center);
        cap_end.push(center);
    } else if half && params.hollow == 0.0 && params.s == [0.0, 1.0] {
        let center = Point { position: Vec2::ZERO, t: 0.0 };
        cap_begin.push(center);
        cap_end.push(center);
        cap_end.push(outer[0]);
    }
    Profile { outer, inner, cap_begin, cap_end, sides, inner_flat, open }
}

fn path(params: &Params, detail: Detail) -> (Vec<Ring>, bool) {
    let twist = (params.twist[1] - params.twist[0]).abs();
    if [0x10, 0x80].contains(&params.path) {
        let split = if params.path == 0x10 && params.scale != Vec2::ONE && (1..=4).contains(&params.profile) {
            0
        } else {
            (detail.scale() * 0.66).floor() as usize
        };
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
    let closed = if params.path == 0x30 {
        params.t[1] - params.t[0] >= 0.99 && params.scale.x >= 0.99 && params.twist[0] == params.twist[1]
    } else {
        params.t[1] - params.t[0] >= 1.0
            && params.skew.abs() <= 0.001
            && (end - begin).abs().max_element() <= 0.001
            && (radii[1] - radii[0]).abs() <= 0.001
            && params.twist[0] == params.twist[1]
    };
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
    uv_origin: f32,
    cut: bool,
    wrap_s: bool,
    wrap_t: bool,
    pole_normals: bool,
    cancelled: &AtomicBool,
) -> Result<(), MeshBuildError> {
    let base = mesh.vertex_count();
    let width = points.len();
    for ring in path {
        check(cancelled)?;
        for (i, point) in points.iter().enumerate() {
            let s = if cut { i as f32 } else { point.t * uv_scale - uv_origin };
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
    let bottom_converges = path[0].point(points[0]).distance_squared(path[path.len() - 2].point(points[0])) < 1e-6;
    let top_converges = path[0]
        .point(*points.last().expect("profile endpoints"))
        .distance_squared(path[path.len() - 2].point(*points.last().expect("profile endpoints")))
        < 1e-6;
    if wrap_s && !bottom_converges {
        for row in 0..path.len() {
            mesh.seam(base + row * width, base + row * width + width - 1);
        }
    }
    if wrap_t {
        for column in 0..width {
            mesh.seam(base + column, base + (path.len() - 1) * width + column);
        }
    }
    if pole_normals {
        if bottom_converges {
            for row in 0..path.len() {
                mesh.normal_override(base + row * width, Vec3::X);
            }
        }
        if top_converges {
            for row in 0..path.len() {
                mesh.normal_override(base + row * width + width - 1, -Vec3::X);
            }
        }
    }
    Ok(())
}

fn cap(mesh: &mut Builder, profile: &Profile, cap_profile: &[Point], ring: &Ring, top: bool) -> Result<(), MeshBuildError> {
    let start = mesh.begin_face();
    let mut points = cap_profile.to_vec();
    points.extend(profile.inner.iter().rev().copied());
    let base = mesh.vertex_count();
    let mut min_position = Vec3::splat(f32::INFINITY);
    let mut max_position = Vec3::splat(f32::NEG_INFINITY);
    let mut min_uv = Vec2::splat(f32::INFINITY);
    let mut max_uv = Vec2::splat(f32::NEG_INFINITY);
    for point in &points {
        let position = ring.point(*point);
        let uv = Vec2::new(point.position.x + 0.5, if top { point.position.y + 0.5 } else { 0.5 - point.position.y });
        min_position = min_position.min(position);
        max_position = max_position.max(position);
        min_uv = min_uv.min(uv);
        max_uv = max_uv.max(uv);
        mesh.vertex(position, uv)?;
    }
    if profile.inner.is_empty() && !profile.open {
        mesh.vertex((min_position + max_position) * 0.5, (min_uv + max_uv) * 0.5)?;
    }
    if profile.inner.is_empty() {
        if let Some(pivot) = mesh.vertex_count().checked_sub(1).filter(|pivot| *pivot >= base) {
            let triangle_count = pivot.saturating_sub(base).saturating_sub(1);
            for index in 0..triangle_count {
                let a = base + index;
                let b = a + 1;
                if top {
                    mesh.triangle(pivot, a, b)?;
                } else {
                    mesh.triangle(pivot, b, a)?;
                }
            }
        }
    } else {
        let mut first = 0;
        let mut last = points.len() - 1;
        while last - first > 1 {
            let p1 = points[first].position;
            let p2 = points[last].position;
            let pa = points[first + 1].position;
            let pb = points[last - 1].position;
            let area_1a2 = (pa - p1).perp_dot(p2 - p1);
            let area_1ba = (pb - p1).perp_dot(pa - p1);
            let area_21b = (p1 - p2).perp_dot(pb - p2);
            let area_2ab = (pa - p2).perp_dot(pb - p2);
            let first_valid = area_1a2 >= 0.0 && area_2ab >= 0.0;
            let last_valid = area_21b >= 0.0 && area_1ba >= 0.0;
            let use_first = if !first_valid {
                false
            } else if !last_valid {
                true
            } else {
                (p1 - pa).length_squared() < (p2 - pb).length_squared()
            };
            if use_first {
                if top {
                    mesh.triangle(base + first, base + first + 1, base + last)?;
                } else {
                    mesh.triangle(base + first, base + last, base + first + 1)?;
                }
                first += 1;
            } else {
                if top {
                    mesh.triangle(base + first, base + last - 1, base + last)?;
                } else {
                    mesh.triangle(base + first, base + last, base + last - 1)?;
                }
                last -= 1;
            }
        }
    }
    let normal = mesh
        .indices
        .get(start..)
        .and_then(|indices| {
            let triangle = indices.get(..3)?;
            let a = mesh.vertices.get(*triangle.first()? as usize)?.position;
            let b = mesh.vertices.get(*triangle.get(1)? as usize)?.position;
            let c = mesh.vertices.get(*triangle.get(2)? as usize)?.position;
            let cross = (b - a).cross(c - a);
            Some(if cross.length_squared() > 0.00001 {
                cross / cross.length()
            } else if cross.z >= 0.0 {
                Vec3::Z
            } else {
                -Vec3::Z
            })
        })
        .unwrap_or(if top { Vec3::Z } else { -Vec3::Z });
    for index in base..mesh.vertex_count() {
        mesh.normal_override(index, normal);
    }
    mesh.end_face(start)
}

fn cube_cap(mesh: &mut Builder, profile: &Profile, ring: &Ring, top: bool) -> Result<(), MeshBuildError> {
    let start = mesh.begin_face();
    let grid = (profile.outer.len() - 1) / 4;
    let mut corners = [(Vec3::ZERO, Vec2::ZERO); 4];
    for (corner, value) in corners.iter_mut().enumerate() {
        let point = profile.outer[grid * corner];
        value.0 = ring.point(point);
        value.1 = Vec2::new(point.position.x + 0.5, 0.5 - point.position.y);
    }
    let normal =
        (corners[1].0 - corners[0].0).cross(corners[2].0 - corners[1].0).try_normalize().unwrap_or(Vec3::Z) * if top { 1.0 } else { -1.0 };
    if top {
        let uv = corners[0].1;
        corners[0].1 = corners[3].1;
        corners[3].1 = uv;
        let uv = corners[1].1;
        corners[1].1 = corners[2].1;
        corners[2].1 = uv;
    }
    let base = mesh.vertex_count();
    let row_size = grid + 1;
    for gx in 0..=grid {
        for gy in 0..=grid {
            let u = gx as f32 / grid as f32;
            let v = gy as f32 / grid as f32;
            let position = corners[0].0 + (corners[1].0 - corners[0].0) * u + (corners[3].0 - corners[0].0) * v;
            let uv = corners[0].1 + (corners[1].1 - corners[0].1) * u + (corners[3].1 - corners[0].1) * v;
            let index = mesh.vertex(position, uv)?;
            mesh.normal_override(index, normal);
        }
    }
    for gy in 0..grid {
        for gx in 0..grid {
            let base = base + gy * row_size + gx;
            if top {
                mesh.triangle(base, base + row_size, base + row_size + 1)?;
                mesh.triangle(base + row_size + 1, base + 1, base)?;
            } else {
                mesh.triangle(base, base + 1, base + row_size + 1)?;
                mesh.triangle(base + row_size + 1, base + row_size, base)?;
            }
        }
    }
    mesh.end_face(start)
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::{
        collections::HashMap,
        f32::consts::{PI, TAU},
        sync::atomic::AtomicBool,
    };

    use assets::mesh::{MAX_MESH_INDICES, MAX_MESH_VERTICES, Mesh};
    use math::{Quat, Vec2, Vec3};
    use proptest::prelude::*;
    use world::PrimShape;

    use super::{Builder, Detail, MeshBuildError, Point, Profile, Ring, cap, tessellate};

    fn generate_at(shape: PrimShape, detail: Detail) -> Mesh {
        tessellate(shape, detail, &AtomicBool::new(false)).unwrap()
    }

    fn generate(shape: PrimShape) -> Mesh {
        generate_at(shape, Detail::High)
    }

    fn vertex_at(mesh: &Mesh, slot: usize, uv: Vec2) -> assets::mesh::Vertex {
        let face = &mesh.faces()[slot];
        mesh.indices()[face.indices.start as usize..face.indices.end as usize]
            .iter()
            .map(|index| mesh.vertices()[*index as usize])
            .find(|vertex| vertex.uv.distance(uv) < 1e-6)
            .unwrap_or_else(|| panic!("missing UV {uv:?} in face slot {slot}"))
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
            prop_assert!((vertex.normal.length() - 1.0).abs() < 1e-5, "normal is unit length");
            prop_assert!(vertex.position.length() <= 3.0, "normalized bound used by camera planning");
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
        let first_side_index = mesh.indices()[mesh.faces()[1].indices.start as usize] as usize;
        close(mesh.vertices()[first_side_index].normal, -Vec3::Y);
    }

    #[test]
    fn caps_without_triangles_keep_finite_normals() {
        let point = Point { position: Vec2::ZERO, t: 0.0 };
        let ring = Ring { position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec2::ONE, t: 0.0 };
        let make_cap = |inner: Vec<Point>, open: bool, top: bool| {
            let mut mesh = Builder::default();
            let profile =
                Profile { outer: Vec::new(), inner, cap_begin: Vec::new(), cap_end: Vec::new(), sides: None, inner_flat: false, open };
            cap(&mut mesh, &profile, &[point], &ring, top).unwrap();
            mesh
        };

        let open = make_cap(Vec::new(), true, true);
        assert!(open.faces[0].indices.is_empty());
        assert_eq!(open.normal_overrides, [(0, Vec3::Z)]);

        let hollow = make_cap(vec![point], true, false);
        assert!(hollow.faces[0].indices.is_empty());
        assert_eq!(hollow.normal_overrides, [(0, -Vec3::Z), (1, -Vec3::Z)]);
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
    fn primitive_lods_match_reference_counts() {
        let details = [Detail::Lowest, Detail::Low, Detail::Medium, Detail::High];
        let cases = [
            ("box", PrimShape::default(), [(24, 36), (24, 36), (54, 144), (96, 324)], 6),
            ("cylinder", PrimShape { profile_curve: 0, ..PrimShape::default() }, [(30, 72), (42, 108), (82, 270), (152, 576)], 3),
            (
                "sphere",
                PrimShape { path_curve: 0x20, profile_curve: 5, ..PrimShape::default() },
                [(28, 108), (50, 216), (128, 630), (325, 1728)],
                1,
            ),
            (
                "torus",
                PrimShape { path_curve: 0x20, profile_curve: 0, path_scale_x: 150, path_scale_y: 150, ..PrimShape::default() },
                [(49, 216), (100, 486), (256, 1350), (625, 3456)],
                1,
            ),
        ];
        for (name, shape, expected, faces) in cases {
            for (detail, (vertices, indices)) in details.into_iter().zip(expected) {
                let mesh = generate_at(shape, detail);
                assert_eq!(mesh.vertices().len(), vertices, "{name} {detail:?} vertices");
                assert_eq!(mesh.indices().len(), indices, "{name} {detail:?} indices");
                assert_eq!(mesh.faces().len(), faces, "{name} {detail:?} faces");
            }
        }
    }

    #[test]
    fn sphere_poles_follow_axis() {
        let mesh = generate(PrimShape { path_curve: 0x20, profile_curve: 5, ..PrimShape::default() });
        let face = &mesh.faces()[0];
        let indices = &mesh.indices()[face.indices.start as usize..face.indices.end as usize];
        let first =
            indices.iter().map(|index| mesh.vertices()[*index as usize]).find(|vertex| vertex.uv == Vec2::ZERO).expect("first sphere pole");
        let last = indices
            .iter()
            .map(|index| mesh.vertices()[*index as usize])
            .find(|vertex| vertex.uv == Vec2::new(1.0, 0.0))
            .expect("opposite sphere pole");
        close(first.position, Vec3::X * 0.5);
        close(first.normal, Vec3::X);
        close(last.position, -Vec3::X * 0.5);
        close(last.normal, -Vec3::X);
        for vertex in mesh.vertices().iter().filter(|vertex| vertex.position.distance(Vec3::X * 0.5) < 1e-6) {
            close(vertex.normal, Vec3::X);
        }
        for vertex in mesh.vertices().iter().filter(|vertex| vertex.position.distance(-Vec3::X * 0.5) < 1e-6) {
            close(vertex.normal, -Vec3::X);
        }
    }

    #[test]
    fn face_slots_and_uvs_follow_reference() {
        let shapes = [
            PrimShape { profile_curve: 0, ..PrimShape::default() },
            PrimShape { path_curve: 0x20, profile_curve: 5, ..PrimShape::default() },
            PrimShape { path_curve: 0x20, profile_curve: 0, path_scale_x: 150, path_scale_y: 150, ..PrimShape::default() },
            PrimShape { path_curve: 0x30, profile_curve: 0, ..PrimShape::default() },
            PrimShape { path_twist: 50, ..PrimShape::default() },
        ];
        for shape in shapes {
            let reference = generate_at(shape, Detail::Lowest);
            let reference_slots: Vec<_> = reference.faces().iter().map(|face| face.slot).collect();
            for detail in [Detail::Low, Detail::Medium, Detail::High] {
                let mesh = generate_at(shape, detail);
                assert_eq!(mesh.faces().iter().map(|face| face.slot).collect::<Vec<_>>(), reference_slots);
                for (face, reference_face) in mesh.faces().iter().zip(reference.faces()) {
                    for uv in [Vec2::ZERO, Vec2::X, Vec2::Y, Vec2::ONE] {
                        let reference_has_uv = reference.indices()
                            [reference_face.indices.start as usize..reference_face.indices.end as usize]
                            .iter()
                            .map(|index| reference.vertices()[*index as usize].uv)
                            .any(|actual| actual.distance(uv) < 1e-6);
                        if reference_has_uv {
                            let actual = mesh.indices()[face.indices.start as usize..face.indices.end as usize]
                                .iter()
                                .map(|index| mesh.vertices()[*index as usize].uv)
                                .find(|actual| actual.distance(uv) < 1e-6)
                                .unwrap_or_else(|| panic!("shared parametric UV {uv:?} missing at {detail:?}"));
                            assert!(actual.distance(uv) < 1e-6);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn flat_profile_faces_keep_hard_normals() {
        let mesh = generate(PrimShape { path_twist: 50, ..PrimShape::default() });
        let first = vertex_at(&mesh, 1, Vec2::new(1.0, 0.0));
        let next = vertex_at(&mesh, 2, Vec2::ZERO);
        close(first.position, next.position);
        assert!(first.normal.distance(next.normal) > 0.2);
    }

    #[test]
    fn cut_vertices_match_reference_samples() {
        let shape = PrimShape { profile_curve: 1, profile_begin: 6250, profile_end: 6250, ..PrimShape::default() };
        let expected = [
            Vec3::new(0.0, -0.5, -0.5),
            Vec3::new(0.5, -0.5, -0.5),
            Vec3::new(0.5, 0.5, -0.5),
            Vec3::new(-0.5, 0.5, -0.5),
            Vec3::new(-0.5, 0.0, -0.5),
        ];
        for detail in [Detail::Lowest, Detail::Low, Detail::Medium, Detail::High] {
            let mesh = generate_at(shape, detail);
            for position in expected {
                assert!(
                    mesh.vertices().iter().any(|vertex| vertex.position.distance(position) < 2e-5),
                    "missing {position:?} at {detail:?}"
                );
            }
        }
        let curved = PrimShape { profile_curve: 0, profile_begin: 6250, profile_end: 6250, ..PrimShape::default() };
        for (detail, sides, radius) in
            [(Detail::Lowest, 6, 0.525), (Detail::Low, 9, 0.5), (Detail::Medium, 15, 0.5), (Detail::High, 24, 0.5)]
        {
            let mesh = generate_at(curved, detail);
            for t in [0.125, 0.875] {
                let vertex = mesh.vertices().iter().find(|vertex| vertex.uv.distance(Vec2::new(t, 0.0)) < 1e-6).expect("cut endpoint UV");
                let base = (t * sides as f32).floor();
                let fraction = t * sides as f32 - base;
                let start = TAU * (base / sides as f32);
                let end = start + TAU / sides as f32;
                let a = Vec2::new(start.cos(), start.sin()) * radius;
                let b = Vec2::new(end.cos(), end.sin()) * radius;
                close(vertex.position, a.lerp(b, fraction).extend(-0.5));
            }
        }
    }

    #[test]
    fn triangle_cut_faces_keep_uv_ranges() {
        let mesh = generate(PrimShape { profile_curve: 2, profile_begin: 12500, profile_end: 12500, ..PrimShape::default() });
        assert_eq!(mesh.faces().len(), 7);
        for (slot, expected) in [(1, [0.75, 1.0]), (2, [0.0, 1.0]), (3, [0.0, 0.25])] {
            let face = &mesh.faces()[slot];
            let indices = &mesh.indices()[face.indices.start as usize..face.indices.end as usize];
            let min_u = indices.iter().map(|index| mesh.vertices()[*index as usize].uv.x).fold(f32::INFINITY, f32::min);
            let max_u = indices.iter().map(|index| mesh.vertices()[*index as usize].uv.x).fold(f32::NEG_INFINITY, f32::max);
            let min_v = indices.iter().map(|index| mesh.vertices()[*index as usize].uv.y).fold(f32::INFINITY, f32::min);
            let max_v = indices.iter().map(|index| mesh.vertices()[*index as usize].uv.y).fold(f32::NEG_INFINITY, f32::max);
            assert!((min_u - expected[0]).abs() < 1e-5, "triangle face {slot} starts at {min_u}");
            assert!((max_u - expected[1]).abs() < 1e-5, "triangle face {slot} ends at {max_u}");
            assert!(min_v.abs() < 1e-5 && (max_v - 1.0).abs() < 1e-5, "triangle face {slot} path UV range is {min_v}..{max_v}");
        }
    }

    #[test]
    fn mixed_hollows_match_areas() {
        let cases = [
            (
                PrimShape { profile_curve: 1 | (1 << 4), profile_hollow: 12500, ..PrimShape::default() },
                1.0 - 24.0 * 0.125_f32.powi(2) * 0.5 * (PI / 12.0).sin(),
            ),
            (
                PrimShape { profile_curve: 2 << 4, profile_hollow: 12500, ..PrimShape::default() },
                24.0 * 0.5_f32.powi(2) * 0.5 * (PI / 12.0).sin() - 0.0625,
            ),
        ];
        for (shape, expected) in cases {
            let mesh = generate(shape);
            assert!((volume(&mesh) - expected).abs() < 2e-5, "{shape:?}: {} != {expected}", volume(&mesh));
        }
    }

    #[test]
    fn flat_hollow_faces_match_reference_counts() {
        let shape = PrimShape { profile_hollow: 25000, ..PrimShape::default() };
        for (detail, vertices, indices) in [(Detail::Lowest, 56, 126), (Detail::High, 220, 810)] {
            let mesh = generate_at(shape, detail);
            assert_eq!(mesh.vertices().len(), vertices, "{detail:?} vertices");
            assert_eq!(mesh.indices().len(), indices, "{detail:?} indices");
            assert_eq!(mesh.faces().len(), 7, "{detail:?} faces");
        }
    }

    #[test]
    fn quantized_limits_keep_valid_cuts() {
        let shape =
            PrimShape { profile_curve: 2 << 4, profile_begin: 49000, path_begin: 49000, profile_hollow: 35000, ..PrimShape::default() };
        let mesh = generate(shape);
        let indices = &mesh.indices()[mesh.faces()[1].indices.start as usize..mesh.faces()[1].indices.end as usize];
        let min_u = indices.iter().map(|index| mesh.vertices()[*index as usize].uv.x).fold(f32::INFINITY, f32::min);
        let max_u = indices.iter().map(|index| mesh.vertices()[*index as usize].uv.x).fold(f32::NEG_INFINITY, f32::max);
        let min_v = indices.iter().map(|index| mesh.vertices()[*index as usize].uv.y).fold(f32::INFINITY, f32::min);
        let max_v = indices.iter().map(|index| mesh.vertices()[*index as usize].uv.y).fold(f32::NEG_INFINITY, f32::max);
        assert!((min_u - 0.98).abs() < 1e-5 && (max_u - 1.0).abs() < 1e-5);
        assert!((min_v - 0.98).abs() < 1e-5 && (max_v - 1.0).abs() < 1e-5);
        let outside_limit = PrimShape { profile_begin: 49001, ..shape };
        assert_matches!(tessellate(outside_limit, Detail::High, &AtomicBool::new(false)).err(), Some(MeshBuildError::InvalidParameters));
    }

    #[test]
    fn combined_path_modifiers_compose() {
        let mesh = generate(PrimShape {
            path_curve: 0x20,
            profile_curve: 0,
            profile_begin: 12500,
            profile_end: 12500,
            path_begin: 12500,
            path_end: 12500,
            path_scale_x: 150,
            path_scale_y: 150,
            path_shear_x: 20,
            path_shear_y: -10,
            path_taper_x: 20,
            path_taper_y: -15,
            path_twist: 50,
            ..PrimShape::default()
        });
        let vertex = mesh.vertices().iter().find(|vertex| vertex.uv == Vec2::splat(0.25)).expect("first cut ring sample");
        let radial = 0.221875 / 2.0_f32.sqrt();
        close(vertex.position, Vec3::new(0.05 + radial, -0.025, 0.25 - radial));
    }

    #[test]
    fn circle2_path_matches_samples() {
        let mesh = generate_at(PrimShape { path_curve: 0x30, profile_curve: 0, ..PrimShape::default() }, Detail::Lowest);
        assert_eq!(mesh.faces().len(), 1);
        assert_eq!(mesh.vertices().len(), 49);
        assert_eq!(mesh.indices().len(), 216);
        let first = mesh.vertices().iter().find(|vertex| vertex.uv == Vec2::ZERO).expect("first path sample");
        let next = mesh.vertices().iter().find(|vertex| vertex.uv == Vec2::new(0.0, 1.0 / 6.0)).expect("second path sample");
        close(first.position, Vec3::new(-0.025, 0.0, 0.0));
        close(next.position, Vec3::new(-1.025, 0.0, 0.0));
    }

    #[test]
    fn test_curve_matches_samples() {
        let mesh = generate_at(
            PrimShape { path_curve: 0x40, profile_curve: 0, path_scale_x: 150, path_scale_y: 125, path_twist: 50, ..PrimShape::default() },
            Detail::Lowest,
        );
        assert_eq!(mesh.faces().len(), 3);
        let vertex = mesh.vertices().iter().find(|vertex| vertex.uv == Vec2::new(0.0, 0.25)).expect("quarter path sample");
        close(vertex.position, Vec3::new(0.459375, -(PI / 8.0).sin() / 8.0, -0.375 + (PI / 8.0).cos() / 8.0));
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

        assert!(!mesh.faces()[0].indices.is_empty());
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
                vertices.iter().map(|i| mesh.vertices()[*i as usize]).find(|v| v.uv.distance(Vec2::splat(0.5)) < 1e-6).map(|v| v.position)
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
            v.uv.distance(Vec2::splat(0.5)) < 1e-6 && v.position.distance(Vec3::new(0.0, 0.125, 0.0)) < 2e-5
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
