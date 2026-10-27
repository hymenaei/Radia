use std::assert_matches;

use assets::mesh::{MAX_MESH_INDICES, MAX_MESH_VERTICES};
use math::{Vec2, Vec3};
use proptest::prelude::*;
use utils::AssetId;
use world::{SculptFlags, SculptType};

use super::{Detail, MeshBuildError, tessellate_sculpt};
use crate::scene::SculptMeshKey;

fn map(size: [u32; 2], pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(size[0] as usize * size[1] as usize * 4);
    for y in 0..size[1] {
        for x in 0..size[0] {
            rgba.extend(pixel(x, y));
        }
    }
    rgba
}

fn sample_map() -> Vec<u8> {
    map([8, 8], |x, y| [(x * 30) as u8, (y * 30) as u8, (x * 13 + y * 9) as u8, 255])
}

fn decoded_color(red: u8, green: u8, blue: u8, mirror: bool) -> Vec3 {
    let red = red as f32 * (1.0 / 255.0) - 0.5;
    Vec3::new(if mirror { -red } else { red }, green as f32 * (1.0 / 255.0) - 0.5, blue as f32 * (1.0 / 255.0) - 0.5)
}

fn close(a: Vec3, b: Vec3) {
    assert!(a.distance(b) < 1e-5, "{a:?} != {b:?}");
}

#[test]
fn map_positions_and_winding_match_reference() {
    let rgba = sample_map();
    let combinations =
        [(false, false, [60, 60, 44]), (true, false, [180, 60, 96]), (false, true, [180, 60, 96]), (true, true, [60, 60, 44])];
    for sculpt_type in [SculptType::Sphere, SculptType::Torus, SculptType::Plane, SculptType::Cylinder] {
        for (invert, mirror, middle_rgb) in combinations {
            let flags = SculptFlags { invert, mirror };
            let mesh = tessellate_sculpt([8, 8], &rgba, sculpt_type, flags, Detail::High).unwrap();
            assert_eq!(mesh.vertices().len(), 25, "{sculpt_type:?} {flags:?} vertices");
            assert_eq!(mesh.indices().len(), 96, "{sculpt_type:?} {flags:?} indices");
            assert_eq!(mesh.faces().len(), 1, "sculpt maps use one face");
            assert_eq!(mesh.faces()[0].slot, 0, "the sculpt appearance uses face slot zero");
            assert_eq!(mesh.faces()[0].indices, 0..96);

            let reverse = invert ^ mirror;
            let expected_uv = Vec2::new(if reverse { 0.75 } else { 0.25 }, 0.25);
            let middle = mesh.vertices()[6];
            close(middle.position, decoded_color(middle_rgb[0], middle_rgb[1], middle_rgb[2], mirror));
            assert_eq!(middle.uv, expected_uv);

            let expected_first = match sculpt_type {
                SculptType::Sphere => decoded_color(120, 0, 52, mirror),
                SculptType::Torus | SculptType::Cylinder => decoded_color(0, 0, 0, mirror),
                SculptType::Plane => {
                    let [x, y] = if reverse { [7, 0] } else { [0, 0] };
                    decoded_color((x * 30) as u8, (y * 30) as u8, (x * 13 + y * 9) as u8, mirror)
                }
            };
            let first = mesh.vertices()[0];
            close(first.position, expected_first);
            assert_eq!(first.uv, Vec2::new(if reverse { 1.0 } else { 0.0 }, 0.0));

            let triangles = mesh.indices().as_chunks::<3>().0;
            assert_eq!(triangles[0], [0, 6, 5]);
            assert_eq!(triangles[1], [0, 1, 6]);
            let [a, b, c] = triangles[10].map(|index| mesh.vertices()[index as usize].position);
            let winding = (b - a).cross(c - a).z.signum();
            assert_eq!(winding, if invert { -1.0 } else { 1.0 }, "{sculpt_type:?} {flags:?} winding");
        }
    }
}

