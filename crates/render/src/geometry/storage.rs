use std::{
    iter::once,
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use wgpu::{Buffer, BufferUsages, Device};

use super::{Accounting, Lease, Resource, buffer};
use crate::pipeline::VERTEX_BYTES;

const PAGE_BYTES: usize = 1024 * 1024;

pub(super) struct Buffers {
    pub vertices: Buffer,
    pub indices: Buffer,
    _lease: Lease,
}

pub(super) struct Allocation {
    buffers: Arc<Buffers>,
    block: Arc<Block>,
}

impl Allocation {
    pub fn buffers(&self) -> &Arc<Buffers> {
        &self.buffers
    }

    pub fn vertices(&self) -> Range<u32> {
        self.block.vertices.clone()
    }

    pub fn indices(&self) -> Range<u32> {
        self.block.indices.clone()
    }
}

impl Drop for Allocation {
    fn drop(&mut self) {
        self.block.release();
    }
}

struct Block {
    vertices: Range<u32>,
    indices: Range<u32>,
    released: AtomicBool,
    memory: Arc<Accounting>,
}

impl Block {
    fn release(&self) {
        self.released.store(true, Ordering::Release);
        self.memory.mesh_revision.fetch_add(1, Ordering::AcqRel);
    }
}

struct Ranges(Vec<Range<u32>>);

impl Ranges {
    fn new(capacity: u32) -> Self {
        Self(once(0..capacity).collect())
    }

    fn find(&self, count: u32) -> Option<usize> {
        if count == 0 {
            return None;
        }
        self.0.iter().position(|range| range.end - range.start >= count)
    }

    fn take(&mut self, index: usize, count: u32) -> Range<u32> {
        let start = self.0[index].start;
        let end = start + count;
        self.0[index].start = end;
        if self.0[index].is_empty() {
            self.0.remove(index);
        }
        start..end
    }

    fn release(&mut self, mut range: Range<u32>) {
        let mut index = self.0.partition_point(|free| free.start < range.start);
        if index > 0 && self.0[index - 1].end == range.start {
            index -= 1;
            range.start = self.0.remove(index).start;
        }
        if self.0.get(index).is_some_and(|free| free.start == range.end) {
            range.end = self.0.remove(index).end;
        }
        self.0.insert(index, range);
    }
}

struct Layout {
    vertices: Ranges,
    indices: Ranges,
    blocks: Vec<Arc<Block>>,
}

impl Layout {
    fn new(vertices: u32, indices: u32) -> Self {
        Self { vertices: Ranges::new(vertices), indices: Ranges::new(indices), blocks: Vec::new() }
    }

    fn allocate(&mut self, vertices: u32, indices: u32, memory: &Arc<Accounting>) -> Option<Arc<Block>> {
        self.reclaim();
        let vertex_slot = self.vertices.find(vertices)?;
        let index_slot = self.indices.find(indices)?;
        let block = Arc::new(Block {
            vertices: self.vertices.take(vertex_slot, vertices),
            indices: self.indices.take(index_slot, indices),
            released: AtomicBool::new(false),
            memory: memory.clone(),
        });
        self.blocks.push(block.clone());
        Some(block)
    }

    fn reclaim(&mut self) {
        self.blocks.retain(|block| {
            if block.released.load(Ordering::Acquire) {
                self.vertices.release(block.vertices.clone());
                self.indices.release(block.indices.clone());
                false
            } else {
                true
            }
        });
    }
}

struct Page {
    buffers: Arc<Buffers>,
    layout: Layout,
}

#[derive(Default)]
pub(super) struct Storage {
    pages: Vec<Page>,
}

impl Storage {
    pub fn allocate(&mut self, device: &Device, vertices: usize, indices: usize, memory: &Arc<Accounting>) -> Option<Allocation> {
        let vertex_count = u32::try_from(vertices).ok()?;
        let index_count = u32::try_from(indices).ok()?;
        if vertex_count == 0 || index_count == 0 {
            return None;
        }
        for page in &mut self.pages {
            if let Some(block) = page.layout.allocate(vertex_count, index_count, memory) {
                return Some(Allocation { buffers: page.buffers.clone(), block });
            }
        }
        let (vertex_bytes, index_bytes) = page_bytes(vertices, indices)?;
        let bytes = vertex_bytes.checked_add(index_bytes)?;
        let vertex_capacity = u32::try_from(vertex_bytes / VERTEX_BYTES).ok()?;
        let index_capacity = u32::try_from(index_bytes / size_of::<u32>()).ok()?;
        let lease = memory.reserve(bytes, Resource::Mesh).or_else(|| {
            self.pages.retain(|page| !page.layout.blocks.is_empty());
            memory.reserve(bytes, Resource::Mesh)
        })?;
        let buffers = Arc::new(Buffers {
            vertices: buffer(device, "Scene vertex page", vertex_bytes, BufferUsages::VERTEX),
            indices: buffer(device, "Scene index page", index_bytes, BufferUsages::INDEX),
            _lease: lease,
        });
        let mut layout = Layout::new(vertex_capacity, index_capacity);
        let block = layout.allocate(vertex_count, index_count, memory).expect("new page fits its admitted mesh");
        self.pages.push(Page { buffers: buffers.clone(), layout });
        Some(Allocation { buffers, block })
    }
}

fn page_bytes(vertices: usize, indices: usize) -> Option<(usize, usize)> {
    let vertex_bytes = vertices.checked_mul(VERTEX_BYTES)?;
    let index_bytes = indices.checked_mul(size_of::<u32>())?;
    let bytes = vertex_bytes.checked_add(index_bytes)?;
    let meshes = PAGE_BYTES.checked_div(bytes)?.max(1);
    Some((vertex_bytes.checked_mul(meshes)?, index_bytes.checked_mul(meshes)?))
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
