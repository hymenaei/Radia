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
    pub composition: Option<Buffer>,
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
    pub fn allocate(
        &mut self,
        device: &Device,
        vertices: usize,
        indices: usize,
        has_composition: bool,
        memory: &Arc<Accounting>,
    ) -> Option<Allocation> {
        let vertex_count = u32::try_from(vertices).ok()?;
        let index_count = u32::try_from(indices).ok()?;
        if vertex_count == 0 || index_count == 0 {
            return None;
        }
        for page in &mut self.pages {
            page.layout.reclaim();
        }
        self.pages.retain(|page| !page.layout.blocks.is_empty());
        for page in &mut self.pages {
            if page.buffers.composition.is_some() != has_composition {
                continue;
            }
            if let Some(block) = page.layout.allocate(vertex_count, index_count, memory) {
                return Some(Allocation { buffers: page.buffers.clone(), block });
            }
        }
        let (vertex_bytes, index_bytes, composition_bytes) = page_bytes(vertices, indices, has_composition)?;
        let bytes = vertex_bytes.checked_add(index_bytes)?.checked_add(composition_bytes)?;
        let vertex_capacity = u32::try_from(vertex_bytes / VERTEX_BYTES).ok()?;
        let index_capacity = u32::try_from(index_bytes / size_of::<u32>()).ok()?;
        let lease = memory.reserve(bytes, Resource::Mesh).or_else(|| {
            self.pages.retain(|page| !page.layout.blocks.is_empty());
            memory.reserve(bytes, Resource::Mesh)
        })?;
        let buffers = Arc::new(Buffers {
            vertices: buffer(device, "Scene vertex page", vertex_bytes, BufferUsages::VERTEX),
            composition: has_composition.then(|| buffer(device, "Terrain composition page", composition_bytes, BufferUsages::VERTEX)),
            indices: buffer(device, "Scene index page", index_bytes, BufferUsages::INDEX),
            _lease: lease,
        });
        let mut layout = Layout::new(vertex_capacity, index_capacity);
        let block = layout.allocate(vertex_count, index_count, memory).expect("new page fits its admitted mesh");
        self.pages.push(Page { buffers: buffers.clone(), layout });
        Some(Allocation { buffers, block })
    }
}

