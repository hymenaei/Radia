use base::{AssetId, UUID};
use llsd::{Limits, binary};
use serde_json::{Map, Value};

use crate::Failure;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaMode {
    Opaque,
    Mask,
    Blend,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAlphaMode {
    Opaque,
    Blend,
    Mask,
    Emissive,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LegacyMaterial {
    pub diffuse_color: [f32; 4],
    pub diffuse: MaterialTexture,
    pub texgen: TexGen,
    pub fullbright: bool,
    pub bump: u8,
    pub shiny: u8,
    pub glow: f32,
    pub normal: MaterialTexture,
    pub specular: MaterialTexture,
    pub specular_color: [u8; 4],
    pub specular_exponent: u8,
    pub environment_intensity: u8,
    pub alpha_mode: LegacyAlphaMode,
    pub alpha_cutoff: f32,
}

impl Default for LegacyMaterial {
    fn default() -> Self {
        Self {
            diffuse_color: [1.0; 4],
            diffuse: MaterialTexture::default(),
            texgen: TexGen::Default,
            fullbright: false,
            bump: 0,
            shiny: 0,
            glow: 0.0,
            normal: MaterialTexture::default(),
            specular: MaterialTexture::default(),
            specular_color: [255; 4],
            specular_exponent: 51,
            environment_intensity: 0,
            alpha_mode: LegacyAlphaMode::Blend,
            alpha_cutoff: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TexGen {
    #[default]
    Default,
    Planar,
    Unsupported(u8),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Filter {
    Nearest,
    Linear,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MinFilter {
    Nearest,
    Linear,
    NearestMipmapNearest,
    LinearMipmapNearest,
    NearestMipmapLinear,
    LinearMipmapLinear,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Wrap {
    ClampToEdge,
    MirroredRepeat,
    #[default]
    Repeat,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Sampler {
    pub mag_filter: Option<Filter>,
    pub min_filter: Option<MinFilter>,
    pub wrap_s: Wrap,
    pub wrap_t: Wrap,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextureRef {
    Absent,
    Clear,
    Asset(AssetId),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialTexture {
    pub texture: TextureRef,
    pub transform: TextureTransform,
    pub tex_coord: u32,
    pub sampler: Sampler,
}

impl Default for MaterialTexture {
    fn default() -> Self {
        Self { texture: TextureRef::Absent, transform: TextureTransform::default(), tex_coord: 0, sampler: Sampler::default() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureTransform {
    pub offset: [f32; 2],
    pub scale: [f32; 2],
    pub rotation: f32,
}

impl Default for TextureTransform {
    fn default() -> Self {
        Self { offset: [0.0; 2], scale: [1.0; 2], rotation: 0.0 }
    }
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
    pub alpha_mode: AlphaMode,
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
            alpha_mode: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            double_sided: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MaterialDescription {
    Legacy(LegacyMaterial),
    Pbr(PbrMaterial),
}

impl MaterialDescription {
    pub fn base_color_texture(&self) -> &MaterialTexture {
        match self {
            Self::Legacy(material) => &material.diffuse,
            Self::Pbr(material) => &material.base_color,
        }
    }
}

pub(crate) fn legacy(bytes: &[u8]) -> Result<MaterialDescription, Failure> {
    let limits = Limits {
        max_input_bytes: 4 * 1024 * 1024,
        max_depth: 16,
        max_values: 4096,
        max_container_items: 512,
        max_string_bytes: 2 * 1024 * 1024,
        max_map_key_bytes: 128,
        max_binary_bytes: 2 * 1024 * 1024,
    };
    let llsd::Value::Map(fields) = binary::from_slice_with_limits(bytes, limits).map_err(|_| Failure::Malformed)? else {
        return Err(Failure::Malformed);
    };
    let byte = |key| match fields.get(key) {
        Some(llsd::Value::Integer(value)) => u8::try_from(*value).map_err(|_| Failure::Malformed),
        _ => Err(Failure::Malformed),
    };
    let alpha_mode = match byte("DiffuseAlphaMode")? {
        0 => LegacyAlphaMode::Opaque,
        1 => LegacyAlphaMode::Blend,
        2 => LegacyAlphaMode::Mask,
        3 => LegacyAlphaMode::Emissive,
        _ => return Err(Failure::Unsupported),
    };
    let fixed = |key| match fields.get(key) {
        Some(llsd::Value::Integer(value)) => Ok(*value as f32 / 10_000.0),
        _ => Err(Failure::Malformed),
    };
    let texture = |key| match fields.get(key) {
        Some(llsd::Value::UUID(id)) => Ok(texture_ref(*id)),
        _ => Err(Failure::Malformed),
    };
    let Some(llsd::Value::Array(color)) = fields.get("SpecColor") else { return Err(Failure::Malformed) };
    let mut specular_color = [0; 4];
    if color.len() != specular_color.len() {
        return Err(Failure::Malformed);
    }
    for (dest, value) in specular_color.iter_mut().zip(color) {
        let llsd::Value::Integer(value) = value else { return Err(Failure::Malformed) };
        *dest = u8::try_from(*value).map_err(|_| Failure::Malformed)?;
    }
    Ok(MaterialDescription::Legacy(LegacyMaterial {
        normal: MaterialTexture {
            texture: texture("NormMap")?,
            transform: TextureTransform {
                offset: [fixed("NormOffsetX")?, fixed("NormOffsetY")?],
                scale: [fixed("NormRepeatX")?, fixed("NormRepeatY")?],
                rotation: fixed("NormRotation")?,
            },
            ..MaterialTexture::default()
        },
        specular: MaterialTexture {
            texture: texture("SpecMap")?,
            transform: TextureTransform {
                offset: [fixed("SpecOffsetX")?, fixed("SpecOffsetY")?],
                scale: [fixed("SpecRepeatX")?, fixed("SpecRepeatY")?],
                rotation: fixed("SpecRotation")?,
            },
            ..MaterialTexture::default()
        },
        specular_color,
        specular_exponent: byte("SpecExp")?,
        environment_intensity: byte("EnvIntensity")?,
        alpha_mode,
        alpha_cutoff: f32::from(byte("AlphaMaskCutoff")?) / 255.0,
        ..LegacyMaterial::default()
    }))
}

fn texture_ref(id: UUID) -> TextureRef {
    if id.is_nil() || id == UUID::max() { TextureRef::Clear } else { TextureRef::Asset(AssetId::new(id)) }
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
    let id = UUID::parse_str(uri).map_err(|_| Failure::Unsupported)?;
    let mut result = MaterialTexture { texture: texture_ref(id), ..MaterialTexture::default() };
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

pub(crate) fn pbr(bytes: &[u8]) -> Result<MaterialDescription, Failure> {
    let root = super::material_json::parse(bytes)?;
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
            "OPAQUE" => AlphaMode::Opaque,
            "MASK" => AlphaMode::Mask,
            "BLEND" => AlphaMode::Blend,
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
#[path = "material_tests.rs"]
mod tests;
