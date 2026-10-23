use std::{
    future::{Future, pending},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use futures::{FutureExt, StreamExt, future::BoxFuture, pin_mut, stream::FuturesOrdered};
use protocol::{
    lludp::{FieldValue, Message},
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
        let Message::Schema(message) = message else { return };
        if message.schema().name != "LayerData" {
            return;
        }
        let [layer, data] = message.blocks() else { return };
        let [layer] = layer.instances.as_slice() else { return };
        let [data] = data.instances.as_slice() else { return };
        let ([FieldValue::U8(b'L')], [FieldValue::Bytes(data)]) = (layer.fields.as_slice(), data.fields.as_slice()) else { return };
        if data.len() > layer_data::MAX_BYTES {
            tracing::debug!("land layer exceeded compressed payload limit");
            return;
        }
        assert!(!self.is_full(), "intake pauses while terrain decoder admission is full");
        let data = data.clone();
        let cancelled = Arc::clone(&self.cancelled);
        self.jobs.push_back(
            runtime
                .spawn_blocking(move || decode_terrain_patches(source, &data, &cancelled))
                .map(move |result| {
                    result.unwrap_or_else(|error| {
                        tracing::error!(%error, "terrain worker stopped unexpectedly");
                        DecodedTerrain { source, patches: Vec::new(), rejected: 1 }
                    })
                })
                .boxed(),
        );
    }

    pub(super) async fn select<T>(&mut self, intake: impl Future<Output = T>) -> Result<T, DecodedTerrain> {
        let decoded =
            async { if self.jobs.is_empty() { pending().await } else { self.jobs.next().await.expect("nonempty terrain jobs") } }.fuse();
        let intake = intake.fuse();
        pin_mut!(decoded, intake);
        futures::select_biased! {
            decoded = decoded => Err(decoded),
            input = intake => Ok(input),
        }
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
    use super::*;
    use crate::session::world::{SessionWorld, viewer_world_limits};
    use futures::{channel::oneshot, future::ready};
    use network::testing::simulator_id;
    use protocol::lludp::{BlockValues, SchemaMessage};
    use world::{OrderEvidence, Region};

    fn source(order: u32) -> UpdateSource {
        UpdateSource::new(Region::new(1), order, OrderEvidence::Ordered(u64::from(order)), 0)
    }

    fn flat() -> Vec<u8> {
        vec![8, 1, 16, b'L', 0, 0, 0, 32, 65, 20, 0, 101, 38, 16]
    }

    fn message(data: Vec<u8>) -> Message {
        Message::Schema(
            SchemaMessage::new(
                SchemaMessage::schema_by_name("LayerData").unwrap(),
                vec![BlockValues::single(vec![FieldValue::U8(b'L')]), BlockValues::single(vec![FieldValue::Bytes(data)])],
            )
            .unwrap(),
        )
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
        release.pop().unwrap().send(()).unwrap();
        assert!(matches!(work.select(ready(7)).await, Ok(7)));
        for send in release {
            send.send(()).unwrap();
        }
        for order in 1..=MAX_TERRAIN_JOBS as u32 {
            let Err(decoded) = work.select(pending::<()>()).await else { unreachable!() };
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
        let Err(decoded) = work.select(pending::<()>()).await else { unreachable!() };
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