fn page_bytes(vertices: usize, indices: usize, has_composition: bool) -> Option<(usize, usize, usize)> {
    let vertex_bytes = vertices.checked_mul(VERTEX_BYTES)?;
    let index_bytes = indices.checked_mul(size_of::<u32>())?;
    let composition_bytes = if has_composition { vertices.checked_mul(size_of::<f32>())? } else { 0 };
    let bytes = vertex_bytes.checked_add(index_bytes)?.checked_add(composition_bytes)?;
    let meshes = PAGE_BYTES.checked_div(bytes)?.max(1);
    Some((vertex_bytes.checked_mul(meshes)?, index_bytes.checked_mul(meshes)?, composition_bytes.checked_mul(meshes)?))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use proptest::prelude::*;

    use super::*;

    #[test]
    fn failed_admission_preserves_ranges() {
        let memory = Arc::new(Accounting::default());
        let mut layout = Layout::new(8, 12);
        let first = layout.allocate(4, 6, &memory).unwrap();
        let original = (first.vertices.clone(), first.indices.clone());
        assert!(layout.allocate(4, 9, &memory).is_none());
        assert!(layout.allocate(5, 6, &memory).is_none());
        assert!(layout.allocate(0, 6, &memory).is_none());
        assert!(layout.allocate(4, 0, &memory).is_none());
        let second = layout.allocate(4, 6, &memory).unwrap();
        let retained = (second.vertices.clone(), second.indices.clone());
        assert!(first.vertices.end <= second.vertices.start || second.vertices.end <= first.vertices.start);
        assert!(first.indices.end <= second.indices.start || second.indices.end <= first.indices.start);
        assert!(layout.allocate(1, 3, &memory).is_none());
        first.release();
        let reused = layout.allocate(4, 6, &memory).unwrap();
        assert_eq!((reused.vertices.clone(), reused.indices.clone()), original, "only the released mesh's space is available");
        assert_eq!((second.vertices.clone(), second.indices.clone()), retained, "live mesh offsets remain stable");
    }

    #[test]
    fn terrain_page_budget_includes_composition_stream() {
        let (surface_vertices, surface_indices, surface_composition) = page_bytes(289, 1536, false).unwrap();
        let (terrain_vertices, terrain_indices, terrain_composition) = page_bytes(289, 1536, true).unwrap();
        assert_eq!(surface_composition, 0);
        let surface_bytes = surface_vertices + surface_indices + surface_composition;
        let terrain_bytes = terrain_vertices + terrain_indices + terrain_composition;
        let surface_meshes = surface_vertices / (289 * VERTEX_BYTES);
        let meshes = terrain_vertices / (289 * VERTEX_BYTES);
        assert_eq!(surface_bytes, surface_meshes * (289 * VERTEX_BYTES + 1536 * size_of::<u32>()));
        assert_eq!(terrain_composition, meshes * 289 * size_of::<f32>());
        assert_eq!(terrain_bytes, meshes * (289 * VERTEX_BYTES + 1536 * size_of::<u32>() + 289 * size_of::<f32>()));
        assert!(terrain_bytes > meshes * (289 * VERTEX_BYTES + 1536 * size_of::<u32>()));
    }

    #[test]
    fn adjacent_holes_coalesce() {
        let memory = Arc::new(Accounting::default());
        let mut layout = Layout::new(16, 24);
        let blocks: Vec<_> = (0..4).map(|_| layout.allocate(4, 6, &memory).unwrap()).collect();
        blocks[0].release();
        blocks[2].release();
        assert!(layout.allocate(8, 12, &memory).is_none(), "separated holes cannot hold one contiguous mesh");
        blocks[1].release();
        let merged = layout.allocate(12, 18, &memory).unwrap();
        assert!(merged.vertices.end <= blocks[3].vertices.start || blocks[3].vertices.end <= merged.vertices.start);
        assert!(merged.indices.end <= blocks[3].indices.start || blocks[3].indices.end <= merged.indices.start);
        assert!(layout.allocate(1, 3, &memory).is_none());
    }

    proptest! {
        #[test]
        fn churn_matches_occupancy(
            ops in prop::collection::vec((0u8..3, 0usize..16, 1u32..17, 1u32..9), 1..256),
        ) {
            let memory = Arc::new(Accounting::default());
            let mut layout = Layout::new(32, 48);
            let mut slots: Vec<Option<Arc<Block>>> = vec![None; 16];
            let mut vertices = [false; 32];
            let mut indices = [false; 48];
            for (operation, slot, vertex_count, triangles) in ops {
                if operation != 1
                    && let Some(block) = slots[slot].take()
                {
                    vertices[block.vertices.start as usize..block.vertices.end as usize].fill(false);
                    indices[block.indices.start as usize..block.indices.end as usize].fill(false);
                    block.release();
                }
                if operation == 0 || slots[slot].is_some() {
                    continue;
                }
                let index_count = triangles * 3;
                let vertices_fit = vertices.windows(vertex_count as usize).any(|range| range.iter().all(|used| !used));
                let indices_fit = indices.windows(index_count as usize).any(|range| range.iter().all(|used| !used));
                let block = layout.allocate(vertex_count, index_count, &memory);
                prop_assert_eq!(block.is_some(), vertices_fit && indices_fit, "both contiguous ranges must fit before committing either");
                if let Some(block) = block {
                    prop_assert_eq!(block.vertices.len(), vertex_count as usize);
                    prop_assert_eq!(block.indices.len(), index_count as usize);
                    prop_assert!(block.vertices.end as usize <= vertices.len());
                    prop_assert!(block.indices.end as usize <= indices.len());
                    let vertex_start = block.vertices.start as usize;
                    let index_start = block.indices.start as usize;
                    prop_assert!(vertices[vertex_start..vertex_start + vertex_count as usize].iter().all(|used| !used), "live vertices cannot be overwritten");
                    prop_assert!(indices[index_start..index_start + index_count as usize].iter().all(|used| !used), "live triangles cannot be overwritten");
                    vertices[vertex_start..vertex_start + vertex_count as usize].fill(true);
                    indices[index_start..index_start + index_count as usize].fill(true);
                    slots[slot] = Some(block);
                }
            }
        }
    }
}
