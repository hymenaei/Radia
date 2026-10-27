use std::{
    fmt::{self, Debug, Formatter},
    ops::Deref,
    sync::Arc,
};

use crate::{
    Blob, EnvironmentDescription, Failure, Image, Kind, MaterialDescription, Request, Resource, TextureHandle, TextureSize, WeakTexture,
    budget::{Reservation, Resident},
    envelope, environment, material, mesh, texture,
};

#[derive(Clone)]
pub struct Environment(Arc<EnvironmentInner>);

impl Deref for Environment {
    type Target = EnvironmentDescription;
    fn deref(&self) -> &Self::Target {
        &self.0.description
    }
}

impl Debug for Environment {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.0.description.fmt(f)
    }
}

struct EnvironmentInner {
    description: EnvironmentDescription,
    _reservation: Resident,
}

#[derive(Clone)]
pub struct Material(Arc<MaterialInner>);

impl Deref for Material {
    type Target = MaterialDescription;
    fn deref(&self) -> &Self::Target {
        &self.0.description
    }
}

impl Debug for Material {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.0.description.fmt(f)
    }
}

struct MaterialInner {
    description: MaterialDescription,
    _reservation: Resident,
}

#[derive(Clone)]
pub struct MeshHeaderAsset(Arc<MeshHeaderInner>);

impl Deref for MeshHeaderAsset {
    type Target = mesh::MeshHeader;
    fn deref(&self) -> &Self::Target {
        &self.0.header
    }
}

impl Debug for MeshHeaderAsset {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.0.header.fmt(f)
    }
}

impl MeshHeaderAsset {
    pub fn lod(&self, id: utils::AssetId, lod: mesh::MeshLod) -> Result<mesh::MeshLodRequest, Failure> {
        self.0.header.lod(id, lod).map_err(mesh_failure)
    }
}

struct MeshHeaderInner {
    header: mesh::MeshHeader,
    _reservation: Resident,
}

#[derive(Clone)]
pub struct MeshAsset(Arc<MeshInner>);

impl MeshAsset {
    pub fn lod(&self) -> mesh::MeshLod {
        self.0.lod
    }

    pub fn geometry(&self) -> &mesh::LodGeometry {
        &self.0.geometry
    }
}

impl Debug for MeshAsset {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("MeshAsset").field("lod", &self.0.lod).field("geometry", &self.0.geometry).finish()
    }
}

struct MeshInner {
    lod: mesh::MeshLod,
    geometry: mesh::LodGeometry,
    _reservation: Resident,
}

#[derive(Clone, Debug)]
pub enum Content {
    Texture(TextureHandle),
    Material(Material),
    Environment(Environment),
    MeshHeader(MeshHeaderAsset),
    Mesh(MeshAsset),
}

