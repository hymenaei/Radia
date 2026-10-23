use std::{
    future::pending,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use futures::{FutureExt, StreamExt, future::BoxFuture, stream::FuturesOrdered};
use protocol::{
    lludp::Message,
    types::layer_data::{self, terrain},
};
use tokio::runtime::Handle;
use world::{TerrainPatch, UpdateSource};

const MAX_TERRAIN_JOBS: usize = 4;

pub(super) struct DecodedTerrain {
    pub(super) source: UpdateSource,
    pub(super) patches: Vec<(u32, TerrainPatch)>,
    pub(super) rejected: usize,
}

#[derive(Default)]
pub(super) struct TerrainDecodeQueue {
    jobs: FuturesOrdered<BoxFuture<'static, DecodedTerrain>>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for TerrainDecodeQueue {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl TerrainDecodeQueue {
    pub(super) fn is_full(&self) -> bool {
        self.jobs.len() == MAX_TERRAIN_JOBS
    }

    pub(super) fn enqueue(&mut self, message: &Message, source: UpdateSource, runtime: &Handle) {
        let Message::LayerData(message) = message else { return };
        if message.layer_type != b'L' {
            return;
        }
        if message.data.len() > layer_data::MAX_BYTES {
            return;
        }
        assert!(!self.is_full(), "intake pauses while terrain decoder admission is full");
        let data = message.data.clone();
        let cancelled = Arc::clone(&self.cancelled);
        let span = tracing::Span::current();
        self.jobs.push_back(
            runtime
                .spawn_blocking(move || {
                    let _entered = span.enter();
                    decode_terrain_patches(source, &data, &cancelled)
                })
                .map(move |result| {
                    result.unwrap_or_else(|error| {
                        tracing::error!(region = source.region().get(), %error, "terrain worker stopped unexpectedly");
                        DecodedTerrain { source, patches: Vec::new(), rejected: 1 }
                    })
                })
                .boxed(),
        );
    }

    pub(super) async fn next_completion(&mut self) -> DecodedTerrain {
        async { if self.jobs.is_empty() { pending().await } else { self.jobs.next().await.expect("nonempty terrain jobs") } }.await
    }
}

fn decode_terrain_patches(source: UpdateSource, data: &[u8], cancelled: &AtomicBool) -> DecodedTerrain {
    let mut result = DecodedTerrain { source, patches: Vec::new(), rejected: 0 };
    let layer = match terrain::Decoder::new(b'L', data) {
        Ok(layer) => layer,
        Err(_) => {
            result.rejected = 1;
            return result;
        }
    };
    let mut layer = layer.enumerate();
    loop {
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        let Some((ordinal, patch)) = layer.next() else { break };
        match patch {
            Ok(patch) => {
                let (x, y) = patch.coordinates();
                match TerrainPatch::new(x, y, patch.into_heights()) {
                    Ok(patch) => result.patches.push((ordinal as u32, patch)),
                    Err(_) => result.rejected += 1,
                }
            }
            Err(error) => {
                result.rejected += 1;
                if error != layer_data::Error::NonFinite {
                    break;
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use futures::channel::oneshot;
    use network::testing::simulator_id;
    use protocol::messages::LayerData;
    use world::{OrderEvidence, Region};

    use super::*;
    use crate::session::world::{SessionWorld, viewer_world_limits};

    fn source(order: u32) -> UpdateSource {
        UpdateSource::new(Region::new(1), order, OrderEvidence::Ordered(u64::from(order)), 0)
    }

    fn flat() -> Vec<u8> {
        vec![8, 1, 16, b'L', 0, 0, 0, 32, 65, 20, 0, 101, 38, 16]
    }

    fn message(data: Vec<u8>) -> Message {
        Message::LayerData(LayerData { layer_type: b'L', data })
    }

    #[tokio::test]
    async fn ordered_jobs_cancel_safely() {
        let mut work = TerrainDecodeQueue::default();
        let mut release = Vec::new();
        for order in 1..=MAX_TERRAIN_JOBS as u32 {
            let (send, receive) = oneshot::channel();
            release.push(send);
            work.jobs.push_back(
                async move {
                    receive.await.unwrap();
                    DecodedTerrain { source: source(order), patches: Vec::new(), rejected: 0 }
                }
                .boxed(),
            );
        }
        assert!(work.is_full());
        release.remove(0).send(()).unwrap();
        let decoded = work.next_completion().await;
        assert_eq!(decoded.source.sequence(), 1);
        for send in release {
            send.send(()).unwrap();
        }
        for order in 2..=MAX_TERRAIN_JOBS as u32 {
            let decoded = work.next_completion().await;
            assert_eq!(decoded.source.sequence(), order);
            assert!(!work.is_full());
        }
        let cancelled = Arc::clone(&work.cancelled);
        drop(work);
        let decoded = decode_terrain_patches(source(1), &flat(), &cancelled);
        assert!(decoded.patches.is_empty());
    }

    #[tokio::test]
    async fn intake_rejects_retired_regions() {
        let mut world = SessionWorld::new(viewer_world_limits());
        let simulator = simulator_id();
        let region = world.register_simulator(simulator, None).unwrap();
        let source = UpdateSource::new(region, 1, OrderEvidence::Ordered(1), 0);
        let mut work = TerrainDecodeQueue::default();
        work.enqueue(&message(flat()), source, &Handle::current());
        let decoded = work.next_completion().await;
        assert_eq!(decoded.rejected, 0);
        world.apply_terrain(decoded);
        let page = world.world().terrain_page(region, None, 8).unwrap().unwrap();
        assert_eq!(page.known_patches, 1);
        assert_eq!(page.patches[0].heights(), &[20.0; 256]);
        let mut truncated = flat();
        truncated.pop();
        let partial = decode_terrain_patches(source, &truncated, &AtomicBool::new(false));
        assert_eq!(partial.patches.len(), 1);
        assert_eq!(partial.rejected, 1);
        let late = decode_terrain_patches(source, &flat(), &AtomicBool::new(false));
        world.retire_all();
        world.apply_terrain(late);
        assert_eq!(world.world().terrain_page(region, None, 8), Ok(None));
    }
}
