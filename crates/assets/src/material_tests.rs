use super::*;
use crate::Priority;
use crate::{Budget, Content, Kind, Request, Resource, Status, Store};
use std::time::Instant;

fn pbr_description(json: &[u8]) -> PbrMaterial {
    let MaterialDescription::Pbr(material) = pbr(json).unwrap() else { panic!("expected PBR") };
    material
}

#[test]
fn factors_and_defaults() {
    assert_eq!(pbr_description(br#"{"asset":{"version":"2.0"},"materials":[{}]}"#), PbrMaterial::default());
    let material = pbr_description(br#"{"asset":{"version":"2.0"},"materials":[{"pbrMetallicRoughness":{"baseColorFactor":[0.2,0.4,0.6,0.8]},"alphaMode":"BLEND","doubleSided":true}]}"#);
    assert_eq!(material.base_color_factor, [0.2, 0.4, 0.6, 0.8]);
    assert_eq!(material.base_color.texture, TextureRef::Absent);
    assert_eq!(material.alpha_mode, AlphaMode::Blend);
    assert!(material.double_sided);
    assert_eq!(pbr_description(br#"{"asset":{"version":"2.0"},"materials":[{"alphaCutoff":1.25}]}"#).alpha_cutoff, 1.25);
}

#[test]
fn color_channel_selection() {
    let color = MaterialTexture {
        texture: TextureRef::Asset(AssetId::new(Uuid::from_u128(17))),
        transform: TextureTransform { offset: [0.25, -0.5], scale: [2.0, -3.0], rotation: 0.75 },
        tex_coord: 2,
        sampler: Sampler { wrap_s: Wrap::ClampToEdge, ..Sampler::default() },
    };
    let legacy = MaterialDescription::Legacy(LegacyMaterial { diffuse: color, ..LegacyMaterial::default() });
    let pbr = MaterialDescription::Pbr(PbrMaterial { base_color: color, ..PbrMaterial::default() });
    for material in [legacy, pbr] {
        assert_eq!(*material.base_color_texture(), color, "the color channel retains its own reference, UVs and sampler");
    }
}

const TEXTURED: &str = r#"{
    "asset":{"version":"2.0"},
    "images":[{"uri":"00112233-4455-6677-8899-aabbccddeeff"}],
    "textures":[{"source":0}],
    "materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0,
        "extensions":{"KHR_texture_transform":{"offset":[0.25,-0.5],"scale":[2,3],"rotation":1.5}}}},
        "alphaMode":"MASK","alphaCutoff":0.25,"doubleSided":false}]
}"#;

const CHANNELS: &str = r#"{
    "asset":{"version":"2.0"},
    "images":[
        {"uri":"00000000-0000-0000-0000-000000000001"},
        {"uri":"00000000-0000-0000-0000-000000000002"},
        {"uri":"00000000-0000-0000-0000-000000000003"},
        {"uri":"00000000-0000-0000-0000-000000000004"},
        {"uri":"00000000-0000-0000-0000-000000000005"}
    ],
    "samplers":[{"magFilter":9728,"minFilter":9987,"wrapS":33071,"wrapT":33648}],
    "textures":[{"source":0},{"source":1},{"source":2,"sampler":0},{"source":3},{"source":4}],
    "materials":[{
        "pbrMetallicRoughness":{
            "baseColorTexture":{"index":0},
            "metallicRoughnessTexture":{"index":2,"texCoord":1,
                "extensions":{"KHR_texture_transform":{"offset":[-0.25,0.75],"scale":[-2,0],"rotation":-1,"texCoord":3}}},
            "metallicFactor":0.3,"roughnessFactor":0.7
        },
        "normalTexture":{"index":1,"scale":-0.5,"texCoord":2},
        "emissiveTexture":{"index":3},"emissiveFactor":[0.1,0.2,0.3],
        "occlusionTexture":{"index":4,"strength":0.4},
        "extensions":{"KHR_materials_unlit":{}}
    }]
}"#;

#[test]
fn pbr_channels_survive() {
    let material = pbr_description(CHANNELS.as_bytes());
    for (info, id) in
        [(material.base_color, 1), (material.normal, 2), (material.metallic_roughness, 3), (material.emissive, 4), (material.occlusion, 5)]
    {
        assert_eq!(info.texture, TextureRef::Asset(AssetId::new(Uuid::from_u128(id))));
    }
    assert_eq!(material.metallic, 0.3);
    assert_eq!(material.roughness, 0.7);
    assert_eq!(material.emissive_factor, [0.1, 0.2, 0.3]);
    assert_eq!(material.normal_scale, -0.5);
    assert_eq!(material.occlusion_strength, 0.4);
    assert!(material.unlit);
    assert_eq!(material.normal.tex_coord, 2);
    assert_eq!(material.normal.sampler, Sampler::default());
    let info = material.metallic_roughness;
    assert_eq!(info.tex_coord, 3);
    assert_eq!(info.transform, TextureTransform { offset: [-0.25, 0.75], scale: [-2.0, 0.0], rotation: -1.0 });
    assert_eq!(
        info.sampler,
        Sampler {
            mag_filter: Some(Filter::Nearest),
            min_filter: Some(MinFilter::LinearMipmapLinear),
            wrap_s: Wrap::ClampToEdge,
            wrap_t: Wrap::MirroredRepeat,
        }
    );
}

