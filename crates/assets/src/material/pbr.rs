use serde_json::{Map, Value};
use uuid::Uuid;

use super::{Filter, MaterialDescription, MaterialTexture, MinFilter, Sampler, TextureRef, Wrap, json};
use crate::Failure;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PbrAlphaMode {
    Opaque,
    Mask,
    Blend,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PbrMaterial {
    pub base_color_factor: [f32; 4],
    pub base_color: MaterialTexture,
    pub normal: MaterialTexture,
    pub metallic_roughness: MaterialTexture,
    pub emissive: MaterialTexture,
    pub occlusion: MaterialTexture,
    pub metallic: f32,
    pub roughness: f32,
    pub emissive_factor: [f32; 3],
    pub normal_scale: f32,
    pub occlusion_strength: f32,
    pub unlit: bool,
    pub alpha_mode: PbrAlphaMode,
    pub alpha_cutoff: f32,
    pub double_sided: bool,
}

impl Default for PbrMaterial {
    fn default() -> Self {
        Self {
            base_color_factor: [1.0; 4],
            base_color: MaterialTexture::default(),
            normal: MaterialTexture::default(),
            metallic_roughness: MaterialTexture::default(),
            emissive: MaterialTexture::default(),
            occlusion: MaterialTexture::default(),
            metallic: 1.0,
            roughness: 1.0,
            emissive_factor: [0.0; 3],
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            unlit: false,
            alpha_mode: PbrAlphaMode::Opaque,
            alpha_cutoff: 0.5,
            double_sided: false,
        }
    }
}

fn object(value: &Value) -> Result<&Map<String, Value>, Failure> {
    value.as_object().ok_or(Failure::Malformed)
}

fn number(value: &Value) -> Result<f32, Failure> {
    let value = value.as_f64().ok_or(Failure::Malformed)? as f32;
    if value.is_finite() { Ok(value) } else { Err(Failure::Malformed) }
}

fn vector<const N: usize>(value: &Value) -> Result<[f32; N], Failure> {
    let values = value.as_array().filter(|values| values.len() == N).ok_or(Failure::Malformed)?;
    let mut result = [0.0; N];
    for (dest, value) in result.iter_mut().zip(values) {
        *dest = number(value)?;
    }
    Ok(result)
}

fn unit(value: &Value) -> Result<f32, Failure> {
    let value = value.as_f64().ok_or(Failure::Malformed)?;
    if (0.0..=1.0).contains(&value) { Ok(value as f32) } else { Err(Failure::Malformed) }
}

fn index<'a>(root: &'a Map<String, Value>, collection: &str, value: &Value) -> Result<&'a Value, Failure> {
    let index = value.as_u64().and_then(|v| usize::try_from(v).ok()).ok_or(Failure::Malformed)?;
    root.get(collection).and_then(Value::as_array).and_then(|items| items.get(index)).ok_or(Failure::Malformed)
}

fn sampler(value: &Value) -> Result<Sampler, Failure> {
    let fields = object(value)?;
    let mut result = Sampler::default();
    if let Some(filter) = fields.get("magFilter") {
        result.mag_filter = Some(match filter.as_u64().ok_or(Failure::Malformed)? {
            9728 => Filter::Nearest,
            9729 => Filter::Linear,
            _ => return Err(Failure::Malformed),
        });
    }
    if let Some(filter) = fields.get("minFilter") {
        result.min_filter = Some(match filter.as_u64().ok_or(Failure::Malformed)? {
            9728 => MinFilter::Nearest,
            9729 => MinFilter::Linear,
            9984 => MinFilter::NearestMipmapNearest,
            9985 => MinFilter::LinearMipmapNearest,
            9986 => MinFilter::NearestMipmapLinear,
            9987 => MinFilter::LinearMipmapLinear,
            _ => return Err(Failure::Malformed),
        });
    }
    for (key, dest) in [("wrapS", &mut result.wrap_s), ("wrapT", &mut result.wrap_t)] {
        if let Some(wrap) = fields.get(key) {
            *dest = match wrap.as_u64().ok_or(Failure::Malformed)? {
                33071 => Wrap::ClampToEdge,
                33648 => Wrap::MirroredRepeat,
                10497 => Wrap::Repeat,
                _ => return Err(Failure::Malformed),
            };
        }
    }
    Ok(result)
}

