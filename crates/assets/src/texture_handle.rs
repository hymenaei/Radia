use std::{
    fmt,
    sync::{
        Arc, Mutex, PoisonError, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

use tokio::sync::watch;
use utils::AssetId;

use crate::{Blob, Budget, Failure, Image, PixelFormat, TextureSize, budget::TEXTURE_WORK_BYTES, texture};

#[cfg(test)]
#[path = "texture_handle_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TextureId(u64);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextureUsage {
    #[default]
    RenderOnly,
    MainAndRender,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageInfo {
    pub source_size: [u32; 2],
    pub size: [u32; 2],
    pub discard: u8,
    pub format: PixelFormat,
    pub revision: u64,
}

#[derive(Clone)]
pub struct TextureEvents(watch::Sender<u64>);

impl Default for TextureEvents {
    fn default() -> Self {
        Self(watch::channel(0).0)
    }
}

impl TextureEvents {
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.subscribe()
    }

    pub fn revision(&self) -> u64 {
        *self.0.borrow()
    }

    pub fn same_source(&self, other: &Self) -> bool {
        self.0.same_channel(&other.0)
    }

    fn notify(&self) {
        self.0.send_modify(|revision| *revision = revision.checked_add(1).expect("texture events exhausted"));
    }
}

struct TextureData {
    info: Option<ImageInfo>,
    image: Option<Image>,
    source: Option<Blob>,
    usage: TextureUsage,
}

struct TextureInner {
    id: TextureId,
    asset: AssetId,
    data: Mutex<TextureData>,
    budget: Budget,
    events: TextureEvents,
}

impl Drop for TextureInner {
    fn drop(&mut self) {
        self.events.notify();
        self.budget.wake().send_replace(());
    }
}

#[derive(Clone)]
pub struct TextureHandle(Arc<TextureInner>);

impl fmt::Debug for TextureHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextureHandle").field("id", &self.id()).field("asset", &self.0.asset).field("info", &self.info()).finish()
    }
}

#[derive(Clone)]
pub struct WeakTexture(Weak<TextureInner>);

impl fmt::Debug for WeakTexture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WeakTexture").field("alive", &self.is_alive()).finish()
    }
}

impl WeakTexture {
    pub fn upgrade(&self) -> Option<TextureHandle> {
        self.0.upgrade().map(TextureHandle)
    }

    pub fn is_alive(&self) -> bool {
        self.0.strong_count() > 0
    }
}

impl TextureHandle {
    pub(crate) fn new(asset: AssetId, budget: Budget, events: TextureEvents) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = TextureId(NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1)).expect("texture IDs exhausted"));
        Self(Arc::new(TextureInner {
            id,
            asset,
            data: Mutex::new(TextureData { info: None, image: None, source: None, usage: TextureUsage::RenderOnly }),
            budget,
            events,
        }))
    }

    pub fn id(&self) -> TextureId {
        self.0.id
    }

    pub fn asset(&self) -> AssetId {
        self.0.asset
    }

    pub fn downgrade(&self) -> WeakTexture {
        WeakTexture(Arc::downgrade(&self.0))
    }

    pub fn events(&self) -> TextureEvents {
        self.0.events.clone()
    }

    pub fn info(&self) -> Option<ImageInfo> {
        self.0.data.lock().unwrap_or_else(PoisonError::into_inner).info
    }

    pub fn image(&self) -> Option<Image> {
        self.0.data.lock().unwrap_or_else(PoisonError::into_inner).image.clone()
    }

    pub fn usage(&self) -> TextureUsage {
        self.0.data.lock().unwrap_or_else(PoisonError::into_inner).usage
    }

    pub fn set_usage(&self, usage: TextureUsage) {
        self.0.data.lock().unwrap_or_else(PoisonError::into_inner).usage = usage;
    }

    pub fn uploaded(&self, revision: u64) {
        let mut data = self.0.data.lock().unwrap_or_else(PoisonError::into_inner);
        if data.usage == TextureUsage::RenderOnly && data.info.is_some_and(|info| info.revision == revision) {
            data.image = None;
        }
    }

    pub(crate) fn source(&self) -> Option<Blob> {
        self.0.data.lock().unwrap_or_else(PoisonError::into_inner).source.clone()
    }

    pub(crate) fn install(&self, image: Image, source: Option<Blob>) {
        let mut data = self.0.data.lock().unwrap_or_else(PoisonError::into_inner);
        let revision = data.info.map_or(1, |info| info.revision.checked_add(1).expect("texture revisions exhausted"));
        data.info = Some(ImageInfo {
            source_size: image.source_size(),
            size: image.size(),
            discard: image.discard_level(),
            format: image.format(),
            revision,
        });
        data.image = Some(image);
        if source.is_some() {
            data.source = source;
        }
        drop(data);
        self.0.events.notify();
    }

    pub fn decode_job(&self) -> Option<TextureDecodeJob> {
        let data = self.0.data.lock().unwrap_or_else(PoisonError::into_inner);
        let source = data.source.clone()?;
        let info = data.info?;
        let reservation = self.0.budget.reserve(TEXTURE_WORK_BYTES)?;
        Some(TextureDecodeJob { texture: self.downgrade(), source, info, reservation })
    }
}

pub struct TextureDecodeJob {
    texture: WeakTexture,
    source: Blob,
    info: ImageInfo,
    reservation: crate::budget::Reservation,
}

impl TextureDecodeJob {
    pub fn run(self) -> Result<Image, Failure> {
        let image = texture::decode(&self.source, TextureSize::new(self.info.size[0].max(self.info.size[1])), || !self.texture.is_alive())?
            .finish(self.reservation)?;
        let Some(texture) = self.texture.upgrade() else { return Err(Failure::Cancelled) };
        let mut data = texture.0.data.lock().unwrap_or_else(PoisonError::into_inner);
        if data.info != Some(self.info) {
            return Err(Failure::Cancelled);
        }
        if data.usage == TextureUsage::MainAndRender {
            data.image = Some(image.clone());
        }
        Ok(image)
    }
}