#[test]
fn edge_stitching_matches_reference() {
    let rgba = sample_map();
    let sphere = tessellate_sculpt([8, 8], &rgba, SculptType::Sphere, SculptFlags::default(), Detail::High).unwrap();
    close(sphere.vertices()[0].position, decoded_color(120, 0, 52, false));
    close(sphere.vertices()[4].position, decoded_color(120, 0, 52, false));
    close(sphere.vertices()[20].position, decoded_color(120, 210, 115, false));
    close(sphere.vertices()[24].position, decoded_color(120, 210, 115, false));

    let torus = tessellate_sculpt([8, 8], &rgba, SculptType::Torus, SculptFlags::default(), Detail::High).unwrap();
    close(torus.vertices()[0].position, decoded_color(0, 0, 0, false));
    close(torus.vertices()[4].position, torus.vertices()[0].position);
    close(torus.vertices()[20].position, torus.vertices()[0].position);
    close(torus.vertices()[24].position, torus.vertices()[0].position);

    let plane = tessellate_sculpt([8, 8], &rgba, SculptType::Plane, SculptFlags::default(), Detail::High).unwrap();
    close(plane.vertices()[4].position, decoded_color(210, 0, 91, false));
    close(plane.vertices()[20].position, decoded_color(0, 210, 63, false));
    close(plane.vertices()[24].position, decoded_color(210, 210, 154, false));

    let cylinder = tessellate_sculpt([8, 8], &rgba, SculptType::Cylinder, SculptFlags::default(), Detail::High).unwrap();
    close(cylinder.vertices()[4].position, cylinder.vertices()[0].position);
    close(cylinder.vertices()[24].position, cylinder.vertices()[20].position);
    assert_ne!(cylinder.vertices()[20].position, cylinder.vertices()[0].position);
}

#[test]
fn small_maps_use_sphere_placeholder() {
    for size in [[2, 2], [4, 4]] {
        let rgba = map(size, |x, y| [(x * 60) as u8, (y * 60) as u8, 100, 255]);
        for sculpt_type in [SculptType::Sphere, SculptType::Torus, SculptType::Plane, SculptType::Cylinder] {
            for (invert, mirror) in [(false, false), (true, false), (false, true), (true, true)] {
                let flags = SculptFlags { invert, mirror };
                for detail in [Detail::Lowest, Detail::High] {
                    let mesh = tessellate_sculpt(size, &rgba, sculpt_type, flags, detail).unwrap();
                    assert_eq!(mesh.vertices().len(), 25);
                    assert_eq!(mesh.indices().len(), 96);
                    close(mesh.vertices()[0].position, Vec3::new(0.0, 0.0, 0.3));
                    close(mesh.vertices()[2].position, Vec3::new(0.3, 0.0, 0.0));
                    assert_eq!(mesh.vertices()[0].uv.x, if invert ^ mirror { 1.0 } else { 0.0 });
                    assert_eq!(mesh.indices()[..6], [0, 6, 5, 0, 1, 6]);
                }
            }
        }
    }
}

#[test]
fn resolution_matches_detail_and_aspect_ratio() {
    let rgba = map([64, 64], |x, y| [(x * 3) as u8, (y * 3) as u8, 128, 255]);
    for (detail, sides) in [(Detail::Lowest, 6), (Detail::Low, 8), (Detail::Medium, 16), (Detail::High, 32)] {
        let mesh = tessellate_sculpt([64, 64], &rgba, SculptType::Plane, SculptFlags::default(), detail).unwrap();
        assert_eq!(mesh.vertices().len(), (sides + 1) * (sides + 1), "{detail:?} vertex count");
        assert_eq!(mesh.indices().len(), sides * sides * 6, "{detail:?} index count");
    }

    let rgba = map([16, 8], |x, y| [(x * 15) as u8, (y * 30) as u8, (x * 5 + y * 10) as u8, 255]);
    for sculpt_type in [SculptType::Sphere, SculptType::Torus, SculptType::Plane, SculptType::Cylinder] {
        for (invert, mirror) in [(false, false), (true, false), (false, true), (true, true)] {
            let flags = SculptFlags { invert, mirror };
            let mesh = tessellate_sculpt([16, 8], &rgba, sculpt_type, flags, Detail::High).unwrap();
            assert_eq!(mesh.vertices().len(), 45);
            assert_eq!(mesh.indices().len(), 192);
            close(mesh.vertices()[22].position, decoded_color(120, 120, 80, mirror));
            assert_eq!(mesh.vertices()[22].uv, Vec2::new(0.5, 0.5));
            let first_rgb = match sculpt_type {
                SculptType::Sphere => [120, 0, 40],
                SculptType::Torus | SculptType::Cylinder => [0, 0, 0],
                SculptType::Plane if invert ^ mirror => [225, 0, 75],
                SculptType::Plane => [0, 0, 0],
            };
            close(mesh.vertices()[0].position, decoded_color(first_rgb[0], first_rgb[1], first_rgb[2], mirror));
            assert_eq!(mesh.vertices()[0].uv, Vec2::new(if invert ^ mirror { 1.0 } else { 0.0 }, 0.0));
            assert_eq!(mesh.indices()[..6], [0, 10, 9, 0, 1, 10]);
        }
    }
}