fn texture_info(root: &Map<String, Value>, value: &Value) -> Result<MaterialTexture, Failure> {
    let info = object(value)?;
    let texture = object(index(root, "textures", info.get("index").ok_or(Failure::Malformed)?)?)?;
    let image = object(index(root, "images", texture.get("source").ok_or(Failure::Malformed)?)?)?;
    let uri = image.get("uri").and_then(Value::as_str).ok_or(Failure::Unsupported)?;
    let id = Uuid::parse_str(uri).map_err(|_| Failure::Unsupported)?;
    let mut result = MaterialTexture { source: TextureRef::from(id), ..MaterialTexture::default() };
    if let Some(value) = texture.get("sampler") {
        result.sampler = sampler(index(root, "samplers", value)?)?;
    }
    let mut tex_coord = info.get("texCoord");
    if let Some(extensions) = info.get("extensions")
        && let Some(transform) = object(extensions)?.get("KHR_texture_transform")
    {
        let transform = object(transform)?;
        if let Some(offset) = transform.get("offset") {
            result.transform.offset = vector(offset)?;
        }
        if let Some(scale) = transform.get("scale") {
            result.transform.scale = vector(scale)?;
        }
        if let Some(rotation) = transform.get("rotation") {
            result.transform.rotation = number(rotation)?;
        }
        tex_coord = transform.get("texCoord").or(tex_coord);
    }
    if let Some(coord) = tex_coord {
        result.tex_coord = coord.as_u64().and_then(|value| u32::try_from(value).ok()).ok_or(Failure::Malformed)?;
    }
    Ok(result)
}

