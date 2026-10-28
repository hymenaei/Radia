use std::{
    collections::{HashMap, hash_map::Entry},
    ops::Range,
    sync::Arc,
};

use super::{INSTANCE_BUFFER_BYTES, TRANSFORM_BYTES, mesh::GpuMesh, storage::Buffers};
use crate::{
    pipeline::{INSTANCE_BYTES, Pipeline},
    scene::MeshId,
    texture::TextureKey,
};
use world::Region;

pub(super) const MAX_FACE_INSTANCES: usize = INSTANCE_BUFFER_BYTES / INSTANCE_BYTES;
pub(super) const INDIRECT_DRAW_BYTES: usize = 20;
const MAX_CACHED_GROUPS: usize = 65_536;

pub(super) type FaceOrder = (u8, u128, u16, u8);

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) struct DrawKey {
    pub mesh: MeshId,
    pub start: u32,
    pub end: u32,
    pub pipeline: Pipeline,
    pub texture: Option<TextureKey>,
    pub terrain: Option<Region>,
}

impl DrawKey {
    fn command(self, first: u32, count: u32) -> DrawCommand {
        DrawCommand {
            mesh: self.mesh,
            pipeline: self.pipeline,
            first,
            count,
            indices: self.start..self.end,
            texture: self.texture,
            terrain: self.terrain,
        }
    }
}

pub(super) struct DrawCommand {
    pub mesh: MeshId,
    pub pipeline: Pipeline,
    pub first: u32,
    pub count: u32,
    pub indices: Range<u32>,
    pub texture: Option<TextureKey>,
    pub terrain: Option<Region>,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) struct DrawBatchKey {
    pub pipeline: Pipeline,
    pub texture: Option<TextureKey>,
    pub terrain: Option<Region>,
    page: usize,
}

pub(super) struct DrawBatch {
    pub key: DrawBatchKey,
    pub buffers: Arc<Buffers>,
    pub offset: u64,
    pub count: u32,
}

struct IndirectGroup {
    buffers: Arc<Buffers>,
    commands: Vec<DrawIndexedArgs>,
}

#[derive(Clone, Copy)]
struct DrawIndexedArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

struct Group {
    first: FaceOrder,
    bytes: Vec<u8>,
    orders: Vec<FaceOrder>,
    count: u32,
}

struct Blended {
    distance_squared: f64,
    order: FaceOrder,
    key: DrawKey,
    bytes: [u8; INSTANCE_BYTES],
}

#[derive(Default)]
pub(super) struct Batches {
    groups: HashMap<DrawKey, Group>,
    active_groups: Vec<DrawKey>,
    blended: Vec<Blended>,
    blend_index: HashMap<FaceOrder, usize>,
    ordered: Vec<DrawKey>,
    count: usize,
    omitted: usize,
    opaque_draws: usize,
    opaque_instances: usize,
    indirect_groups: HashMap<DrawBatchKey, IndirectGroup>,
    indirect_order: Vec<DrawBatchKey>,
    draw_batches: Vec<DrawBatch>,
    indirect_bytes: Vec<u8>,
    first_instances: Vec<u32>,
}

impl Batches {
    pub fn clear(&mut self) {
        self.blended.clear();
        self.blend_index.clear();
        self.ordered.clear();
        self.count = 0;
        self.omitted = 0;
        self.opaque_draws = 0;
        self.opaque_instances = 0;
        self.indirect_order.clear();
        self.draw_batches.clear();
        self.indirect_bytes.clear();
        self.first_instances.clear();
        for group in self.indirect_groups.values_mut() {
            group.commands.clear();
        }
        for key in self.active_groups.drain(..) {
            let Some(group) = self.groups.get_mut(&key) else { continue };
            group.bytes.clear();
            group.orders.clear();
            group.count = 0;
        }
    }

