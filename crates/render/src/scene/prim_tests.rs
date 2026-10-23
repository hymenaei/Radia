use std::{f32::consts::PI, sync::atomic::AtomicBool};

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

#[test]
fn box_face_layout() {
    let mesh = generate(PrimShape::default());
    assert_eq!(mesh.faces().len(), 6);
    close(mesh.bounds().min, Vec3::splat(-0.5));
    close(mesh.bounds().max, Vec3::splat(0.5));
    assert!((volume(&mesh) - 1.0).abs() < 1e-5);
    for (face, expected) in mesh.faces().iter().zip([Vec3::Z, -Vec3::Y, Vec3::X, Vec3::Y, -Vec3::X, -Vec3::Z]) {
        assert_eq!(usize::from(face.slot), mesh.faces().iter().position(|f| f == face).unwrap());
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
    let torus = generate(PrimShape { path_curve: 0x20, profile_curve: 0, path_scale_x: 150, path_scale_y: 150, ..PrimShape::default() });
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
fn collapsed_linear_path_keeps_its_empty_end_face() {
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
    assert!(matches!(
        tessellate(PrimShape { profile_curve: 15, ..PrimShape::default() }, Detail::Low, &cancel),
        Err(MeshBuildError::UnsupportedShape)
    ));
    assert!(matches!(
        tessellate(PrimShape { path_curve: 0x11, ..PrimShape::default() }, Detail::Low, &cancel),
        Err(MeshBuildError::UnsupportedShape)
    ));
    assert!(matches!(
        tessellate(PrimShape { profile_begin: 50000, ..PrimShape::default() }, Detail::Low, &cancel),
        Err(MeshBuildError::InvalidParameters)
    ));
    assert!(matches!(tessellate(PrimShape::default(), Detail::Low, &AtomicBool::new(true)), Err(MeshBuildError::Cancelled)));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn bounded_prim_geometry(
        profile in 0u8..6, hole in 0u8..4, path in prop::sample::select(vec![0x10u8, 0x20, 0x30, 0x40, 0x80]),
        cuts in prop::array::uniform4(0u16..20000), hollow in 0u16..=35000,
        scales in prop::array::uniform2(0u8..=200), shear in prop::array::uniform2(-50i8..=50),
        twist in prop::array::uniform2(-100i8..=100), taper in prop::array::uniform2(-100i8..=100),
        radius in -100i8..=100, revolutions in 0u8..=200, skew in -95i8..=95,
    ) {
        let shape = PrimShape { profile_curve: profile | (hole << 4), path_curve: path,
            profile_begin: cuts[0], profile_end: cuts[1], path_begin: cuts[2], path_end: cuts[3], profile_hollow: hollow,
            path_scale_x: if path == 0x20 && profile != 5 { 150 + scales[0] / 5 } else { scales[0] },
            path_scale_y: if path == 0x20 && profile != 5 { 150 + scales[1] / 5 } else { scales[1] },
            path_shear_x: shear[0], path_shear_y: shear[1], path_twist_begin: twist[0], path_twist: twist[1],
            path_taper_x: taper[0], path_taper_y: taper[1], path_radius_offset: radius, path_revolutions: revolutions, path_skew: skew };
        match tessellate(shape, Detail::High, &AtomicBool::new(false)) {
            Ok(mesh) => {
                prop_assert!(mesh.vertices().len() <= MAX_MESH_VERTICES);
                prop_assert!(mesh.indices().len() <= MAX_MESH_INDICES && mesh.indices().len().is_multiple_of(3));
                prop_assert!(mesh.faces().len() <= 9);
                for vertex in mesh.vertices() {
                    prop_assert!(vertex.position.length() <= 3.0, "normalized bound used by camera planning");
                    prop_assert!(vertex.position.is_finite() && vertex.uv.is_finite() && vertex.normal.is_finite());
                    prop_assert!((vertex.normal.length() - 1.0).abs() < 1e-5);
                    prop_assert!(vertex.position.cmpge(mesh.bounds().min).all() && vertex.position.cmple(mesh.bounds().max).all());
                }
                prop_assert!(mesh.indices().iter().all(|i| (*i as usize) < mesh.vertices().len()));
                for (slot, face) in mesh.faces().iter().enumerate() {
                    prop_assert_eq!(usize::from(face.slot), slot);
                    prop_assert!(face.indices.start <= face.indices.end && face.indices.end as usize <= mesh.indices().len());
                }
            }
            Err(error) => prop_assert!(matches!(error, MeshBuildError::InvalidParameters | MeshBuildError::TooComplex | MeshBuildError::Degenerate)),
        }
    }
}
