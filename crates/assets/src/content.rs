use std::{
    fmt::{self, Debug, Formatter},
    ops::Deref,
    sync::Arc,
};

use crate::{
    Blob, EnvironmentDescription, Failure, Image, Kind, MaterialDescription, Resource, TextureHandle, TextureSize, WeakTexture,
    budget::{Reservation, Resident},
    envelope, environment, material, texture,
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

#[derive(Clone, Debug)]
pub enum Content {
    Texture(TextureHandle),
    Material(Material),
    Environment(Environment),
}

impl Content {
    pub(crate) fn downgrade(&self) -> StoredContent {
        match self {
            Self::Texture(texture) => StoredContent::Texture(texture.downgrade()),
            Self::Material(material) => StoredContent::Material(material.clone()),
            Self::Environment(environment) => StoredContent::Environment(environment.clone()),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum StoredContent {
    Texture(WeakTexture),
    Material(Material),
    Environment(Environment),
}

impl StoredContent {
    pub(crate) fn upgrade(&self) -> Option<Content> {
        match self {
            Self::Texture(texture) => texture.upgrade().map(Content::Texture),
            Self::Material(material) => Some(Content::Material(material.clone())),
            Self::Environment(environment) => Some(Content::Environment(environment.clone())),
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
        }?;
        Ok((content, source, persisted))
    }
}

pub(crate) fn prepare_cancellable(
    resource: Resource,
    size: TextureSize,
    bytes: Vec<u8>,
    cancelled: impl Fn() -> bool,
) -> Result<Prepared, Failure> {
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    let bytes = envelope::extract(resource, bytes)?;
    let data = decode_payload(resource, size, &bytes, &cancelled)?;
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    Ok(Prepared { kind: resource.kind, data, source: Some(bytes) })
}

pub(crate) fn prepare_cached(size: TextureSize, source: &Blob, cancelled: impl Fn() -> bool) -> Result<Prepared, Failure> {
    let data = Data::Texture(texture::decode(source, size, cancelled)?);
    Ok(Prepared { kind: Kind::Texture, data, source: None })
}

pub(crate) fn prepare_disk(
    resource: Resource,
    size: TextureSize,
    bytes: Vec<u8>,
    cancelled: impl Fn() -> bool,
) -> Result<Prepared, Failure> {
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    let data = decode_payload(resource, size, &bytes, &cancelled)?;
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
    }
}