    pub fn push(
        &mut self,
        key: DrawKey,
        order: FaceOrder,
        distance_squared: Option<f64>,
        encode: impl FnOnce() -> [u8; INSTANCE_BYTES],
    ) -> bool {
        if self.count == MAX_FACE_INSTANCES {
            self.omitted += 1;
            return false;
        }
        self.count += 1;
        let bytes = encode();
        if let Some(distance_squared) = distance_squared {
            self.blend_index.insert(order, self.blended.len());
            self.blended.push(Blended { distance_squared, order, key, bytes });
        } else {
            let group = self.groups.entry(key).or_insert_with(|| Group { first: order, bytes: Vec::new(), orders: Vec::new(), count: 0 });
            if group.count == 0 {
                self.active_groups.push(key);
                group.first = order;
            } else {
                group.first = group.first.min(order);
            }
            group.bytes.extend(bytes);
            group.orders.push(order);
            group.count += 1;
        }
        true
    }

    pub fn omitted(&self) -> usize {
        self.omitted
    }

    pub fn can_append(&self, count: usize) -> bool {
        self.count.checked_add(count).is_some_and(|total| total <= MAX_FACE_INSTANCES)
    }

    pub fn has_blended(&self) -> bool {
        !self.blended.is_empty()
    }

    pub fn append_opaque(
        &mut self,
        key: DrawKey,
        order: FaceOrder,
        encoded: [u8; INSTANCE_BYTES],
        draws: &mut Vec<DrawCommand>,
        bytes: &mut Vec<u8>,
        slots: &mut HashMap<FaceOrder, u32>,
    ) -> Option<u32> {
        if self.count == MAX_FACE_INSTANCES {
            self.omitted = self.omitted.saturating_add(1);
            return None;
        }
        draws.truncate(self.opaque_draws);
        bytes.truncate(self.opaque_instances * INSTANCE_BYTES);
        let group = self.groups.entry(key).or_insert_with(|| Group { first: order, bytes: Vec::new(), orders: Vec::new(), count: 0 });
        if group.count == 0 {
            self.active_groups.push(key);
            group.first = order;
        } else {
            group.first = group.first.min(order);
        }
        group.bytes.extend(encoded);
        group.orders.push(order);
        group.count += 1;
        self.count += 1;
        self.opaque_draws += 1;
        self.opaque_instances += 1;
        let first = self.opaque_instances as u32 - 1;
        bytes.extend(encoded);
        draws.push(key.command(first, 1));
        slots.insert(order, first);
        Some(first)
    }

    pub fn append_blended(&mut self, key: DrawKey, order: FaceOrder, distance_squared: f64, encoded: [u8; INSTANCE_BYTES]) -> bool {
        if self.count == MAX_FACE_INSTANCES {
            self.omitted = self.omitted.saturating_add(1);
            return false;
        }
        self.blend_index.insert(order, self.blended.len());
        self.blended.push(Blended { distance_squared, order, key, bytes: encoded });
        self.count += 1;
        true
    }

    pub fn opaque_bytes(&self) -> usize {
        self.opaque_instances * INSTANCE_BYTES
    }

    pub fn draw_batches(&self) -> &[DrawBatch] {
        &self.draw_batches
    }

    pub fn indirect_bytes(&self) -> &[u8] {
        &self.indirect_bytes
    }

    pub fn first_instance(&self, offset: u64, command: u32) -> u32 {
        let index = offset as usize / INDIRECT_DRAW_BYTES + command as usize;
        self.first_instances[index]
    }

