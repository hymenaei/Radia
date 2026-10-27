use std::{mem::size_of, ops::Range};

use flate2::{Decompress, FlushDecompress, Status};
use llsd::{Limits, Value, binary};
use math::{Vec2, Vec3};
use utils::AssetId;

use crate::budget::{Reservation, Resident};

use super::{
    Face, LodGeometry, MAX_MESH_INDICES, MAX_MESH_INDICES_PER_FACE, MAX_MESH_VERTICES, MAX_MESH_VERTICES_PER_FACE, Mesh, MeshError, Vertex,
};

pub const MESH_HEADER_BYTES: usize = 4096;
pub const MAX_MESH_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_MESH_ASSET_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_MESH_BLOCK_BYTES: usize = MAX_MESH_ASSET_BYTES;
const MAX_MESH_FACES_PER_LOD: usize = 8;
const MAX_MESH_LLSD_OVERHEAD_BYTES: usize = 128 * 1024;
const MAX_MESH_PACKED_VERTEX_BYTES: usize = 6 + 6 + 4 + 12;
const MAX_MESH_DECODED_BLOCK_BYTES: usize = MAX_MESH_VERTICES_PER_FACE * MAX_MESH_FACES_PER_LOD * MAX_MESH_PACKED_VERTEX_BYTES
    + MAX_MESH_INDICES_PER_FACE * MAX_MESH_FACES_PER_LOD * 2
    + MAX_MESH_LLSD_OVERHEAD_BYTES;
const MAX_MESH_VERSION: u32 = 999;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MeshLod {
    Lowest,
    Low,
    Medium,
    High,
}

impl MeshLod {
    const fn index(self) -> usize {
        match self {
            Self::Lowest => 0,
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
        }
    }

