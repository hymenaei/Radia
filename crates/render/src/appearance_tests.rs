use super::*;
use assets::{MaterialTexture, PbrMaterial};
use base::{AssetId, UUID};
use scene::{AssetState, Resolution};
use std::f32::consts::FRAC_PI_2;

fn mapped(matrix: [[f32; 4]; 2], uv: [f32; 2]) -> [f32; 2] {
    matrix.map(|row| row[0] * uv[0] + row[1] * uv[1] + row[2])
}

#[test]
fn legacy_transform_order() {
    let matrix = legacy_uv(TextureTransform { offset: [0.125, -0.25], scale: [2.0, 3.0], rotation: FRAC_PI_2 });
    let actual = mapped(matrix, [0.75, 0.625]);
    for (actual, expected) in actual.into_iter().zip([0.875, 1.5]) {
        assert!((actual - expected).abs() < 1e-6, "legacy UVs rotate about center before scaling, then flip V");
    }
    assert_eq!(mapped(legacy_uv(TextureTransform::default()), [0.0, 0.0]), [0.0, 1.0]);
}

#[test]
fn pbr_transform_order() {
    let matrix = pbr_uv(TextureTransform { offset: [0.125, -0.25], scale: [2.0, 3.0], rotation: FRAC_PI_2 });
    let actual = mapped(matrix, [0.75, 0.625]);
    for (actual, expected) in actual.into_iter().zip([-1.0, 1.25]) {
        assert!((actual - expected).abs() < 1e-6, "glTF UVs flip V before scale, origin rotation, and translation");
    }
    assert_eq!(mapped(pbr_uv(TextureTransform::default()), [0.0, 1.0]), [0.0, 0.0]);
}

#[test]
fn fallback_preserves_material() {
    let id = AssetId::new(UUID::from_u128(42));
    let mut material = PbrMaterial {
        base_color_factor: [0.2, 0.3, 0.4, 0.5],
        double_sided: true,
        unlit: true,
        base_color: MaterialTexture { tex_coord: 1, ..Default::default() },
        ..Default::default()
    };
    let mut face = FaceMaterial {
        slot: 0,
        material: MaterialDescription::Pbr(material),
        legacy_state: None,
        pbr_state: None,
        alpha: Alpha::Mask(0.7),
        base_color: None,
        base_color_state: Some(AssetState { asset: id, resolution: Resolution::Pending }),
    };
    let params = FaceParams::new(Some(&face));
    assert_eq!(params.color, material.base_color_factor);
    assert_eq!(params.params, [1.0, 1.0, 0.7, 0.0]);
    assert!(params.double_sided);
    assert!(!params.blend);
    assert_eq!(params.texture, None, "unsupported UV sets use a neutral image");
    material.base_color.tex_coord = 0;
    face.material = MaterialDescription::Pbr(material);
    face.alpha = Alpha::Blend;
    let params = FaceParams::new(Some(&face));
    assert_eq!(params.texture, None, "pending assets do not own a GPU texture");
    assert!(params.blend);
    assert_eq!(params.params[1], 2.0);
}