    pub fn build_draw_batches(&mut self, draws: &[DrawCommand], meshes: &HashMap<MeshId, GpuMesh>, indirect_first_instance: bool) {
        for group in self.indirect_groups.values_mut() {
            group.commands.clear();
        }
        self.indirect_order.clear();
        self.draw_batches.clear();
        self.indirect_bytes.clear();
        self.first_instances.clear();

        for draw in &draws[..self.opaque_draws] {
            let (key, buffers, args) = indirect_command(draw, meshes);
            let group = match self.indirect_groups.entry(key) {
                Entry::Occupied(entry) => {
                    let group = entry.into_mut();
                    if group.commands.is_empty() {
                        self.indirect_order.push(key);
                    }
                    group
                }
                Entry::Vacant(entry) => {
                    self.indirect_order.push(key);
                    entry.insert(IndirectGroup { buffers, commands: Vec::new() })
                }
            };
            group.commands.push(args);
        }
        for key in &self.indirect_order {
            let Some(group) = self.indirect_groups.get(key) else { continue };
            let offset = self.indirect_bytes.len() as u64;
            for args in &group.commands {
                self.first_instances.push(args.first_instance);
                encode_draw(args, &mut self.indirect_bytes, indirect_first_instance);
            }
            self.draw_batches.push(DrawBatch { key: *key, buffers: group.buffers.clone(), offset, count: group.commands.len() as u32 });
        }

        let blended_start = self.draw_batches.len();
        for draw in &draws[self.opaque_draws..] {
            let (key, buffers, args) = indirect_command(draw, meshes);
            let merge = self.draw_batches.len() > blended_start && self.draw_batches.last().is_some_and(|batch| batch.key == key);
            if merge {
                self.first_instances.push(args.first_instance);
                encode_draw(&args, &mut self.indirect_bytes, indirect_first_instance);
                if let Some(batch) = self.draw_batches.last_mut() {
                    batch.count += 1;
                }
            } else {
                let offset = self.indirect_bytes.len() as u64;
                self.first_instances.push(args.first_instance);
                encode_draw(&args, &mut self.indirect_bytes, indirect_first_instance);
                self.draw_batches.push(DrawBatch { key, buffers, offset, count: 1 });
            }
        }
        self.indirect_groups.retain(|_, group| !group.commands.is_empty());
    }

    pub fn finish(&mut self, draws: &mut Vec<DrawCommand>, bytes: &mut Vec<u8>, slots: &mut HashMap<FaceOrder, u32>) {
        draws.clear();
        bytes.clear();
        slots.clear();
        if self.groups.len() > MAX_CACHED_GROUPS {
            self.groups.retain(|_, group| group.count > 0);
        }
        self.ordered.clear();
        self.ordered.extend(self.active_groups.iter().copied());
        self.ordered.sort_unstable_by(|a, b| self.groups[a].first.cmp(&self.groups[b].first));
        for key in &self.ordered {
            let Some(group) = self.groups.get(key) else { continue };
            if group.count == 1
                && let Some(last) = draws.last_mut()
                && last.count == 1
                && last.mesh == key.mesh
                && last.pipeline == key.pipeline
                && last.texture == key.texture
                && last.indices.end == key.start
                && bytes[last.first as usize * INSTANCE_BYTES..(last.first as usize + 1) * INSTANCE_BYTES] == group.bytes
            {
                last.indices.end = key.end;
                if let Some(order) = group.orders.first() {
                    slots.insert(*order, last.first);
                }
                continue;
            }
            let first = (bytes.len() / INSTANCE_BYTES) as u32;
            bytes.extend_from_slice(&group.bytes);
            for (index, order) in group.orders.iter().enumerate() {
                slots.insert(*order, first + index as u32);
            }
            draws.push(key.command(first, group.count));
        }
        self.opaque_draws = draws.len();
        self.opaque_instances = bytes.len() / INSTANCE_BYTES;
        self.write_blended(draws, bytes, slots);
    }

    pub fn update_blended(&mut self, order: FaceOrder, distance_squared: f64, bytes: [u8; INSTANCE_BYTES]) {
        let Some(index) = self.blend_index.get(&order).copied() else { return };
        let Some(blended) = self.blended.get_mut(index) else { return };
        blended.distance_squared = distance_squared;
        blended.bytes = bytes;
    }