fn unit_vector<const N: usize>(value: &Value) -> Result<[f32; N], Failure> {
    let values = value.as_array().filter(|values| values.len() == N).ok_or(Failure::Malformed)?;
    let mut result = [0.0; N];
    for (dest, value) in result.iter_mut().zip(values) {
        *dest = unit(value)?;
    }
    Ok(result)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<MaterialDescription, Failure> {
    let root = json::parse(bytes)?;
    let root = object(&root)?;
    let asset = object(root.get("asset").ok_or(Failure::Malformed)?)?;
    if asset.get("version").and_then(Value::as_str) != Some("2.0") {
        return Err(Failure::Unsupported);
    }
    if let Some(version) = asset.get("minVersion")
        && version.as_str().ok_or(Failure::Malformed)? != "2.0"
    {
        return Err(Failure::Unsupported);
    }
    if let Some(required) = root.get("extensionsRequired") {
        for extension in required.as_array().ok_or(Failure::Malformed)? {
            match extension.as_str().ok_or(Failure::Malformed)? {
                "KHR_texture_transform" | "KHR_materials_unlit" => {}
                _ => return Err(Failure::Unsupported),
            }
        }
    }
    let mut result = PbrMaterial::default();
    let material = match root.get("materials") {
        Some(value) => value.as_array().ok_or(Failure::Malformed)?.first(),
        None => None,
    };
    let Some(material) = material else {
        return Ok(MaterialDescription::Pbr(result));
    };
    let material = object(material)?;
    if let Some(extensions) = material.get("extensions")
        && let Some(unlit) = object(extensions)?.get("KHR_materials_unlit")
    {
        object(unlit)?;
        result.unlit = true;
    }
    if let Some(mode) = material.get("alphaMode") {
        result.alpha_mode = match mode.as_str().ok_or(Failure::Malformed)? {
            "OPAQUE" => PbrAlphaMode::Opaque,
            "MASK" => PbrAlphaMode::Mask,
            "BLEND" => PbrAlphaMode::Blend,
            _ => return Err(Failure::Unsupported),
        };
    }
    if let Some(cutoff) = material.get("alphaCutoff") {
        result.alpha_cutoff = number(cutoff)?;
        if result.alpha_cutoff < 0.0 {
            return Err(Failure::Malformed);
        }
    }
    if let Some(double_sided) = material.get("doubleSided") {
        result.double_sided = double_sided.as_bool().ok_or(Failure::Malformed)?;
    }
    if let Some(pbr) = material.get("pbrMetallicRoughness") {
        let pbr = object(pbr)?;
        if let Some(color) = pbr.get("baseColorFactor") {
            result.base_color_factor = unit_vector(color)?;
        }
        if let Some(info) = pbr.get("baseColorTexture") {
            result.base_color = texture_info(root, info)?;
        }
        if let Some(info) = pbr.get("metallicRoughnessTexture") {
            result.metallic_roughness = texture_info(root, info)?;
        }
        if let Some(value) = pbr.get("metallicFactor") {
            result.metallic = unit(value)?;
        }
        if let Some(value) = pbr.get("roughnessFactor") {
            result.roughness = unit(value)?;
        }
    }
    if let Some(value) = material.get("emissiveFactor") {
        result.emissive_factor = unit_vector(value)?;
    }
    if let Some(info) = material.get("emissiveTexture") {
        result.emissive = texture_info(root, info)?;
    }
    if let Some(info) = material.get("normalTexture") {
        result.normal = texture_info(root, info)?;
        if let Some(scale) = object(info)?.get("scale") {
            result.normal_scale = number(scale)?;
        }
    }
    if let Some(info) = material.get("occlusionTexture") {
        result.occlusion = texture_info(root, info)?;
        if let Some(strength) = object(info)?.get("strength") {
            result.occlusion_strength = unit(strength)?;
        }
    }
    Ok(MaterialDescription::Pbr(result))
}

#[cfg(test)]
mod tests {
    use math::UvTransform;
    use utils::AssetId;

    use super::*;

    fn pbr_description(json: &[u8]) -> PbrMaterial {
        let MaterialDescription::Pbr(material) = decode(json).unwrap() else { panic!("expected PBR") };
        material
    }

    #[test]
    fn factors_and_defaults() {
        assert_eq!(pbr_description(br#"{"asset":{"version":"2.0"},"materials":[{}]}"#), PbrMaterial::default());
        let material = pbr_description(br#"{"asset":{"version":"2.0"},"materials":[{"pbrMetallicRoughness":{"baseColorFactor":[0.2,0.4,0.6,0.8]},"alphaMode":"BLEND","doubleSided":true}]}"#);
        assert_eq!(material.base_color_factor, [0.2, 0.4, 0.6, 0.8]);
        assert_eq!(material.base_color.source, TextureRef::Absent);
        assert_eq!(material.alpha_mode, PbrAlphaMode::Blend);
        assert!(material.double_sided);
        assert_eq!(pbr_description(br#"{"asset":{"version":"2.0"},"materials":[{"alphaCutoff":1.25}]}"#).alpha_cutoff, 1.25);
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
        for (info, id) in [
            (material.base_color, 1),
            (material.normal, 2),
            (material.metallic_roughness, 3),
            (material.emissive, 4),
            (material.occlusion, 5),
        ] {
            assert_eq!(info.source, TextureRef::Asset(AssetId::new(Uuid::from_u128(id))));
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
        assert_eq!(info.transform, UvTransform { offset: [-0.25, 0.75], scale: [-2.0, 0.0], rotation: -1.0 });
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
            assert_eq!(decode(CHANNELS.replace(from, to).as_bytes()), Err(Failure::Malformed), "{from} -> {to}");
        }
    }

    #[test]
    fn texture_transform_and_clear() {
        let material = pbr_description(TEXTURED.as_bytes());
        assert_eq!(material.base_color.source, TextureRef::Asset(AssetId::new(Uuid::from_u128(0x00112233445566778899aabbccddeeff))));
        assert_eq!(material.base_color.transform, UvTransform { offset: [0.25, -0.5], scale: [2.0, 3.0], rotation: 1.5 });
        assert_eq!(material.alpha_mode, PbrAlphaMode::Mask);
        assert_eq!(material.alpha_cutoff, 0.25);
        assert!(!material.double_sided);
        for clear in ["00000000-0000-0000-0000-000000000000", "ffffffff-ffff-ffff-ffff-ffffffffffff"] {
            let json = TEXTURED.replace("00112233-4455-6677-8899-aabbccddeeff", clear);
            assert_eq!(pbr_description(json.as_bytes()).base_color.source, TextureRef::Clear);
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
        ] {
            assert_eq!(decode(json.as_bytes()), Err(Failure::Malformed), "{json}");
        }
        for json in [
            TEXTURED.replace("00112233-4455-6677-8899-aabbccddeeff", "https://example.com/image.png"),
            TEXTURED.replace("\"MASK\"", "\"UNKNOWN\""),
            TEXTURED.replace("\"2.0\"", "\"3.0\""),
            TEXTURED.replace("\"version\":\"2.0\"", "\"version\":\"2.0\",\"minVersion\":\"2.1\""),
            TEXTURED.replace("\"asset\":", "\"extensionsRequired\":[\"KHR_materials_transmission\"],\"asset\":"),
        ] {
            assert_eq!(decode(json.as_bytes()), Err(Failure::Unsupported), "{json}");
        }
        assert_eq!(decode(&TEXTURED.as_bytes()[..TEXTURED.len() - 1]), Err(Failure::Malformed));
    }
}