#[test]
fn auxiliary_channels_validate() {
    for (from, to) in [
        ("\"index\":1", "\"index\":99"),
        ("\"scale\":-0.5", "\"scale\":1e100"),
        ("\"strength\":0.4", "\"strength\":-0.1"),
        ("\"metallicFactor\":0.3", "\"metallicFactor\":1.00000001"),
        ("\"roughnessFactor\":0.7", "\"roughnessFactor\":null"),
        ("\"emissiveFactor\":[0.1,0.2,0.3]", "\"emissiveFactor\":[0.1,0.2]"),
        ("\"texCoord\":2", "\"texCoord\":4294967296"),
        ("\"KHR_materials_unlit\":{}", "\"KHR_materials_unlit\":false"),
        ("\"sampler\":0", "\"sampler\":1"),
        ("\"magFilter\":9728", "\"magFilter\":9987"),
        ("\"minFilter\":9987", "\"minFilter\":0"),
        ("\"wrapS\":33071", "\"wrapS\":0"),
        ("\"wrapT\":33648", "\"wrapT\":null"),
    ] {
        assert_eq!(pbr(CHANNELS.replace(from, to).as_bytes()), Err(Failure::Malformed), "{from} -> {to}");
    }
}

#[test]
fn texture_transform_and_clear() {
    let material = pbr_description(TEXTURED.as_bytes());
    assert_eq!(material.base_color.texture, TextureRef::Asset(AssetId::new(Uuid::from_u128(0x00112233445566778899aabbccddeeff))));
    assert_eq!(material.base_color.transform, TextureTransform { offset: [0.25, -0.5], scale: [2.0, 3.0], rotation: 1.5 });
    assert_eq!(material.alpha_mode, AlphaMode::Mask);
    assert_eq!(material.alpha_cutoff, 0.25);
    assert!(!material.double_sided);
    for clear in ["00000000-0000-0000-0000-000000000000", "ffffffff-ffff-ffff-ffff-ffffffffffff"] {
        let json = TEXTURED.replace("00112233-4455-6677-8899-aabbccddeeff", clear);
        assert_eq!(pbr_description(json.as_bytes()).base_color.texture, TextureRef::Clear);
    }
}

#[test]
fn invalid_materials_fail() {
    for json in [
        TEXTURED.replace("\"source\":0", "\"source\":99"),
        TEXTURED.replace("\"index\":0", "\"index\":-1"),
        TEXTURED.replace("\"rotation\":1.5", "\"rotation\":1e100"),
        TEXTURED.replace("\"alphaCutoff\":0.25", "\"alphaCutoff\":-0.1"),
        TEXTURED.replace("\"doubleSided\":false", "\"doubleSided\":0"),
        TEXTURED.replace("\"scale\":[2,3]", "\"scale\":[2]"),
        TEXTURED.replace("\"alphaMode\":\"MASK\"", "\"alphaMode\":null"),
        r#"{"asset":{"version":"2.0"},"materials":[{"pbrMetallicRoughness":{"baseColorFactor":[1,1,1,1.00000001]}}]}"#.into(),
        r#"{"asset":{"version":"2.0"},"asset":{"version":"2.0"}}"#.into(),
    ] {
        assert_eq!(pbr(json.as_bytes()), Err(Failure::Malformed), "{json}");
    }
    for json in [
        TEXTURED.replace("00112233-4455-6677-8899-aabbccddeeff", "https://example.com/image.png"),
        TEXTURED.replace("\"MASK\"", "\"UNKNOWN\""),
        TEXTURED.replace("\"2.0\"", "\"3.0\""),
        TEXTURED.replace("\"version\":\"2.0\"", "\"version\":\"2.0\",\"minVersion\":\"2.1\""),
        TEXTURED.replace("\"asset\":", "\"extensionsRequired\":[\"KHR_materials_transmission\"],\"asset\":"),
    ] {
        assert_eq!(pbr(json.as_bytes()), Err(Failure::Unsupported), "{json}");
    }
    assert_eq!(pbr(&TEXTURED.as_bytes()[..TEXTURED.len() - 1]), Err(Failure::Malformed));
}