impl Content {
    pub(crate) fn downgrade(&self) -> StoredContent {
        match self {
            Self::Texture(texture) => StoredContent::Texture(texture.downgrade()),
            Self::Material(material) => StoredContent::Material(material.clone()),
            Self::Environment(environment) => StoredContent::Environment(environment.clone()),
            Self::MeshHeader(header) => StoredContent::MeshHeader(header.clone()),
            Self::Mesh(mesh) => StoredContent::Mesh(mesh.clone()),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum StoredContent {
    Texture(WeakTexture),
    Material(Material),
    Environment(Environment),
    MeshHeader(MeshHeaderAsset),
    Mesh(MeshAsset),
}

impl StoredContent {
    pub(crate) fn upgrade(&self) -> Option<Content> {
        match self {
            Self::Texture(texture) => texture.upgrade().map(Content::Texture),
            Self::Material(material) => Some(Content::Material(material.clone())),
            Self::Environment(environment) => Some(Content::Environment(environment.clone())),
            Self::MeshHeader(header) => Some(Content::MeshHeader(header.clone())),
            Self::Mesh(mesh) => Some(Content::Mesh(mesh.clone())),
        }
    }
}

pub(crate) enum Finished {
    Texture(Image),
    Other(Content),
}

enum Data {
    Texture(texture::Decoded),
    Material(MaterialDescription),
    Environment(EnvironmentDescription),
    MeshHeader(mesh::MeshHeader),
    Mesh { lod: mesh::MeshLod, geometry: mesh::LodGeometry },
}

pub(crate) struct Prepared {
    kind: Kind,
    data: Data,
    source: Option<Vec<u8>>,
}

impl Prepared {
    pub(crate) fn finish(self, mut reservation: Reservation, persist: bool) -> Result<(Finished, Option<Blob>, Option<Blob>), Failure> {
        let (source, persisted) = match self.source {
            Some(bytes) if self.kind == Kind::Texture || persist => {
                let blob = reservation.split(bytes.capacity()).and_then(|part| part.finish(bytes)).map_err(|_| Failure::TooLarge)?;
                let source = (self.kind == Kind::Texture).then(|| blob.clone());
                let persisted = persist.then_some(blob);
                (source, persisted)
            }
            _ => (None, None),
        };
        let content = match self.data {
            Data::Texture(image) => image.finish(reservation).map(Finished::Texture),
            Data::Material(description) => {
                let reservation = reservation.retain(size_of::<MaterialDescription>()).map_err(|_| Failure::TooLarge)?;
                Ok(Finished::Other(Content::Material(Material(Arc::new(MaterialInner { description, _reservation: reservation })))))
            }
            Data::Environment(description) => {
                let bytes = description.retained_bytes() + size_of::<Resident>() + 2 * size_of::<usize>();
                let reservation = reservation.retain(bytes).map_err(|_| Failure::TooLarge)?;
                Ok(Finished::Other(Content::Environment(Environment(Arc::new(EnvironmentInner {
                    description,
                    _reservation: reservation,
                })))))
            }
            Data::MeshHeader(header) => {
                let reservation = reservation.retain(size_of::<MeshHeaderInner>()).map_err(|_| Failure::TooLarge)?;
                Ok(Finished::Other(Content::MeshHeader(MeshHeaderAsset(Arc::new(MeshHeaderInner { header, _reservation: reservation })))))
            }
            Data::Mesh { lod, geometry } => {
                let bytes = match &geometry {
                    mesh::LodGeometry::Geometry(mesh) => mesh.owned_bytes(),
                    mesh::LodGeometry::Empty { .. } => size_of::<mesh::LodGeometry>(),
                }
                .checked_add(size_of::<Resident>())
                .ok_or(Failure::TooLarge)?;
                let reservation = reservation.retain(bytes).map_err(|_| Failure::TooLarge)?;
                Ok(Finished::Other(Content::Mesh(MeshAsset(Arc::new(MeshInner { lod, geometry, _reservation: reservation })))))
            }
        }?;
        Ok((content, source, persisted))
    }
}

pub(crate) fn prepare_cancellable(
    request: Request,
    size: TextureSize,
    response: crate::FetchResponse,
    reservation: &mut Reservation,
    cancelled: impl Fn() -> bool,
) -> Result<Prepared, Failure> {
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    let resource = request.resource();
    let (data, source) = match response {
        crate::FetchResponse::Full(bytes) if resource.kind == Kind::Mesh => {
            let capacity = bytes.capacity();
            let data = decode_mesh(request, &bytes, Some(bytes.len() as u64), None, capacity, reservation, &cancelled)?;
            (data, Some(bytes))
        }
        crate::FetchResponse::Partial { bytes, range, total_length } if resource.kind == Kind::Mesh => {
            let capacity = bytes.capacity();
            (decode_mesh(request, &bytes, total_length, Some(range), capacity, reservation, &cancelled)?, None)
        }
        crate::FetchResponse::Full(bytes) => {
            let bytes = envelope::extract(resource, bytes)?;
            (decode_payload(resource, size, &bytes, &cancelled)?, Some(bytes))
        }
        crate::FetchResponse::Partial { .. } => return Err(Failure::Malformed),
    };
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    Ok(Prepared { kind: resource.kind, data, source })
}

pub(crate) fn prepare_cached(size: TextureSize, source: &Blob, cancelled: impl Fn() -> bool) -> Result<Prepared, Failure> {
    let data = Data::Texture(texture::decode(source, size, cancelled)?);
    Ok(Prepared { kind: Kind::Texture, data, source: None })
}

pub(crate) fn prepare_disk(
    request: Request,
    size: TextureSize,
    bytes: Vec<u8>,
    reservation: &mut Reservation,
    cancelled: impl Fn() -> bool,
) -> Result<Prepared, Failure> {
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    let resource = request.resource();
    let data = if resource.kind == Kind::Mesh {
        let capacity = bytes.capacity();
        decode_mesh(request, &bytes, Some(bytes.len() as u64), None, capacity, reservation, &cancelled)?
    } else {
        decode_payload(resource, size, &bytes, &cancelled)?
    };
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    let source = (resource.kind == Kind::Texture).then_some(bytes);
    Ok(Prepared { kind: resource.kind, data, source })
}

fn decode_payload(resource: Resource, size: TextureSize, bytes: &[u8], cancelled: &impl Fn() -> bool) -> Result<Data, Failure> {
    match resource.kind {
        Kind::Texture => texture::decode(bytes, size, cancelled).map(Data::Texture),
        Kind::PbrMaterial => material::pbr::decode(bytes).map(Data::Material),
        Kind::LegacyMaterial => material::legacy::decode(bytes).map(Data::Material),
        Kind::Environment => environment::decode(bytes).map(Data::Environment),
        Kind::Mesh => Err(Failure::Malformed),
    }
}

fn decode_mesh(
    request: Request,
    bytes: &[u8],
    total_length: Option<u64>,
    range: Option<std::ops::Range<u64>>,
    input_capacity: usize,
    reservation: &mut Reservation,
    cancelled: &impl Fn() -> bool,
) -> Result<Data, Failure> {
    if bytes.len() > mesh::MAX_MESH_ASSET_BYTES {
        return Err(Failure::TooLarge);
    }
    match request {
        Request::MeshHeader(_) => {
            let parse_bytes = input_capacity
                .checked_add(mesh::MAX_MESH_HEADER_BYTES)
                .and_then(|bytes| bytes.checked_add(std::mem::size_of::<MeshHeaderInner>()))
                .ok_or(Failure::TooLarge)?;
            if !reservation.resize(parse_bytes) {
                return Err(Failure::TooLarge);
            }
            let total_length = total_length.ok_or(Failure::MalformedBlock)?;
            if let Some(range) = range {
                let initial_end = mesh::MESH_HEADER_BYTES.min(usize::try_from(total_length).unwrap_or(usize::MAX)) as u64;
                if range.start != 0
                    || range.end < initial_end
                    || range.end > mesh::MAX_MESH_HEADER_BYTES as u64
                    || range.end > total_length
                    || bytes.len() as u64 != range.end
                {
                    return Err(Failure::MalformedBlock);
                }
            }
            let header_bytes = &bytes[..bytes.len().min(mesh::MAX_MESH_HEADER_BYTES)];
            mesh::decode_header(header_bytes, total_length).map(Data::MeshHeader).map_err(mesh_failure)
        }
        Request::MeshLod(block) => {
            let lod = block.lod();
            if total_length.is_some_and(|length| length != block.asset_length()) {
                return Err(Failure::MalformedBlock);
            }
            let expected = block.range();
            let block_bytes = if let Some(range) = range {
                if range != expected || bytes.len() as u64 != expected.end - expected.start {
                    return Err(Failure::MalformedBlock);
                }
                bytes
            } else {
                if bytes.len() as u64 != block.asset_length() {
                    return Err(Failure::MalformedBlock);
                }
                let start = usize::try_from(expected.start).map_err(|_| Failure::MalformedBlock)?;
                let end = usize::try_from(expected.end).map_err(|_| Failure::MalformedBlock)?;
                bytes.get(start..end).ok_or(Failure::MalformedBlock)?
            };
            mesh::decode_lod_cancellable_reserved(block_bytes, input_capacity, reservation, cancelled)
                .map(|geometry| Data::Mesh { lod, geometry })
                .map_err(mesh_failure)
        }
        _ => Err(Failure::Malformed),
    }
}

fn mesh_failure(error: mesh::AssetError) -> Failure {
    match error {
        mesh::AssetError::MissingLod(lod) => Failure::MissingLod(lod),
        mesh::AssetError::MalformedBlock => Failure::MalformedBlock,
        mesh::AssetError::UnsupportedVersion(version) => Failure::UnsupportedVersion(version),
        mesh::AssetError::TooLarge => Failure::TooLarge,
        mesh::AssetError::Cancelled => Failure::Cancelled,
    }
}
