use std::time::Instant;

use uuid::Uuid;

use super::*;
use crate::{Content, Kind, Priority, Request, Resource, Status, Store};
use test_support::texture::{RGBA_J2C, RGBA_RAW};

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