#[test]
fn ignored_fields_stay_bounded() {
    let nested = format!(r#"{{"asset":{{"version":"2.0"}},"extras":{}0{}}}"#, "[".repeat(17), "]".repeat(17));
    assert_eq!(pbr(nested.as_bytes()), Err(Failure::Malformed));
    let wide = format!(r#"{{"asset":{{"version":"2.0"}},"extras":[{}0]}}"#, "0,".repeat(512));
    assert_eq!(pbr(wide.as_bytes()), Err(Failure::Malformed));
    let values = format!(r#"{{"asset":{{"version":"2.0"}},"extras":[{}[]]}}"#, format!("[{}0],", "0,".repeat(499)).repeat(9));
    assert_eq!(pbr(values.as_bytes()), Err(Failure::Malformed));
    assert_eq!(pbr(&vec![b' '; 2 * 1024 * 1024 + 1]), Err(Failure::TooLarge));
}

fn legacy_fields(mode: i32, cutoff: i32) -> llsd::Map {
    let mut fields: llsd::Map = [
        ("NormMap".into(), llsd::Value::Uuid(Uuid::from_u128(7))),
        ("SpecMap".into(), llsd::Value::Uuid(Uuid::nil())),
        ("SpecColor".into(), llsd::Value::Array([51, 102, 153, 204].map(llsd::Value::Integer).into())),
    ]
    .into();
    for (name, value) in [
        ("NormOffsetX", 2500),
        ("NormOffsetY", -5000),
        ("NormRepeatX", 20000),
        ("NormRepeatY", 30000),
        ("NormRotation", 15000),
        ("SpecOffsetX", -1250),
        ("SpecOffsetY", 7500),
        ("SpecRepeatX", -10000),
        ("SpecRepeatY", 0),
        ("SpecRotation", -20000),
        ("SpecExp", 137),
        ("EnvIntensity", 73),
        ("DiffuseAlphaMode", mode),
        ("AlphaMaskCutoff", cutoff),
    ] {
        fields.insert(name.into(), llsd::Value::Integer(value));
    }
    fields
}

fn legacy_bytes(mode: i32, cutoff: i32) -> Vec<u8> {
    binary::to_vec(&llsd::Value::Map(legacy_fields(mode, cutoff))).unwrap()
}

fn legacy_expected(alpha_mode: LegacyAlphaMode) -> LegacyMaterial {
    LegacyMaterial {
        normal: MaterialTexture {
            texture: TextureRef::Asset(AssetId::new(Uuid::from_u128(7))),
            transform: TextureTransform { offset: [0.25, -0.5], scale: [2.0, 3.0], rotation: 1.5 },
            ..MaterialTexture::default()
        },
        specular: MaterialTexture {
            texture: TextureRef::Clear,
            transform: TextureTransform { offset: [-0.125, 0.75], scale: [-1.0, 0.0], rotation: -2.0 },
            ..MaterialTexture::default()
        },
        specular_color: [51, 102, 153, 204],
        specular_exponent: 137,
        environment_intensity: 73,
        alpha_mode,
        alpha_cutoff: 128.0 / 255.0,
        ..LegacyMaterial::default()
    }
}

#[test]
fn legacy_channels_survive() {
    for (wire, mode) in
        [(0, LegacyAlphaMode::Opaque), (1, LegacyAlphaMode::Blend), (2, LegacyAlphaMode::Mask), (3, LegacyAlphaMode::Emissive)]
    {
        assert_eq!(legacy(&legacy_bytes(wire, 128)), Ok(MaterialDescription::Legacy(legacy_expected(mode))));
    }
    assert_eq!(legacy(&legacy_bytes(4, 0)), Err(Failure::Unsupported));
    assert_eq!(legacy(&legacy_bytes(1, 256)), Err(Failure::Malformed));
    assert_eq!(legacy(b"{\0\0\0\0}"), Err(Failure::Malformed));
}

#[test]
fn legacy_fields_validate() {
    let fields = legacy_fields(1, 128);
    for name in fields.keys() {
        let mut missing = fields.clone();
        missing.remove(name);
        assert_eq!(legacy(&binary::to_vec(&llsd::Value::Map(missing)).unwrap()), Err(Failure::Malformed), "missing {name}");
        let mut mistyped = fields.clone();
        mistyped.insert(name.clone(), llsd::Value::String("invalid".into()));
        assert_eq!(legacy(&binary::to_vec(&llsd::Value::Map(mistyped)).unwrap()), Err(Failure::Malformed), "mistyped {name}");
    }
    for (name, value) in [
        ("SpecExp", llsd::Value::Integer(-1)),
        ("EnvIntensity", llsd::Value::Integer(256)),
        ("SpecColor", llsd::Value::Array(vec![llsd::Value::Integer(255); 3])),
        ("SpecColor", llsd::Value::Array(vec![llsd::Value::Integer(256); 4])),
    ] {
        let mut invalid = fields.clone();
        invalid.insert(name.into(), value);
        assert_eq!(legacy(&binary::to_vec(&llsd::Value::Map(invalid)).unwrap()), Err(Failure::Malformed), "invalid {name}");
    }
}

fn envelope(json: &str) -> Vec<u8> {
    let mut bytes = b"{\0\0\0\x03k\0\0\0\x07versions\0\0\0\x031.1k\0\0\0\x04types\0\0\0\x08GLTF 2.0k\0\0\0\x04datas".to_vec();
    bytes.extend((json.len() as u32).to_be_bytes());
    bytes.extend_from_slice(json.as_bytes());
    bytes.push(b'}');
    bytes
}

fn legacy_envelope(id: AssetId) -> Vec<u8> {
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;
    let record = binary::to_vec(&llsd::Value::Array(vec![llsd::Value::Map(
        [
            ("ID".into(), llsd::Value::Binary(llsd::Binary::new(id.uuid().as_bytes().to_vec()))),
            ("Material".into(), llsd::Value::Map(legacy_fields(2, 128))),
        ]
        .into(),
    )]))
    .unwrap();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&record).unwrap();
    let zipped = encoder.finish().unwrap();
    llsd::xml::to_vec(&llsd::Value::Map([("Zipped".into(), llsd::Value::Binary(llsd::Binary::new(zipped)))].into())).unwrap()
}

#[test]
fn shared_material_retains_charge() {
    let now = Instant::now();
    let budget = Budget::default();
    let mut store = Store::new(budget.clone());
    let route = store.register_route().unwrap();
    let resource = Resource { id: AssetId::new(Uuid::from_u128(2)), kind: Kind::PbrMaterial };
    let first = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
    let second = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    let prepared = job.prepare(envelope(TEXTURED)).unwrap();
    store.complete(job, Ok(prepared), now);
    let Status::Ready(Content::Material(material)) = first.status() else { panic!("expected material") };
    let Status::Ready(Content::Material(shared)) = second.status() else { panic!("expected shared material") };
    assert!(std::ptr::eq(&*material, &*shared));
    assert_eq!(budget.stats().resident_bytes, size_of::<MaterialDescription>());
    store.close();
    assert!(matches!(second.status(), Status::StaleSession));
    assert_eq!(*material, MaterialDescription::Pbr(pbr_description(TEXTURED.as_bytes())));
    drop(material);
    assert_eq!(budget.stats().charged_bytes, size_of::<MaterialDescription>());
    drop(shared);
    assert_eq!(budget.stats().charged_bytes, 0);
}

#[test]
fn retired_material_cannot_publish() {
    let now = Instant::now();
    let budget = Budget::default();
    let mut store = Store::new(budget.clone());
    let route = store.register_route().unwrap();
    let resource = Resource { id: AssetId::new(Uuid::from_u128(3)), kind: Kind::LegacyMaterial };
    let old = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    let prepared = job.prepare(legacy_envelope(resource.id)).unwrap();
    store.retire_route(route);
    store.complete(job, Ok(prepared), now);
    assert!(matches!(old.status(), Status::Failed { failure: Failure::RouteClosed, .. }));
    assert_eq!(budget.stats().charged_bytes, 0);
    let route = store.register_route().unwrap();
    let current = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    let result = job.prepare(legacy_envelope(resource.id));
    store.complete(job, result, now);
    let Status::Ready(Content::Material(material)) = current.status() else { panic!("expected legacy material") };
    assert_eq!(*material, MaterialDescription::Legacy(legacy_expected(LegacyAlphaMode::Mask)));
}

#[test]
fn material_identity_must_match() {
    let now = Instant::now();
    let budget = Budget::default();
    let mut store = Store::new(budget.clone());
    let route = store.register_route().unwrap();
    let resource = Resource { id: AssetId::new(Uuid::from_u128(1)), kind: Kind::PbrMaterial };
    let demand = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    let other = Resource { id: AssetId::new(Uuid::from_u128(2)), ..resource };
    let _other_demand = store.request(route, Request::new(other), Priority::Visible, now).unwrap();
    let mut other_job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut other_job));
    let result = other_job.prepare(envelope(TEXTURED));
    drop(other_job);
    store.complete(job, result, now);
    assert!(matches!(demand.status(), Status::Failed { failure: Failure::Malformed, retry_at: None }));
    assert_eq!(budget.stats().charged_bytes, 0);
}