    pub fn update_blended_transform(&mut self, order: FaceOrder, distance_squared: f64, transform: &[u8; TRANSFORM_BYTES]) {
        let Some(index) = self.blend_index.get(&order).copied() else { return };
        let Some(blended) = self.blended.get_mut(index) else { return };
        blended.distance_squared = distance_squared;
        blended.bytes[..TRANSFORM_BYTES].copy_from_slice(transform);
    }

    pub fn update_blended_distance(&mut self, order: FaceOrder, distance_squared: f64) {
        let Some(index) = self.blend_index.get(&order).copied() else { return };
        let Some(blended) = self.blended.get_mut(index) else { return };
        blended.distance_squared = distance_squared;
    }

    pub fn sort_blended(&mut self, draws: &mut Vec<DrawCommand>, bytes: &mut Vec<u8>, slots: &mut HashMap<FaceOrder, u32>) {
        self.write_blended(draws, bytes, slots);
    }

    pub fn blended_start(&self) -> usize {
        self.opaque_instances * INSTANCE_BYTES
    }

    fn write_blended(&mut self, draws: &mut Vec<DrawCommand>, bytes: &mut Vec<u8>, slots: &mut HashMap<FaceOrder, u32>) {
        draws.truncate(self.opaque_draws);
        bytes.truncate(self.opaque_instances * INSTANCE_BYTES);
        for blended in &self.blended {
            slots.remove(&blended.order);
        }
        self.blended.sort_unstable_by(|a, b| b.distance_squared.total_cmp(&a.distance_squared).then_with(|| a.order.cmp(&b.order)));
        self.blend_index.clear();
        for (index, blended) in self.blended.iter().enumerate() {
            self.blend_index.insert(blended.order, index);
        }
        for Blended { order, key, bytes: instance, .. } in &self.blended {
            let first = (bytes.len() / INSTANCE_BYTES) as u32;
            bytes.extend_from_slice(instance);
            slots.insert(*order, first);
            draws.push(key.command(first, 1));
        }
    }
}

fn indirect_command(draw: &DrawCommand, meshes: &HashMap<MeshId, GpuMesh>) -> (DrawBatchKey, Arc<Buffers>, DrawIndexedArgs) {
    let mesh = meshes.get(&draw.mesh).expect("draw command mesh is resident");
    let allocation = &mesh.allocation;
    let buffers = allocation.buffers().clone();
    let key = DrawBatchKey { pipeline: draw.pipeline, texture: draw.texture, terrain: draw.terrain, page: Arc::as_ptr(&buffers) as usize };
    let args = DrawIndexedArgs {
        index_count: draw.indices.end - draw.indices.start,
        instance_count: draw.count,
        first_index: allocation.indices().start + draw.indices.start,
        base_vertex: i32::try_from(allocation.vertices().start).expect("mesh page fits the GPU budget"),
        first_instance: draw.first,
    };
    (key, buffers, args)
}