    #[cfg(test)]
    const fn name(self) -> &'static str {
        match self {
            Self::Lowest => "lowest_lod",
            Self::Low => "low_lod",
            Self::Medium => "medium_lod",
            Self::High => "high_lod",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BlockRange {
    start: u64,
    size: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MeshHeader {
    asset_length: u64,
    header_size: u64,
    blocks: [Result<Option<BlockRange>, AssetError>; 6],
}

impl MeshHeader {
    pub fn asset_length(self) -> u64 {
        self.asset_length
    }

    pub fn lod(self, id: AssetId, lod: MeshLod) -> Result<MeshLodRequest, AssetError> {
        let block = self.blocks[lod.index()]?.ok_or(AssetError::MissingLod(lod))?;
        Ok(MeshLodRequest { id, lod, block, asset_length: self.asset_length })
    }

    pub fn header_size(self) -> u64 {
        self.header_size
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MeshLodRequest {
    id: AssetId,
    lod: MeshLod,
    block: BlockRange,
    asset_length: u64,
}

impl MeshLodRequest {
    pub fn asset_id(self) -> AssetId {
        self.id
    }

    pub fn lod(self) -> MeshLod {
        self.lod
    }

    pub fn range(self) -> Range<u64> {
        self.block.start..self.block.start + self.block.size
    }

    pub fn asset_length(self) -> u64 {
        self.asset_length
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssetError {
    MissingLod(MeshLod),
    MalformedBlock,
    UnsupportedVersion(u32),
    TooLarge,
    Cancelled,
}

enum PreparedSubmesh<'a> {
    NoGeometry,
    Empty,
    Geometry {
        position_bytes: &'a [u8],
        normal_bytes: Option<&'a [u8]>,
        uv_bytes: Option<&'a [u8]>,
        triangle_bytes: &'a [u8],
        triangle_count: usize,
        position_domain: (Vec3, Vec3),
        uv_domain: Option<(Vec2, Vec2)>,
    },
}

pub fn decode_header(bytes: &[u8], asset_length: u64) -> Result<MeshHeader, AssetError> {
    if asset_length == 0 || asset_length > MAX_MESH_ASSET_BYTES as u64 || bytes.len() > MAX_MESH_HEADER_BYTES {
        return Err(AssetError::TooLarge);
    }
    let value = binary::from_slice_prefix_with_limits::<Value>(bytes, header_limits()).map_err(|_| AssetError::MalformedBlock)?;
    let (Value::Map(fields), header_size) = value else { return Err(AssetError::MalformedBlock) };
    let header_size = u64::try_from(header_size).map_err(|_| AssetError::MalformedBlock)?;
    if header_size == 0 || header_size > bytes.len() as u64 || header_size > asset_length {
        return Err(AssetError::MalformedBlock);
    }
    let version = parse_version(fields.get("version").ok_or(AssetError::MalformedBlock)?)?;
    if version > MAX_MESH_VERSION {
        return Err(AssetError::UnsupportedVersion(version));
    }
    let mut blocks = [Ok(None); 6];
    for (index, name) in ["lowest_lod", "low_lod", "medium_lod", "high_lod", "physics_mesh", "skin"].into_iter().enumerate() {
        blocks[index] = parse_block_range(fields.get(name), header_size, asset_length);
    }
    Ok(MeshHeader { asset_length, header_size, blocks })
}

pub fn header_needs_more(bytes: &[u8]) -> bool {
    matches!(binary::from_slice_prefix_with_limits::<Value>(bytes, header_limits()), Err(llsd::Error::UnexpectedEof))
}

pub fn decode_lod(bytes: &[u8]) -> Result<LodGeometry, AssetError> {
    decode_lod_cancellable(bytes, || false)
}

pub fn decode_lod_cancellable(bytes: &[u8], cancelled: impl Fn() -> bool) -> Result<LodGeometry, AssetError> {
    decode_lod_inner(bytes, bytes.len(), None, cancelled)
}

pub(crate) fn decode_lod_cancellable_reserved(
    bytes: &[u8],
    input_capacity: usize,
    reservation: &mut Reservation,
    cancelled: impl Fn() -> bool,
) -> Result<LodGeometry, AssetError> {
    decode_lod_inner(bytes, input_capacity, Some(reservation), cancelled)
}

fn decode_lod_inner(
    bytes: &[u8],
    input_capacity: usize,
    mut reservation: Option<&mut Reservation>,
    cancelled: impl Fn() -> bool,
) -> Result<LodGeometry, AssetError> {
    if bytes.len() > MAX_MESH_BLOCK_BYTES {
        return Err(AssetError::TooLarge);
    }
    if input_capacity < bytes.len() || input_capacity > MAX_MESH_ASSET_BYTES {
        return Err(AssetError::TooLarge);
    }
    resize_reservation(&mut reservation, input_capacity)?;
    let decoded = decompress_bounded(bytes, input_capacity, &mut reservation, &cancelled)?;
    let before_parse = input_capacity
        .checked_add(decoded.capacity())
        .and_then(|size| size.checked_add(decoded.len()))
        .and_then(|size| size.checked_add(MAX_MESH_LLSD_OVERHEAD_BYTES))
        .ok_or(AssetError::TooLarge)?;
    resize_reservation(&mut reservation, before_parse)?;
    let limits = Limits {
        max_bytes: MAX_MESH_DECODED_BLOCK_BYTES,
        max_depth: 8,
        max_values: 512,
        max_container_items: 64,
        max_string_bytes: 64,
        max_map_key_bytes: 64,
        max_binary_bytes: MAX_MESH_DECODED_BLOCK_BYTES,
    };
    let value = binary::from_slice_with_limits::<Value>(&decoded, limits).map_err(|_| AssetError::MalformedBlock)?;
    drop(decoded);
    let (submeshes, total_vertices, total_indices) = prepare_submeshes(&value, &cancelled)?;
    let parsed_value_bytes = binary_value_bytes(&value)?.checked_add(MAX_MESH_LLSD_OVERHEAD_BYTES).ok_or(AssetError::TooLarge)?;
    if total_indices == 0 {
        let face_count = u8::try_from(submeshes.len()).map_err(|_| AssetError::TooLarge)?;
        drop(submeshes);
        drop(value);
        let empty_bytes = input_capacity
            .checked_add(size_of::<LodGeometry>())
            .and_then(|bytes| bytes.checked_add(size_of::<Resident>()))
            .ok_or(AssetError::TooLarge)?;
        resize_reservation(&mut reservation, empty_bytes)?;
        return Ok(LodGeometry::Empty { face_count });
    }
    let geometry_bytes = geometry_bytes(total_vertices, total_indices, submeshes.len())?;
    let with_geometry =
        input_capacity.checked_add(parsed_value_bytes).and_then(|size| size.checked_add(geometry_bytes)).ok_or(AssetError::TooLarge)?;
    resize_reservation(&mut reservation, with_geometry)?;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut faces = Vec::new();
    vertices.try_reserve_exact(total_vertices).map_err(|_| AssetError::TooLarge)?;
    indices.try_reserve_exact(total_indices).map_err(|_| AssetError::TooLarge)?;
    faces.try_reserve_exact(submeshes.len()).map_err(|_| AssetError::TooLarge)?;
    for (slot, submesh) in submeshes.into_iter().enumerate() {
        if cancelled() {
            return Err(AssetError::Cancelled);
        }
        let index_start = u32::try_from(indices.len()).map_err(|_| AssetError::TooLarge)?;
        let PreparedSubmesh::Geometry {
            position_bytes,
            normal_bytes,
            uv_bytes,
            triangle_bytes,
            triangle_count,
            position_domain,
            uv_domain,
        } = submesh
        else {
            faces.push(Face { slot: u8::try_from(slot).map_err(|_| AssetError::TooLarge)?, indices: index_start..index_start });
            continue;
        };
        let base = u32::try_from(vertices.len()).map_err(|_| AssetError::TooLarge)?;
        let position_chunks = position_bytes.as_chunks::<6>().0;
        let normal_chunks = normal_bytes.map(|bytes| bytes.as_chunks::<6>().0);
        let uv_chunks = uv_bytes.map(|bytes| bytes.as_chunks::<4>().0);
        for (vertex_index, encoded) in position_chunks.iter().enumerate() {
            if vertex_index.is_multiple_of(1024) && cancelled() {
                return Err(AssetError::Cancelled);
            }
            let position = unpack_position(encoded, position_domain);
            let normal = match normal_chunks {
                Some(chunks) => unpack_normal(chunks.get(vertex_index).ok_or(AssetError::MalformedBlock)?),
                None => Vec3::ZERO,
            };
            let uv = match uv_chunks {
                Some(chunks) => {
                    unpack_uv(chunks.get(vertex_index).ok_or(AssetError::MalformedBlock)?, uv_domain.ok_or(AssetError::MalformedBlock)?)
                }
                None => Vec2::ZERO,
            };
            vertices.push(Vertex { position, normal, uv });
        }
        let index_start = u32::try_from(indices.len()).map_err(|_| AssetError::TooLarge)?;
        let triangle_end = triangle_count.checked_mul(2).ok_or(AssetError::TooLarge)?;
        let triangle_bytes = triangle_bytes.get(..triangle_end).ok_or(AssetError::MalformedBlock)?;
        for (index_position, encoded) in triangle_bytes.as_chunks::<2>().0.iter().enumerate() {
            if index_position.is_multiple_of(1024) && cancelled() {
                return Err(AssetError::Cancelled);
            }
            let index = u32::from(u16::from_le_bytes([encoded[0], encoded[1]]));
            indices.push(base.checked_add(index).ok_or(AssetError::TooLarge)?);
        }
        if normal_chunks.is_none() {
            let start = usize::try_from(index_start).map_err(|_| AssetError::TooLarge)?;
            let base_index = usize::try_from(base).map_err(|_| AssetError::TooLarge)?;
            generate_normals(vertices.get_mut(base_index..).ok_or(AssetError::MalformedBlock)?, &indices[start..], base, &cancelled)?;
        }
        let index_end = u32::try_from(indices.len()).map_err(|_| AssetError::TooLarge)?;
        faces.push(Face { slot: u8::try_from(slot).map_err(|_| AssetError::TooLarge)?, indices: index_start..index_end });
    }
    drop(value);
    let after_decode = input_capacity.checked_add(geometry_bytes).ok_or(AssetError::TooLarge)?;
    resize_reservation(&mut reservation, after_decode)?;
    Mesh::from_parts(vertices, indices, faces).map(LodGeometry::Geometry).map_err(|error| match error {
        MeshError::TooComplex => AssetError::TooLarge,
        MeshError::InvalidParameters | MeshError::Degenerate => AssetError::MalformedBlock,
    })
}

fn resize_reservation(reservation: &mut Option<&mut Reservation>, bytes: usize) -> Result<(), AssetError> {
    if let Some(reservation) = reservation.as_deref_mut()
        && !reservation.resize(bytes)
    {
        return Err(AssetError::TooLarge);
    }
    Ok(())
}

fn geometry_bytes(vertices: usize, indices: usize, faces: usize) -> Result<usize, AssetError> {
    vertices
        .checked_mul(size_of::<Vertex>())
        .and_then(|bytes| bytes.checked_add(indices.checked_mul(size_of::<u32>())?))
        .and_then(|bytes| bytes.checked_add(faces.checked_mul(size_of::<Face>())?))
        .and_then(|bytes| bytes.checked_add(size_of::<Mesh>()))
        .and_then(|bytes| bytes.checked_add(size_of::<Resident>()))
        .ok_or(AssetError::TooLarge)
}

fn binary_value_bytes(value: &Value) -> Result<usize, AssetError> {
    let bytes = match value {
        Value::Binary(value) => value.len(),
        Value::String(value) => value.len(),
        Value::Uri(value) => value.as_str().len(),
        Value::Array(values) => {
            values.iter().try_fold(0_usize, |total, value| total.checked_add(binary_value_bytes(value)?).ok_or(AssetError::TooLarge))?
        }
        Value::Map(values) => values.iter().try_fold(0_usize, |total, (key, value)| {
            let value_bytes = binary_value_bytes(value)?;
            total.checked_add(key.len()).and_then(|total| total.checked_add(value_bytes)).ok_or(AssetError::TooLarge)
        })?,
        _ => 0,
    };
    Ok(bytes)
}

fn prepare_submeshes<'a>(value: &'a Value, cancelled: &impl Fn() -> bool) -> Result<(Vec<PreparedSubmesh<'a>>, usize, usize), AssetError> {
    let Value::Array(submeshes) = value else { return Err(AssetError::MalformedBlock) };
    if submeshes.len() > MAX_MESH_FACES_PER_LOD {
        return Err(AssetError::TooLarge);
    }
    let mut prepared = Vec::new();
    prepared.try_reserve_exact(submeshes.len()).map_err(|_| AssetError::TooLarge)?;
    let mut total_vertices = 0_usize;
    let mut total_indices = 0_usize;
    for submesh in submeshes {
        if cancelled() {
            return Err(AssetError::Cancelled);
        }
        let Value::Map(fields) = submesh else { return Err(AssetError::MalformedBlock) };
        if fields.contains_key("NoGeometry") {
            prepared.push(PreparedSubmesh::NoGeometry);
            continue;
        }
        let triangle_bytes = optional_binary_field(fields, "TriangleList")?;
        if !triangle_bytes.len().is_multiple_of(2) {
            return Err(AssetError::MalformedBlock);
        }
        let triangle_count = triangle_bytes.len() / 2 / 3 * 3;
        if triangle_count > MAX_MESH_INDICES_PER_FACE {
            return Err(AssetError::TooLarge);
        }
        if triangle_count == 0 {
            prepared.push(PreparedSubmesh::Empty);
            continue;
        }
        let position_bytes = binary_field(fields, "Position")?;
        if !position_bytes.len().is_multiple_of(6) {
            return Err(AssetError::MalformedBlock);
        }
        let vertex_count = position_bytes.len() / 6;
        if vertex_count == 0 {
            return Err(AssetError::MalformedBlock);
        }
        if vertex_count > MAX_MESH_VERTICES_PER_FACE {
            return Err(AssetError::TooLarge);
        }
        total_vertices = total_vertices.checked_add(vertex_count).ok_or(AssetError::TooLarge)?;
        total_indices = total_indices.checked_add(triangle_count).ok_or(AssetError::TooLarge)?;
        if total_vertices > MAX_MESH_VERTICES || total_indices > MAX_MESH_INDICES {
            return Err(AssetError::TooLarge);
        }
        for (index_position, encoded) in triangle_bytes[..triangle_count * 2].as_chunks::<2>().0.iter().enumerate() {
            if index_position.is_multiple_of(1024) && cancelled() {
                return Err(AssetError::Cancelled);
            }
            let index = usize::from(u16::from_le_bytes([encoded[0], encoded[1]]));
            if index >= vertex_count {
                return Err(AssetError::MalformedBlock);
            }
        }
        let normal_bytes = match fields.get("Normal") {
            None => None,
            Some(Value::Binary(value)) if value.is_empty() => None,
            Some(Value::Binary(value)) if value.len() == vertex_count.checked_mul(6).ok_or(AssetError::TooLarge)? => Some(value.as_slice()),
            Some(Value::Binary(_)) => return Err(AssetError::MalformedBlock),
            Some(_) => return Err(AssetError::MalformedBlock),
        };
        let uv_bytes = match optional_binary_field(fields, "TexCoord0")? {
            [] => None,
            uv_bytes if uv_bytes.len() == vertex_count.checked_mul(4).ok_or(AssetError::TooLarge)? => Some(uv_bytes),
            _ => return Err(AssetError::MalformedBlock),
        };
        let position_domain = fields.get("PositionDomain").map(domain3).transpose()?.unwrap_or((Vec3::splat(-0.5), Vec3::splat(0.5)));
        let uv_domain = uv_bytes.map(|_| domain2(fields.get("TexCoord0Domain").ok_or(AssetError::MalformedBlock)?)).transpose()?;
        prepared.push(PreparedSubmesh::Geometry {
            position_bytes,
            normal_bytes,
            uv_bytes,
            triangle_bytes,
            triangle_count,
            position_domain,
            uv_domain,
        });
    }
    Ok((prepared, total_vertices, total_indices))
}

fn parse_version(value: &Value) -> Result<u32, AssetError> {
    let version = match value {
        Value::Integer(value) if *value >= 0 => f64::from(*value),
        Value::Real(value) if value.is_finite() && *value >= 0.0 && value.fract() == 0.0 => *value,
        _ => return Err(AssetError::MalformedBlock),
    };
    if version > f64::from(MAX_MESH_VERSION) {
        return Ok(version as u32);
    }
    Ok(version as u32)
}

fn parse_block_range(value: Option<&Value>, header_size: u64, asset_length: u64) -> Result<Option<BlockRange>, AssetError> {
    let Some(value) = value else { return Ok(None) };
    let Value::Map(fields) = value else { return Err(AssetError::MalformedBlock) };
    let offset = fields.get("offset").and_then(Value::as_integer).ok_or(AssetError::MalformedBlock)?;
    let size = fields.get("size").and_then(Value::as_integer).ok_or(AssetError::MalformedBlock)?;
    if offset == -1 && size == 0 {
        return Ok(None);
    }
    if offset < 0 || size <= 0 {
        return Err(AssetError::MalformedBlock);
    }
    let size = u64::try_from(size).map_err(|_| AssetError::MalformedBlock)?;
    if size > MAX_MESH_BLOCK_BYTES as u64 {
        return Err(AssetError::TooLarge);
    }
    let start =
        header_size.checked_add(u64::try_from(offset).map_err(|_| AssetError::MalformedBlock)?).ok_or(AssetError::MalformedBlock)?;
    let end = start.checked_add(size).ok_or(AssetError::MalformedBlock)?;
    if end > asset_length {
        return Err(AssetError::MalformedBlock);
    }
    Ok(Some(BlockRange { start, size }))
}

fn header_limits() -> Limits {
    Limits {
        max_bytes: MAX_MESH_HEADER_BYTES,
        max_depth: 8,
        max_values: 128,
        max_container_items: 24,
        max_string_bytes: 64,
        max_map_key_bytes: 64,
        max_binary_bytes: 64,
    }
}

fn decompress_bounded(
    bytes: &[u8],
    input_capacity: usize,
    reservation: &mut Option<&mut Reservation>,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<u8>, AssetError> {
    let mut decoder = Decompress::new(true);
    let mut decoded = Vec::new();
    let mut chunk = [0; 16 * 1024];
    let mut input_position = 0;
    loop {
        if cancelled() {
            return Err(AssetError::Cancelled);
        }
        let input = bytes.get(input_position..).ok_or(AssetError::MalformedBlock)?;
        let before_input = decoder.total_in();
        let before_output = decoder.total_out();
        let status = decoder.decompress(input, &mut chunk, FlushDecompress::None).map_err(|_| AssetError::MalformedBlock)?;
        let consumed = usize::try_from(decoder.total_in() - before_input).map_err(|_| AssetError::MalformedBlock)?;
        let produced = usize::try_from(decoder.total_out() - before_output).map_err(|_| AssetError::MalformedBlock)?;
        input_position = input_position.checked_add(consumed).ok_or(AssetError::MalformedBlock)?;
        let end = decoded.len().checked_add(produced).ok_or(AssetError::TooLarge)?;
        if end > MAX_MESH_DECODED_BLOCK_BYTES {
            return Err(AssetError::TooLarge);
        }
        let required = input_capacity.checked_add(end).ok_or(AssetError::TooLarge)?;
        resize_reservation(reservation, required)?;
        decoded.try_reserve_exact(produced).map_err(|_| AssetError::TooLarge)?;
        let allocated = input_capacity.checked_add(decoded.capacity()).ok_or(AssetError::TooLarge)?;
        resize_reservation(reservation, allocated)?;
        decoded.extend_from_slice(&chunk[..produced]);
        if status == Status::StreamEnd {
            return (input_position == bytes.len()).then_some(decoded).ok_or(AssetError::MalformedBlock);
        }
        if consumed == 0 && produced == 0 {
            return Err(AssetError::MalformedBlock);
        }
    }
}

fn binary_field<'a>(fields: &'a llsd::Map, name: &str) -> Result<&'a [u8], AssetError> {
    fields.get(name).and_then(Value::as_binary).ok_or(AssetError::MalformedBlock)
}

fn optional_binary_field<'a>(fields: &'a llsd::Map, name: &str) -> Result<&'a [u8], AssetError> {
    match fields.get(name) {
        None => Ok(&[]),
        Some(Value::Binary(value)) => Ok(value.as_slice()),
        Some(_) => Err(AssetError::MalformedBlock),
    }
}

fn domain3(value: &Value) -> Result<(Vec3, Vec3), AssetError> {
    let Value::Map(fields) = value else { return Err(AssetError::MalformedBlock) };
    let min = vector3(fields.get("Min").ok_or(AssetError::MalformedBlock)?)?;
    let max = vector3(fields.get("Max").ok_or(AssetError::MalformedBlock)?)?;
    valid_domain(min, max).then_some((min, max)).ok_or(AssetError::MalformedBlock)
}

fn domain2(value: &Value) -> Result<(Vec2, Vec2), AssetError> {
    let Value::Map(fields) = value else { return Err(AssetError::MalformedBlock) };
    let min = vector2(fields.get("Min").ok_or(AssetError::MalformedBlock)?)?;
    let max = vector2(fields.get("Max").ok_or(AssetError::MalformedBlock)?)?;
    valid_domain(min, max).then_some((min, max)).ok_or(AssetError::MalformedBlock)
}

fn valid_domain<T: Domain>(min: T, max: T) -> bool {
    min.is_finite() && max.is_finite() && min.less_equal(max)
}

trait Domain: Copy {
    fn is_finite(self) -> bool;
    fn less_equal(self, other: Self) -> bool;
}

impl Domain for Vec2 {
    fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    fn less_equal(self, other: Self) -> bool {
        self.cmple(other).all()
    }
}

impl Domain for Vec3 {
    fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    fn less_equal(self, other: Self) -> bool {
        self.cmple(other).all()
    }
}

fn vector2(value: &Value) -> Result<Vec2, AssetError> {
    let values = value.as_array().ok_or(AssetError::MalformedBlock)?;
    if values.len() != 2 {
        return Err(AssetError::MalformedBlock);
    }
    Ok(Vec2::new(
        values[0].as_finite_f32().ok_or(AssetError::MalformedBlock)?,
        values[1].as_finite_f32().ok_or(AssetError::MalformedBlock)?,
    ))
}

fn vector3(value: &Value) -> Result<Vec3, AssetError> {
    let values = value.as_array().ok_or(AssetError::MalformedBlock)?;
    if values.len() != 3 {
        return Err(AssetError::MalformedBlock);
    }
    Ok(Vec3::new(
        values[0].as_finite_f32().ok_or(AssetError::MalformedBlock)?,
        values[1].as_finite_f32().ok_or(AssetError::MalformedBlock)?,
        values[2].as_finite_f32().ok_or(AssetError::MalformedBlock)?,
    ))
}

fn unpack_position(encoded: &[u8; 6], (min, max): (Vec3, Vec3)) -> Vec3 {
    let components = [
        u16::from_le_bytes([encoded[0], encoded[1]]),
        u16::from_le_bytes([encoded[2], encoded[3]]),
        u16::from_le_bytes([encoded[4], encoded[5]]),
    ];
    let normalized = Vec3::new(
        f32::from(components[0]) / f32::from(u16::MAX),
        f32::from(components[1]) / f32::from(u16::MAX),
        f32::from(components[2]) / f32::from(u16::MAX),
    );
    min + normalized * (max - min)
}

fn unpack_uv(encoded: &[u8; 4], (min, max): (Vec2, Vec2)) -> Vec2 {
    let components = [u16::from_le_bytes([encoded[0], encoded[1]]), u16::from_le_bytes([encoded[2], encoded[3]])];
    let normalized = Vec2::new(f32::from(components[0]) / f32::from(u16::MAX), f32::from(components[1]) / f32::from(u16::MAX));
    min + normalized * (max - min)
}

fn unpack_normal(encoded: &[u8; 6]) -> Vec3 {
    let components = [
        u16::from_le_bytes([encoded[0], encoded[1]]),
        u16::from_le_bytes([encoded[2], encoded[3]]),
        u16::from_le_bytes([encoded[4], encoded[5]]),
    ];
    Vec3::new(
        f32::from(components[0]) / f32::from(u16::MAX) * 2.0 - 1.0,
        f32::from(components[1]) / f32::from(u16::MAX) * 2.0 - 1.0,
        f32::from(components[2]) / f32::from(u16::MAX) * 2.0 - 1.0,
    )
}

fn generate_normals(vertices: &mut [Vertex], indices: &[u32], base: u32, cancelled: &impl Fn() -> bool) -> Result<(), AssetError> {
    for (position, triangle) in indices.as_chunks::<3>().0.iter().enumerate() {
        if position.is_multiple_of(1024) && cancelled() {
            return Err(AssetError::Cancelled);
        }
        let [a, b, c] = *triangle;
        let a = usize::try_from(a.checked_sub(base).ok_or(AssetError::MalformedBlock)?).map_err(|_| AssetError::MalformedBlock)?;
        let b = usize::try_from(b.checked_sub(base).ok_or(AssetError::MalformedBlock)?).map_err(|_| AssetError::MalformedBlock)?;
        let c = usize::try_from(c.checked_sub(base).ok_or(AssetError::MalformedBlock)?).map_err(|_| AssetError::MalformedBlock)?;
        let a_position = vertices.get(a).ok_or(AssetError::MalformedBlock)?.position;
        let b_position = vertices.get(b).ok_or(AssetError::MalformedBlock)?.position;
        let c_position = vertices.get(c).ok_or(AssetError::MalformedBlock)?.position;
        let normal = (b_position - a_position).cross(c_position - a_position);
        vertices.get_mut(a).ok_or(AssetError::MalformedBlock)?.normal += normal;
        vertices.get_mut(b).ok_or(AssetError::MalformedBlock)?.normal += normal;
        vertices.get_mut(c).ok_or(AssetError::MalformedBlock)?.normal += normal;
    }
    for vertex in vertices {
        if cancelled() {
            return Err(AssetError::Cancelled);
        }
        if !vertex.normal.is_finite() {
            return Err(AssetError::MalformedBlock);
        }
        let scale = vertex.normal.abs().max_element();
        vertex.normal = if scale == 0.0 {
            Vec3::Z
        } else {
            let scaled = vertex.normal / scale;
            scaled / scaled.length()
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{assert_matches, io::Write};

    use flate2::{Compression, write::ZlibEncoder};
    use proptest::prelude::*;
    use utils::AssetId;
    use uuid::Uuid;

    use super::*;

    fn sized(tag: u8, bytes: &[u8]) -> Vec<u8> {
        let mut encoded = vec![tag];
        encoded.extend_from_slice(&u32::try_from(bytes.len()).unwrap().to_be_bytes());
        encoded.extend_from_slice(bytes);
        encoded
    }

    fn integer(value: i32) -> Vec<u8> {
        let mut encoded = vec![b'i'];
        encoded.extend_from_slice(&value.to_be_bytes());
        encoded
    }

    fn real(value: f64) -> Vec<u8> {
        let mut encoded = vec![b'r'];
        encoded.extend_from_slice(&value.to_be_bytes());
        encoded
    }

    fn map(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut encoded = vec![b'{'];
        encoded.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_be_bytes());
        for (key, value) in entries {
            encoded.extend_from_slice(&sized(b'k', key.as_bytes()));
            encoded.extend_from_slice(value);
        }
        encoded.push(b'}');
        encoded
    }

    fn array(items: &[Vec<u8>]) -> Vec<u8> {
        let mut encoded = vec![b'['];
        encoded.extend_from_slice(&u32::try_from(items.len()).unwrap().to_be_bytes());
        for item in items {
            encoded.extend_from_slice(item);
        }
        encoded.push(b']');
        encoded
    }

    fn binary_document(body: Vec<u8>) -> Vec<u8> {
        let mut encoded = b"<?llsd/binary?>\n".to_vec();
        encoded.extend(body);
        encoded
    }

    fn compressed(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn vector(values: &[f64]) -> Vec<u8> {
        array(&values.iter().copied().map(real).collect::<Vec<_>>())
    }

    fn domain(min: &[f64], max: &[f64]) -> Vec<u8> {
        map(&[("Min", vector(min)), ("Max", vector(max))])
    }

    fn submesh(with_normals: bool, no_geometry: bool) -> Vec<u8> {
        if no_geometry {
            return map(&[("NoGeometry", vec![b'1'])]);
        }
        let positions = [0x00, 0x00, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff];
        let uvs = [0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let triangles = [0x00, 0x00, 0x01, 0x00, 0x02, 0x00];
        let mut fields = vec![
            ("Position", sized(b'b', &positions)),
            ("PositionDomain", domain(&[-1.0, -2.0, -3.0], &[1.0, 2.0, 3.0])),
            ("TexCoord0", sized(b'b', &uvs)),
            ("TexCoord0Domain", domain(&[-0.5, 1.25], &[1.5, 2.25])),
            ("TriangleList", sized(b'b', &triangles)),
        ];
        if with_normals {
            fields.push(("Normal", sized(b'b', &[0xff, 0xff, 0xff, 0x7f, 0x00, 0x00].repeat(3))));
        }
        map(&fields)
    }

    fn submesh_without_domains_or_uvs() -> Vec<u8> {
        let positions = [0x00, 0x00, 0xff, 0xff, 0x00, 0x00, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff];
        let triangles = [0x00, 0x00, 0x01, 0x00, 0x02, 0x00];
        map(&[("Position", sized(b'b', &positions)), ("TriangleList", sized(b'b', &triangles))])
    }

    fn submesh_with_partial_triangle() -> Vec<u8> {
        let positions = [0x00, 0x00, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff];
        let uvs = [0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let triangles = [0x00, 0x00, 0x01, 0x00, 0x02, 0x00, 0x01, 0x00];
        map(&[
            ("Position", sized(b'b', &positions)),
            ("PositionDomain", domain(&[-1.0, -1.0, -1.0], &[1.0, 1.0, 1.0])),
            ("TexCoord0", sized(b'b', &uvs)),
            ("TexCoord0Domain", domain(&[0.0, 0.0], &[1.0, 1.0])),
            ("TriangleList", sized(b'b', &triangles)),
        ])
    }

    fn empty_submesh() -> Vec<u8> {
        map(&[("TriangleList", sized(b'b', &[]))])
    }

    fn dense_submesh(vertex_count: usize, index_count: usize, seed: u32) -> Vec<u8> {
        let mut random = seed;
        let mut positions = Vec::with_capacity(vertex_count * 6);
        let mut normals = Vec::with_capacity(vertex_count * 6);
        let mut uvs = Vec::with_capacity(vertex_count * 4);
        let mut weights = Vec::with_capacity(vertex_count * 12);
        let mut triangles = Vec::with_capacity(index_count * 2);
        for vertex in 0..vertex_count {
            let position =
                if vertex == 0 { [0, u16::MAX, 32_768] } else { [next_u16(&mut random), next_u16(&mut random), next_u16(&mut random)] };
            let normal =
                if vertex == 0 { [u16::MAX, 32_768, 0] } else { [next_u16(&mut random), next_u16(&mut random), next_u16(&mut random)] };
            let uv = if vertex == 0 { [0, u16::MAX] } else { [next_u16(&mut random), next_u16(&mut random)] };
            for value in position {
                positions.extend_from_slice(&value.to_le_bytes());
            }
            for value in normal {
                normals.extend_from_slice(&value.to_le_bytes());
            }
            for value in uv {
                uvs.extend_from_slice(&value.to_le_bytes());
            }
            for joint in 0..4_u8 {
                weights.push(joint);
                let weight = if joint == 3 { 16_383_u16 } else { 16_384_u16 };
                weights.extend_from_slice(&weight.to_le_bytes());
            }
        }
        for index in 0..index_count {
            let vertex = u16::try_from(index % vertex_count).unwrap();
            triangles.extend_from_slice(&vertex.to_le_bytes());
        }
        map(&[
            ("Position", sized(b'b', &positions)),
            ("PositionDomain", domain(&[-1.0, -1.0, -1.0], &[1.0, 1.0, 1.0])),
            ("Normal", sized(b'b', &normals)),
            ("TexCoord0", sized(b'b', &uvs)),
            ("TexCoord0Domain", domain(&[0.0, 0.0], &[1.0, 1.0])),
            ("Weights", sized(b'b', &weights)),
            ("TriangleList", sized(b'b', &triangles)),
        ])
    }

    fn next_u16(state: &mut u32) -> u16 {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        *state as u16
    }

    fn block(faces: &[Vec<u8>]) -> Vec<u8> {
        compressed(&binary_document(array(faces)))
    }

    fn header_with_blocks_version(blocks: &[(MeshLod, &[u8])], total_len_extra: usize, version: i32) -> (Vec<u8>, u64) {
        let mut offset = 0_i32;
        let mut ranges = Vec::new();
        for (lod, bytes) in blocks {
            let size = i32::try_from(bytes.len()).unwrap();
            ranges.push((*lod, offset, size));
            offset = offset.checked_add(size).unwrap();
        }
        let header = mesh_header(version, &ranges);
        let total_len = header.len() + usize::try_from(offset).unwrap() + total_len_extra;
        (header, u64::try_from(total_len).unwrap())
    }

    fn header_with_blocks(blocks: &[(MeshLod, &[u8])], total_len_extra: usize) -> (Vec<u8>, u64) {
        header_with_blocks_version(blocks, total_len_extra, 1)
    }

    fn block_entry(offset: i32, size: i32) -> Vec<u8> {
        map(&[("offset", integer(offset)), ("size", integer(size))])
    }

    fn mesh_header(version: i32, blocks: &[(MeshLod, i32, i32)]) -> Vec<u8> {
        let mut fields = vec![
            ("version", integer(version)),
            ("lowest_lod", block_entry(-1, 0)),
            ("low_lod", block_entry(-1, 0)),
            ("medium_lod", block_entry(-1, 0)),
            ("high_lod", block_entry(-1, 0)),
            ("physics_mesh", block_entry(-1, 0)),
            ("skin", block_entry(-1, 0)),
        ];
        for (lod, offset, size) in blocks {
            if let Some((_, entry)) = fields.iter_mut().find(|(name, _)| *name == lod.name()) {
                *entry = block_entry(*offset, *size);
            }
        }
        binary_document(map(&fields))
    }

    fn id() -> AssetId {
        AssetId::new(Uuid::from_u128(1))
    }

    #[test]
    fn decodes_lod_blocks_and_preserves_geometry() {
        let encoded = block(&[submesh(true, false), submesh(false, true), submesh(false, false)]);
        let LodGeometry::Geometry(mesh) = decode_lod(&encoded).unwrap() else { panic!("geometry LOD") };

        assert_eq!(mesh.faces(), &[Face { slot: 0, indices: 0..3 }, Face { slot: 1, indices: 3..3 }, Face { slot: 2, indices: 3..6 }]);
        assert_eq!(mesh.vertices()[0].position, Vec3::new(-1.0, 2.0, -3.0));
        assert_eq!(mesh.vertices()[1].position, Vec3::new(-1.0, -2.0, -3.0));
        assert_eq!(mesh.vertices()[0].uv, Vec2::new(-0.5, 2.25));
        assert_eq!(mesh.vertices()[1].uv, Vec2::new(1.5, 1.25));
        assert_eq!(mesh.vertices()[0].normal.x, 1.0);
        assert!(mesh.vertices()[0].normal.y.abs() < 0.00002);
        assert_eq!(mesh.vertices()[0].normal.z, -1.0);
        assert_eq!(mesh.vertices()[3].normal, Vec3::NEG_X);
    }

    #[test]
    fn small_triangle_keeps_normal() {
        let positions = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff];
        let triangles = [0x00, 0x00, 0x01, 0x00, 0x02, 0x00];
        let face = map(&[
            ("Position", sized(b'b', &positions)),
            ("PositionDomain", domain(&[0.0, 0.0, 0.0], &[0.01, 0.0, 0.01])),
            ("TriangleList", sized(b'b', &triangles)),
        ]);
        let LodGeometry::Geometry(mesh) = decode_lod(&block(&[face])).unwrap() else { panic!("geometry LOD") };

        assert!(mesh.vertices().iter().all(|vertex| vertex.normal == Vec3::NEG_Y));
    }

    #[test]
    fn header_ranges_are_relative_to_the_serialized_header() {
        for lod in [MeshLod::Lowest, MeshLod::Low, MeshLod::Medium, MeshLod::High] {
            let encoded = block(&[submesh(false, false)]);
            let (header_bytes, asset_length) = header_with_blocks(&[(lod, &encoded)], 0);
            let header = decode_header(&header_bytes, asset_length).unwrap();
            let request = header.lod(id(), lod).unwrap();
            let range = request.range();
            assert_eq!(range.start, header.header_size());
            assert_eq!(range.end - range.start, encoded.len() as u64);
            assert_eq!(range.end, asset_length);
        }
    }

    #[test]
    fn missing_lod_is_distinct_from_an_empty_lod() {
        let (header_bytes, length) = header_with_blocks(&[], 0);
        assert_eq!(decode_header(&header_bytes, length).unwrap().lod(id(), MeshLod::Low), Err(AssetError::MissingLod(MeshLod::Low)));
        assert_matches!(decode_lod(&block(&[])), Ok(LodGeometry::Empty { face_count: 0 }));
        assert_matches!(decode_lod(&block(&[submesh(false, true)])), Ok(LodGeometry::Empty { face_count: 1 }));
        assert_matches!(decode_lod(&block(&[empty_submesh()])), Ok(LodGeometry::Empty { face_count: 1 }));
    }

    #[test]
    fn incomplete_trailing_triangles_are_dropped() {
        let LodGeometry::Geometry(mesh) = decode_lod(&block(&[submesh_with_partial_triangle()])).unwrap() else { panic!("geometry LOD") };

        assert_eq!(mesh.faces(), &[Face { slot: 0, indices: 0..3 }]);
        assert_eq!(mesh.indices(), &[0, 1, 2]);
    }

    #[test]
    fn absent_position_domain_and_uvs_use_the_format_defaults() {
        let LodGeometry::Geometry(mesh) = decode_lod(&block(&[submesh_without_domains_or_uvs()])).unwrap() else { panic!("geometry LOD") };

        assert_eq!(mesh.vertices()[0].position, Vec3::new(-0.5, 0.5, -0.5));
        assert_eq!(mesh.vertices()[1].position, Vec3::new(0.5, -0.5, -0.5));
        assert!(mesh.vertices().iter().all(|vertex| vertex.uv == Vec2::ZERO));
    }

    #[test]
    fn lod_accepts_the_maximum_face_count_and_keeps_empty_slots() {
        let faces = [
            submesh(false, true),
            submesh(false, false),
            submesh(false, true),
            submesh(false, false),
            submesh(false, false),
            submesh(false, true),
            submesh(false, false),
            submesh(false, false),
        ];
        let LodGeometry::Geometry(mesh) = decode_lod(&block(&faces)).unwrap() else { panic!("geometry LOD") };

        assert_eq!(mesh.faces().len(), MAX_MESH_FACES_PER_LOD);
        assert_eq!(mesh.faces()[0], Face { slot: 0, indices: 0..0 });
        assert_eq!(mesh.faces()[1], Face { slot: 1, indices: 0..3 });
        assert_eq!(mesh.faces()[2], Face { slot: 2, indices: 3..3 });
        assert_eq!(mesh.faces()[7], Face { slot: 7, indices: 12..15 });
    }

    #[test]
    fn lod_decodes_format_maximum_geometry() {
        let faces = (0..MAX_MESH_FACES_PER_LOD)
            .map(|face| dense_submesh(MAX_MESH_VERTICES_PER_FACE, MAX_MESH_INDICES_PER_FACE, u32::try_from(face + 1).unwrap()))
            .collect::<Vec<_>>();
        let encoded = block(&faces);
        drop(faces);

        assert!(encoded.len() > 2 * 1024 * 1024);
        assert!(encoded.len() <= MAX_MESH_BLOCK_BYTES);
        let budget = crate::budget::Budget::default();
        let mut reservation = budget.reserve(encoded.capacity()).unwrap();
        let LodGeometry::Geometry(mesh) =
            decode_lod_cancellable_reserved(&encoded, encoded.capacity(), &mut reservation, || false).unwrap()
        else {
            panic!("geometry LOD")
        };

        assert!(budget.stats().high_water_bytes < 64 * 1024 * 1024);
        assert_eq!(mesh.vertices().len(), MAX_MESH_VERTICES);
        assert_eq!(mesh.indices().len(), MAX_MESH_INDICES);
        assert_eq!(mesh.faces().len(), MAX_MESH_FACES_PER_LOD);
        assert_eq!(mesh.vertices()[0].position.x, -1.0);
        assert_eq!(mesh.vertices()[0].position.y, 1.0);
        assert_eq!(mesh.vertices()[0].uv, Vec2::new(0.0, 1.0));
        assert_eq!(mesh.vertices()[0].normal.x, 1.0);
        assert_eq!(mesh.vertices()[0].normal.z, -1.0);
        assert_eq!(mesh.faces()[7].indices, (MAX_MESH_INDICES - MAX_MESH_INDICES_PER_FACE) as u32..MAX_MESH_INDICES as u32);
    }

    #[test]
    fn lod_rejects_counts_over_face_limits() {
        let too_many_vertices = block(&[dense_submesh(MAX_MESH_VERTICES_PER_FACE + 1, 3, 1)]);
        assert_eq!(decode_lod(&too_many_vertices).err(), Some(AssetError::TooLarge));

        let too_many_indices = block(&[dense_submesh(3, MAX_MESH_INDICES_PER_FACE + 3, 1)]);
        assert_eq!(decode_lod(&too_many_indices).err(), Some(AssetError::TooLarge));
    }

    #[test]
    fn header_rejects_unsupported_versions_and_out_of_bounds_blocks() {
        let encoded = block(&[submesh(false, false)]);
        let (_, length) = header_with_blocks(&[(MeshLod::Low, &encoded)], 0);
        let unsupported = mesh_header(1000, &[(MeshLod::Low, 0, i32::try_from(encoded.len()).unwrap())]);
        assert_eq!(decode_header(&unsupported, length), Err(AssetError::UnsupportedVersion(1000)));
        let invalid_low = mesh_header(1, &[(MeshLod::Low, i32::MAX, 1), (MeshLod::High, 0, i32::try_from(encoded.len()).unwrap())]);
        let header = decode_header(&invalid_low, length).unwrap();
        assert_eq!(header.lod(id(), MeshLod::Low), Err(AssetError::MalformedBlock));
        assert_eq!(header.lod(id(), MeshLod::High).unwrap().range().start, header.header_size());
    }

    #[test]
    fn truncated_and_malformed_blocks_fail_without_panicking() {
        let encoded = block(&[submesh(false, false)]);
        assert_eq!(decode_lod(&encoded[..encoded.len() - 1]).err(), Some(AssetError::MalformedBlock));
        let mut trailing = encoded;
        trailing.push(0);
        assert_eq!(decode_lod(&trailing).err(), Some(AssetError::MalformedBlock));
        assert_eq!(decode_lod(b"not zlib").err(), Some(AssetError::MalformedBlock));
    }

    #[test]
    fn decompression_stops_at_the_output_limit() {
        let bytes = compressed(&vec![0; MAX_MESH_DECODED_BLOCK_BYTES + 1]);
        let mut reservation = None;
        assert_eq!(decompress_bounded(&bytes, bytes.len(), &mut reservation, &|| false), Err(AssetError::TooLarge));
    }

    #[test]
    fn geometry_reservation_failure_is_too_large() {
        let encoded = block(&[dense_submesh(20_000, 60_000, 1)]);
        let mut no_reservation = None;
        let decoded = decompress_bounded(&encoded, encoded.capacity(), &mut no_reservation, &|| false).unwrap();
        let value = binary::from_slice_with_limits::<Value>(
            &decoded,
            Limits {
                max_bytes: MAX_MESH_DECODED_BLOCK_BYTES,
                max_depth: 8,
                max_values: 512,
                max_container_items: 64,
                max_string_bytes: 64,
                max_map_key_bytes: 64,
                max_binary_bytes: MAX_MESH_DECODED_BLOCK_BYTES,
            },
        )
        .unwrap();
        let (submeshes, total_vertices, total_indices) = prepare_submeshes(&value, &|| false).unwrap();
        let parsed_value_bytes = binary_value_bytes(&value).unwrap() + MAX_MESH_LLSD_OVERHEAD_BYTES;
        let parse_peak = encoded.capacity() + decoded.capacity() + decoded.len() + MAX_MESH_LLSD_OVERHEAD_BYTES;
        let geometry_peak =
            encoded.capacity() + parsed_value_bytes + geometry_bytes(total_vertices, total_indices, submeshes.len()).unwrap();
        assert!(geometry_peak > parse_peak);
        drop(submeshes);
        drop(value);
        drop(decoded);

        let budget = crate::budget::Budget::default();
        let _blocker = budget.reserve(crate::budget::ASSET_WORK_BYTES - parse_peak).unwrap();
        let mut reservation = budget.reserve(encoded.capacity()).unwrap();
        assert_matches!(
            decode_lod_cancellable_reserved(&encoded, encoded.capacity(), &mut reservation, || false),
            Err(AssetError::TooLarge)
        );
    }

    proptest! {
        #[test]
        fn arbitrary_header_bytes_are_bounded_and_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..=MESH_HEADER_BYTES)) {
            let result = decode_header(&bytes, MAX_MESH_ASSET_BYTES as u64);
            if let Ok(header) = result {
                prop_assert!(header.asset_length() <= MAX_MESH_ASSET_BYTES as u64);
                for lod in [MeshLod::Lowest, MeshLod::Low, MeshLod::Medium, MeshLod::High] {
                    if let Ok(request) = header.lod(id(), lod) {
                        let range = request.range();
                        prop_assert!(range.end <= header.asset_length());
                        prop_assert!(range.end - range.start <= MAX_MESH_BLOCK_BYTES as u64);
                    }
                }
            }
        }

        #[test]
        fn arbitrary_block_bytes_are_bounded_and_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..=4096)) {
            let result = decode_lod(&bytes);
            if let Ok(geometry) = result {
                match geometry {
                    LodGeometry::Geometry(mesh) => {
                        prop_assert!(mesh.owned_bytes() <= geometry_bytes(MAX_MESH_VERTICES, MAX_MESH_INDICES, MAX_MESH_FACES_PER_LOD).unwrap());
                        prop_assert!(mesh.vertices().len() <= MAX_MESH_VERTICES);
                        prop_assert!(mesh.indices().len() <= MAX_MESH_INDICES);
                        prop_assert!(mesh.faces().len() <= MAX_MESH_FACES_PER_LOD);
                    }
                    LodGeometry::Empty { face_count } => prop_assert!(usize::from(face_count) <= MAX_MESH_FACES_PER_LOD),
                }
            }
        }
    }
}