#[test]
fn degenerate_map_uses_placeholder_above_lowest_lod() {
    let rgba = vec![127; 8 * 8 * 4];
    let mesh = tessellate_sculpt([8, 8], &rgba, SculptType::Cylinder, SculptFlags::default(), Detail::High).unwrap();
    close(mesh.vertices()[0].position, Vec3::new(0.0, 0.0, 0.3));
    assert!(mesh.vertices().iter().all(|vertex| vertex.position.length() <= 0.300001));
}

#[test]
fn excessive_surface_area_uses_placeholder() {
    let rgba = map([64, 64], |x, y| {
        let x = (x / 2) % 2 != 0;
        let y = (y / 2) % 2 != 0;
        let z = x ^ y;
        [if x { 255 } else { 0 }, if y { 255 } else { 0 }, if z { 255 } else { 0 }, 255]
    });
    let mesh = tessellate_sculpt([64, 64], &rgba, SculptType::Plane, SculptFlags::default(), Detail::High).unwrap();
    close(mesh.vertices()[0].position, Vec3::new(0.0, 0.0, 0.3));
    assert!(mesh.vertices().iter().all(|vertex| vertex.position.length() <= 0.300001));
}

#[test]
fn normals_follow_winding_and_stitches() {
    let rgba = map([8, 8], |x, y| [(x * 30) as u8, (y * 30) as u8, 127, 255]);
    for (invert, mirror, expected_z) in [(false, false, 1.0), (true, false, -1.0), (false, true, 1.0), (true, true, -1.0)] {
        let flags = SculptFlags { invert, mirror };
        let plane = tessellate_sculpt([8, 8], &rgba, SculptType::Plane, flags, Detail::High).unwrap();
        close(plane.vertices()[12].normal, Vec3::Z * expected_z);
        for sculpt_type in [SculptType::Sphere, SculptType::Torus, SculptType::Cylinder] {
            let mesh = tessellate_sculpt([8, 8], &rgba, sculpt_type, flags, Detail::High).unwrap();
            for row in 0..5 {
                close(mesh.vertices()[row * 5].normal, mesh.vertices()[row * 5 + 4].normal);
            }
            if sculpt_type == SculptType::Torus {
                for column in 0..5 {
                    close(mesh.vertices()[column].normal, mesh.vertices()[20 + column].normal);
                }
            }
            if sculpt_type == SculptType::Sphere {
                for column in 1..5 {
                    close(mesh.vertices()[0].normal, mesh.vertices()[column].normal);
                    close(mesh.vertices()[20].normal, mesh.vertices()[20 + column].normal);
                }
            }
        }
    }
}

