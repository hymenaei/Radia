use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use wgpu::{Buffer, BufferDescriptor, BufferUsages, CommandEncoder, Device, MapMode, QuerySet, QuerySetDescriptor, QueryType};

const TIMESTAMP_COUNT: u32 = 4;
const TIMESTAMP_BYTES: u64 = TIMESTAMP_COUNT as u64 * size_of::<u64>() as u64;

pub(crate) struct GpuTimings {
    queries: QuerySet,
    resolve: Buffer,
    readback: Buffer,
    sender: Sender<bool>,
    receiver: Receiver<bool>,
    timestamp_period_ns: f32,
    pending: bool,
    latest: Option<(f64, f64)>,
}

impl GpuTimings {
    pub(crate) fn new(device: &Device, timestamp_period_ns: f32) -> Option<Self> {
        if timestamp_period_ns <= 0.0 {
            return None;
        }
        let queries = device.create_query_set(&QuerySetDescriptor {
            label: Some("Scene pass timestamps"),
            ty: QueryType::Timestamp,
            count: TIMESTAMP_COUNT,
        });
        let resolve = device.create_buffer(&BufferDescriptor {
            label: Some("Scene timestamp resolve"),
            size: TIMESTAMP_BYTES,
            usage: BufferUsages::QUERY_RESOLVE | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&BufferDescriptor {
            label: Some("Scene timestamp readback"),
            size: TIMESTAMP_BYTES,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let (sender, receiver) = channel();
        Some(Self { queries, resolve, readback, sender, receiver, timestamp_period_ns, pending: false, latest: None })
    }

    pub(crate) fn recording(&self) -> bool {
        !self.pending
    }

    pub(crate) fn query_set(&self) -> &QuerySet {
        &self.queries
    }

    pub(crate) fn resolve(&self, encoder: &mut CommandEncoder) {
        encoder.resolve_query_set(&self.queries, 0..TIMESTAMP_COUNT, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, TIMESTAMP_BYTES);
    }

    pub(crate) fn submitted(&mut self) {
        self.pending = true;
        let sender = self.sender.clone();
        self.readback.slice(..).map_async(MapMode::Read, move |result| {
            if let Err(error) = sender.send(result.is_ok()) {
                tracing::trace!(%error, "GPU timestamp receiver closed");
            }
        });
    }

    pub(crate) fn poll(&mut self) {
        if !self.pending {
            return;
        }
        match self.receiver.try_recv() {
            Ok(true) => {
                let mapped = match self.readback.slice(..).get_mapped_range() {
                    Ok(mapped) => mapped,
                    Err(error) => {
                        tracing::trace!(%error, "GPU timestamp readback was unavailable");
                        self.readback.unmap();
                        self.pending = false;
                        return;
                    }
                };
                let mut timestamps = [0; TIMESTAMP_COUNT as usize];
                for (index, timestamp) in timestamps.iter_mut().enumerate() {
                    let start = index * size_of::<u64>();
                    let mut bytes = [0; size_of::<u64>()];
                    bytes.copy_from_slice(&mapped[start..start + size_of::<u64>()]);
                    *timestamp = u64::from_ne_bytes(bytes);
                }
                self.latest = Some((
                    elapsed_ms(timestamps[0], timestamps[1], self.timestamp_period_ns),
                    elapsed_ms(timestamps[2], timestamps[3], self.timestamp_period_ns),
                ));
                drop(mapped);
                self.readback.unmap();
                self.pending = false;
            }
            Ok(false) => self.pending = false,
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.pending = false,
        }
    }

    pub(crate) fn latest(&self) -> Option<(f64, f64)> {
        self.latest
    }
}

fn elapsed_ms(start: u64, end: u64, timestamp_period_ns: f32) -> f64 {
    end.wrapping_sub(start) as f64 * f64::from(timestamp_period_ns) / 1_000_000.0
}
