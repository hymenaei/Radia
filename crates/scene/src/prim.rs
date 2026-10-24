use std::{
    f32::consts::{PI, TAU},
    sync::atomic::{AtomicBool, Ordering},
};

use math::{FloatExt, Quat, Vec2, Vec3};
use world::PrimShape;

use crate::{Detail, Mesh, MeshError, mesh::Builder};

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
    fn new(shape: PrimShape) -> Result<Self, MeshError> {
        let profile = shape.profile_curve & 15;
        let hole = shape.profile_curve >> 4;
        let path = shape.path_curve & 0xf0;
        if profile > 5 || hole > 3 || shape.path_curve & 15 != 0 || ![0x10, 0x20, 0x30, 0x40, 0x80].contains(&path) {
            return Err(MeshError::UnsupportedCurve);
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
            return Err(MeshError::InvalidParameters);
        }
        if path == 0x20 && profile != 5 && (scale.x < 0.05 || scale.x > 1.0 || scale.y < 0.05 || scale.y > 0.5) {
            return Err(MeshError::InvalidParameters);
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

pub fn tessellate(shape: PrimShape, detail: Detail, cancelled: &AtomicBool) -> Result<Mesh, MeshError> {
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

fn check(cancelled: &AtomicBool) -> Result<(), MeshError> {
    if cancelled.load(Ordering::Acquire) { Err(MeshError::Cancelled) } else { Ok(()) }
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
) -> Result<(), MeshError> {
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

fn cap(mesh: &mut Builder, profile: &Profile, ring: &Ring, top: bool) -> Result<(), MeshError> {
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