fn encode_draw(args: &DrawIndexedArgs, bytes: &mut Vec<u8>, indirect_first_instance: bool) {
    let start = bytes.len();
    bytes.extend_from_slice(&args.index_count.to_le_bytes());
    bytes.extend_from_slice(&args.instance_count.to_le_bytes());
    bytes.extend_from_slice(&args.first_index.to_le_bytes());
    bytes.extend_from_slice(&args.base_vertex.to_le_bytes());
    bytes.extend_from_slice(&(if indirect_first_instance { args.first_instance } else { 0 }).to_le_bytes());
    debug_assert_eq!(bytes.len() - start, INDIRECT_DRAW_BYTES);
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;
    use crate::geometry::fixtures::scene;

    fn key() -> DrawKey {
        let (_scene, view, _camera) = scene(255);
        let mesh = view.objects().find_map(|object| object.mesh()).expect("fixture has geometry").1;
        DrawKey { mesh: mesh.id(), start: 0, end: 6, pipeline: Pipeline::Surface, texture: None, terrain: None }
    }

    #[test]
    fn opaque_groups_preserve_order() {
        let key = key();
        let other = DrawKey { start: 6, end: 12, ..key };
        let flipped = DrawKey { pipeline: Pipeline::FlippedWinding, ..key };
        let mut batches = Batches::default();
        for (key, order, value) in [(key, 3, 30), (other, 2, 20), (flipped, 4, 40), (key, 1, 10)] {
            assert!(batches.push(key, (2, order, 0, 0), None, || [value; INSTANCE_BYTES]));
        }
        let mut draws = vec![other.command(99, 99)];
        let mut bytes = Vec::new();
        let mut slots = HashMap::new();
        batches.finish(&mut draws, &mut bytes, &mut slots);
        assert_eq!(draws.len(), 3, "only matching draw keys can share a draw");
        assert_eq!(
            draws.iter().map(|draw| (draw.first, draw.count, draw.indices.clone())).collect::<Vec<_>>(),
            [(0, 2, 0..6), (2, 1, 6..12), (3, 1, 0..6)]
        );
        assert_matches!(draws[2].pipeline, Pipeline::FlippedWinding);
        assert_eq!(bytes, [30, 10, 20, 40].into_iter().flat_map(|value| [value; INSTANCE_BYTES]).collect::<Vec<_>>());
    }

    #[test]
    fn blended_depth_and_ties() {
        let key = key();
        let blend = DrawKey { pipeline: Pipeline::Blend, ..key };
        let mut batches = Batches::default();
        for (order, depth, value) in [(3, 25.0, 30), (1, 9.0, 10), (2, 25.0, 20)] {
            assert!(batches.push(blend, (2, order, 0, 0), Some(depth), || [value; INSTANCE_BYTES]));
        }
        assert!(batches.push(key, (2, 4, 0, 0), None, || [40; INSTANCE_BYTES]));
        let mut draws = Vec::new();
        let mut bytes = Vec::new();
        let mut slots = HashMap::new();
        batches.finish(&mut draws, &mut bytes, &mut slots);
        assert_eq!(draws.iter().map(|draw| (draw.first, draw.count)).collect::<Vec<_>>(), [(0, 1), (1, 1), (2, 1), (3, 1)]);
        assert_matches!(draws[0].pipeline, Pipeline::Surface);
        for draw in &draws[1..] {
            assert_matches!(draw.pipeline, Pipeline::Blend);
        }
        assert_eq!(bytes, [40, 20, 30, 10].into_iter().flat_map(|value| [value; INSTANCE_BYTES]).collect::<Vec<_>>());
    }

    #[test]
    fn camera_reorders_blended_transforms_without_changing_materials() {
        let key = DrawKey { pipeline: Pipeline::Blend, ..key() };
        let first = (2, 1, 0, 0);
        let second = (2, 2, 0, 0);
        let mut batches = Batches::default();
        assert!(batches.push(key, first, Some(1.0), || [1; INSTANCE_BYTES]));
        assert!(batches.push(key, second, Some(2.0), || [2; INSTANCE_BYTES]));
        let mut draws = Vec::new();
        let mut bytes = Vec::new();
        let mut slots = HashMap::new();
        batches.finish(&mut draws, &mut bytes, &mut slots);
        batches.update_blended_transform(first, 3.0, &[9; TRANSFORM_BYTES]);
        batches.sort_blended(&mut draws, &mut bytes, &mut slots);

        assert_eq!(slots.get(&first), Some(&0));
        assert_eq!(slots.get(&second), Some(&1));
        assert_eq!(&bytes[..TRANSFORM_BYTES], &[9; TRANSFORM_BYTES]);
        assert_eq!(&bytes[TRANSFORM_BYTES..INSTANCE_BYTES], &[1; INSTANCE_BYTES - TRANSFORM_BYTES]);
        assert_eq!(&bytes[INSTANCE_BYTES..2 * INSTANCE_BYTES], &[2; INSTANCE_BYTES]);
    }

    #[test]
    fn camera_reorders_blended_faces_without_repacking_materials() {
        let key = DrawKey { pipeline: Pipeline::Blend, ..key() };
        let first = (2, 1, 0, 0);
        let second = (2, 2, 0, 0);
        let mut batches = Batches::default();
        assert!(batches.push(key, first, Some(1.0), || [1; INSTANCE_BYTES]));
        assert!(batches.push(key, second, Some(2.0), || [2; INSTANCE_BYTES]));
        let mut draws = Vec::new();
        let mut bytes = Vec::new();
        let mut slots = HashMap::new();
        batches.finish(&mut draws, &mut bytes, &mut slots);
        batches.update_blended_distance(first, 3.0);
        batches.sort_blended(&mut draws, &mut bytes, &mut slots);

        assert_eq!(slots.get(&first), Some(&0));
        assert_eq!(slots.get(&second), Some(&1));
        assert_eq!(&bytes[..INSTANCE_BYTES], &[1; INSTANCE_BYTES]);
        assert_eq!(&bytes[INSTANCE_BYTES..2 * INSTANCE_BYTES], &[2; INSTANCE_BYTES]);
    }

    #[test]
    fn appending_opaque_keeps_blended_faces_after_opaque_draws() {
        let opaque = key();
        let blend = DrawKey { pipeline: Pipeline::Blend, ..opaque };
        let added = DrawKey { start: 6, end: 12, ..opaque };
        let blend_order = (2, 1, 0, 0);
        let mut batches = Batches::default();
        assert!(batches.push(blend, blend_order, Some(1.0), || [1; INSTANCE_BYTES]));
        assert!(batches.push(opaque, (2, 2, 0, 0), None, || [2; INSTANCE_BYTES]));
        let mut draws = Vec::new();
        let mut bytes = Vec::new();
        let mut slots = HashMap::new();
        batches.finish(&mut draws, &mut bytes, &mut slots);

        assert_eq!(batches.append_opaque(added, (2, 3, 0, 0), [3; INSTANCE_BYTES], &mut draws, &mut bytes, &mut slots), Some(1));
        batches.sort_blended(&mut draws, &mut bytes, &mut slots);

        assert_eq!(draws.iter().map(|draw| draw.pipeline).collect::<Vec<_>>(), [Pipeline::Surface, Pipeline::Surface, Pipeline::Blend]);
        assert_eq!(slots.get(&blend_order), Some(&2));
        assert_eq!(bytes, [2, 3, 1].into_iter().flat_map(|value| [value; INSTANCE_BYTES]).collect::<Vec<_>>());
    }

    #[test]
    fn appending_blended_faces_keeps_back_to_front_order() {
        let opaque = key();
        let blend = DrawKey { pipeline: Pipeline::Blend, ..opaque };
        let first = (2, 1, 0, 0);
        let second = (2, 2, 0, 0);
        let mut batches = Batches::default();
        assert!(batches.push(opaque, (2, 3, 0, 0), None, || [3; INSTANCE_BYTES]));
        assert!(batches.push(blend, first, Some(1.0), || [1; INSTANCE_BYTES]));
        let mut draws = Vec::new();
        let mut bytes = Vec::new();
        let mut slots = HashMap::new();
        batches.finish(&mut draws, &mut bytes, &mut slots);

        assert!(batches.append_blended(blend, second, 4.0, [2; INSTANCE_BYTES]));
        batches.sort_blended(&mut draws, &mut bytes, &mut slots);

        assert_eq!(draws.iter().map(|draw| draw.pipeline).collect::<Vec<_>>(), [Pipeline::Surface, Pipeline::Blend, Pipeline::Blend]);
        assert_eq!(slots.get(&second), Some(&1));
        assert_eq!(slots.get(&first), Some(&2));
        assert_eq!(bytes, [3, 2, 1].into_iter().flat_map(|value| [value; INSTANCE_BYTES]).collect::<Vec<_>>());
    }

    #[test]
    fn adjacent_faces_preserve_materials() {
        let key = key();
        let mut batches = Batches::default();
        for (start, value, pipeline) in [
            (0, 1, Pipeline::Surface),
            (6, 1, Pipeline::Surface),
            (12, 2, Pipeline::Surface),
            (18, 2, Pipeline::DoubleSided),
            (24, 2, Pipeline::DoubleSided),
        ] {
            assert!(
                batches
                    .push(DrawKey { start, end: start + 6, pipeline, ..key }, (2, 1, 0, start as u8), None, || { [value; INSTANCE_BYTES] })
            );
        }
        let mut draws = Vec::new();
        let mut bytes = Vec::new();
        let mut slots = HashMap::new();
        batches.finish(&mut draws, &mut bytes, &mut slots);
        assert_eq!(draws.iter().map(|draw| draw.indices.clone()).collect::<Vec<_>>(), [0..12, 12..18, 18..30]);
        assert_eq!(draws.iter().map(|draw| (draw.first, draw.count)).collect::<Vec<_>>(), [(0, 1), (1, 1), (2, 1)]);
        assert_eq!(bytes, [1, 2, 2].into_iter().flat_map(|value| [value; INSTANCE_BYTES]).collect::<Vec<_>>());
    }

    #[test]
    fn multi_instance_groups_preserve_order() {
        let key = key();
        let mut batches = Batches::default();
        for start in [0, 6] {
            for id in [1, 2] {
                assert!(
                    batches
                        .push(DrawKey { start, end: start + 6, ..key }, (2, id, 0, start as u8), None, || { [id as u8; INSTANCE_BYTES] })
                );
            }
        }
        let mut draws = Vec::new();
        let mut bytes = Vec::new();
        let mut slots = HashMap::new();
        batches.finish(&mut draws, &mut bytes, &mut slots);
        assert_eq!(
            draws.iter().map(|draw| (draw.first, draw.count, draw.indices.clone())).collect::<Vec<_>>(),
            [(0, 2, 0..6), (2, 2, 6..12)]
        );
        assert_eq!(
            bytes,
            [1, 2, 1, 2].into_iter().flat_map(|value| [value; INSTANCE_BYTES]).collect::<Vec<_>>(),
            "multi-instance groups retain object order across adjacent face ranges"
        );
    }

    #[test]
    fn draw_groups_have_no_arbitrary_cap() {
        let key = key();
        let mut batches = Batches::default();
        for index in 0..9_000 {
            let start = index * 6;
            let key = DrawKey { start, end: start + 6, ..key };
            assert!(batches.push(key, (2, index as u128, 0, 0), None, || [index as u8; INSTANCE_BYTES]));
        }
        let mut draws = Vec::new();
        let mut bytes = Vec::new();
        let mut slots = HashMap::new();
        batches.finish(&mut draws, &mut bytes, &mut slots);
        assert_eq!(draws.len(), 9_000);
        assert_eq!(batches.omitted(), 0);
        assert_eq!(bytes.len(), 9_000 * INSTANCE_BYTES);
    }

    #[test]
    fn instance_limit_bounds_all_draws() {
        let key = key();
        let order = (2, 1, 0, 0);
        let mut batches = Batches::default();
        for _ in 0..MAX_FACE_INSTANCES {
            assert!(batches.push(key, order, None, || [7; INSTANCE_BYTES]));
        }
        for depth in [None, Some(1.0)] {
            assert!(!batches.push(key, order, depth, || panic!("rejected faces must not be packed")));
        }
        assert_eq!(batches.omitted(), 2);
        let mut draws = Vec::new();
        let mut bytes = Vec::new();
        let mut slots = HashMap::new();
        batches.finish(&mut draws, &mut bytes, &mut slots);
        assert_eq!(draws.len(), 1);
        assert_eq!((draws[0].first, draws[0].count), (0, MAX_FACE_INSTANCES as u32));
        assert_eq!(bytes.len(), MAX_FACE_INSTANCES * INSTANCE_BYTES);
        assert!(bytes.iter().all(|byte| *byte == 7), "accepted face bytes remain packed");
    }
}