#[test]
fn sculpt_key_tracks_map_generation_inputs() {
    let base = SculptMeshKey {
        asset_id: AssetId::new(uuid::Uuid::from_u128(1)),
        sculpt_type: SculptType::Sphere,
        flags: SculptFlags::default(),
        detail: Detail::High,
        map_size: [8, 8],
        discard_level: 0,
    };
    for changed in [
        SculptMeshKey { asset_id: AssetId::new(uuid::Uuid::from_u128(2)), ..base },
        SculptMeshKey { sculpt_type: SculptType::Torus, ..base },
        SculptMeshKey { flags: SculptFlags { invert: true, mirror: false }, ..base },
        SculptMeshKey { detail: Detail::Medium, ..base },
        SculptMeshKey { map_size: [16, 8], ..base },
        SculptMeshKey { discard_level: 1, ..base },
    ] {
        assert_ne!(base, changed);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn valid_maps_stay_bounded(
        (width, height, rgba) in (1u32..=32, 1u32..=32).prop_flat_map(|(width, height)| {
            (Just(width), Just(height), prop::collection::vec(any::<u8>(), (width * height * 4) as usize..=(width * height * 4) as usize))
        }),
        sculpt_type in prop::sample::select(vec![SculptType::Sphere, SculptType::Torus, SculptType::Plane, SculptType::Cylinder]),
        invert in any::<bool>(),
        mirror in any::<bool>(),
        detail in prop::sample::select(vec![Detail::Lowest, Detail::Low, Detail::Medium, Detail::High]),
    ) {
        let flags = SculptFlags { invert, mirror };
        let mesh = tessellate_sculpt([width, height], &rgba, sculpt_type, flags, detail)
            .map_err(|error| TestCaseError::fail(format!("valid map {width}x{height} failed: {error:?}")))?;
        prop_assert!(mesh.vertices().len() <= MAX_MESH_VERTICES);
        prop_assert!(!mesh.indices().is_empty() && mesh.indices().len() <= MAX_MESH_INDICES);
        prop_assert_eq!(mesh.faces().len(), 1);
        prop_assert!(mesh.indices().iter().all(|index| (*index as usize) < mesh.vertices().len()));
        for vertex in mesh.vertices() {
            prop_assert!(vertex.position.is_finite() && vertex.normal.is_finite() && vertex.uv.is_finite());
            prop_assert!(vertex.position.abs().max_element() <= 0.5);
            prop_assert!((0.0..=1.0).contains(&vertex.uv.x) && (0.0..=1.0).contains(&vertex.uv.y));
        }
    }

    #[test]
    fn arbitrary_maps_never_panic(
        width in 0u32..=4100,
        height in 0u32..=4100,
        rgba in prop::collection::vec(any::<u8>(), 0..=256),
        sculpt_type in prop::sample::select(vec![SculptType::Sphere, SculptType::Torus, SculptType::Plane, SculptType::Cylinder]),
        invert in any::<bool>(),
        mirror in any::<bool>(),
        detail in prop::sample::select(vec![Detail::Lowest, Detail::Low, Detail::Medium, Detail::High]),
    ) {
        let result = tessellate_sculpt([width, height], &rgba, sculpt_type, SculptFlags { invert, mirror }, detail);
        if let Ok(mesh) = result {
            prop_assert!(mesh.vertices().len() <= MAX_MESH_VERTICES);
            prop_assert!(mesh.indices().len() <= MAX_MESH_INDICES);
            prop_assert!(mesh.vertices().iter().all(|vertex| vertex.position.is_finite() && vertex.normal.is_finite() && vertex.uv.is_finite()));
            prop_assert!(mesh.vertices().iter().all(|vertex| vertex.position.abs().max_element() <= 0.5));
        }
    }
}

#[test]
fn malformed_maps_return_typed_errors() {
    assert_matches!(
        tessellate_sculpt([0, 8], &[], SculptType::Sphere, SculptFlags::default(), Detail::Low).err(),
        Some(MeshBuildError::TooComplex)
    );
    assert_matches!(
        tessellate_sculpt([4097, 8], &[], SculptType::Sphere, SculptFlags::default(), Detail::Low).err(),
        Some(MeshBuildError::TooComplex)
    );
    assert_matches!(
        tessellate_sculpt([8, 8], &[0; 255], SculptType::Sphere, SculptFlags::default(), Detail::Low).err(),
        Some(MeshBuildError::InvalidParameters)
    );
}
