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
#[path = "terrain_tests.rs"]
mod tests;
