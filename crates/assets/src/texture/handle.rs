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

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use test_support::texture::{RGBA_J2C, RGBA_RAW};
    use uuid::Uuid;

    use super::*;
    use crate::{Content, Kind, Priority, Request, Resource, Status, Store};

    fn loaded() -> (TextureHandle, Budget) {
        let budget = Budget::default();
        let mut store = Store::new(budget.clone());
        let route = store.register_route().unwrap();
        let now = Instant::now();
        let resource = Resource { id: AssetId::new(Uuid::from_u128(1)), kind: Kind::Texture };
        let demand = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
        let mut job = store.next_job(now).unwrap();
        assert!(store.try_start_decode(&mut job));
        let result = job.prepare(RGBA_J2C.to_vec());
        store.complete(job, result, now);
        let Status::Ready(Content::Texture(texture)) = demand.status() else { panic!("fixture must load") };
        (texture, budget)
    }

    #[test]
    fn final_owner_wakes_release() {
        let (texture, budget) = loaded();
        let other = texture.clone();
        let weak = texture.downgrade();
        let source = texture.events();
        let mut events = source.subscribe();
        events.borrow_and_update();
        drop(texture);
        assert!(weak.is_alive());
        assert!(!events.has_changed().unwrap(), "an ordinary clone drop does not trigger resource release");
        drop(other);
        assert!(!weak.is_alive(), "neither ready reuse nor route notifications retain ownership");
        assert!(events.has_changed().unwrap(), "the final release wakes the render owner");
        assert_eq!(budget.stats().charged_bytes, 0);
    }

    #[test]
    fn usage_controls_pixel_retention() {
        for usage in [TextureUsage::RenderOnly, TextureUsage::MainAndRender] {
            let (texture, budget) = loaded();
            texture.set_usage(usage);
            let info = texture.info().unwrap();
            let pixels = texture.image().unwrap();
            let original = pixels.pixels().as_ptr();
            texture.uploaded(info.revision + 1);
            assert!(texture.image().is_some(), "a stale acknowledgement cannot discard current pixels");
            texture.uploaded(info.revision);
            assert_eq!(texture.image().is_some(), usage == TextureUsage::MainAndRender);
            assert_eq!(texture.info(), Some(info), "upload leaves source and image metadata valid");
            let decoded = texture.decode_job().unwrap().run().unwrap();
            assert_eq!(decoded.pixels(), RGBA_RAW);
            assert_ne!(decoded.pixels().as_ptr(), original, "re-decoding reads the encoded source rather than reusing a pixel copy");
            if usage == TextureUsage::MainAndRender {
                assert_eq!(texture.image().unwrap().pixels().as_ptr(), decoded.pixels().as_ptr());
            } else {
                assert!(texture.image().is_none(), "reload pixels belong to the upload job");
            }
            drop(decoded);
            drop(pixels);
            drop(texture);
            assert_eq!(budget.stats().charged_bytes, 0);
        }
    }

    #[test]
    fn pending_decode_survives_release() {
        let (texture, budget) = loaded();
        let job = texture.decode_job().unwrap();
        let weak = texture.downgrade();
        drop(texture);
        assert!(!weak.is_alive(), "in-flight input retention does not retain the resource handle");
        assert!(budget.stats().charged_bytes > 0, "the running job still owns source bytes and scratch admission");
        assert!(matches!(job.run(), Err(Failure::Cancelled)));
        assert_eq!(budget.stats().charged_bytes, 0);
    }

    #[test]
    fn stale_decode_cannot_publish() {
        let (texture, _) = loaded();
        let job = texture.decode_job().unwrap();
        let replacement = texture.image().unwrap();
        texture.install(replacement.clone(), None);
        assert!(matches!(job.run(), Err(Failure::Cancelled)));
        assert_eq!(texture.image().unwrap().pixels().as_ptr(), replacement.pixels().as_ptr());
    }
}
