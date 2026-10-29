use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
};

use assets::TextureHandle;
use math::{DVec3, Quat, Vec3};
use wgpu::{Buffer, Queue};
use world::Region;

use super::{
    INSTANCE_BUFFER_BYTES, TRANSFORM_BYTES,
    instance::PARAMETER_BYTES,
    items::{CachedFace, CachedItem, ItemIndex, ItemSlab, draw_priority, surface_pipeline},
    mesh::GpuMesh,
    storage::Buffers,
    texture_bindings::{TerrainDemand, TextureDemand},
};
use crate::{
    pipeline::{INSTANCE_BYTES, Pipeline},
    scene::{Camera, MeshId},
    texture::TextureKey,
};

pub(super) const MAX_FACE_INSTANCES: usize = INSTANCE_BUFFER_BYTES / INSTANCE_BYTES;
pub(super) const INDIRECT_DRAW_BYTES: usize = 20;
pub(super) type FaceOrder = (u8, u128, u16, u8);

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) struct DrawKey {
    pub(super) mesh: MeshId,
    pub(super) start: u32,
    pub(super) end: u32,
    pub(super) pipeline: Pipeline,
    pub(super) texture: Option<TextureKey>,
    pub(super) terrain: Option<Region>,
}

impl DrawKey {
    pub(super) fn blended(self) -> bool {
        matches!(self.pipeline, Pipeline::Blend | Pipeline::BlendDoubleSided | Pipeline::BlendFlipped)
    }

    fn command(self, group: u32, first: u32, count: u32) -> DrawCommand {
        DrawCommand {
            group,
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
    group: u32,
    pub(super) mesh: MeshId,
    pub(super) pipeline: Pipeline,
    pub(super) first: u32,
    pub(super) count: u32,
    pub(super) indices: Range<u32>,
    pub(super) texture: Option<TextureKey>,
    pub(super) terrain: Option<Region>,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) struct DrawBatchKey {
    pub(super) pipeline: Pipeline,
    pub(super) texture: Option<TextureKey>,
    pub(super) terrain: Option<Region>,
    page: usize,
}

pub(super) struct DrawBatch {
    id: usize,
    pub(super) key: DrawBatchKey,
    pub(super) buffers: Arc<Buffers>,
    pub(super) offset: u64,
    pub(super) count: u32,
}

struct BatchGroup {
    key: DrawBatchKey,
    buffers: Arc<Buffers>,
    commands: Vec<DrawIndexedArgs>,
    refs: usize,
}

#[derive(Clone, Copy)]
struct BatchBinding {
    id: usize,
    index_offset: u32,
    base_vertex: i32,
}

#[derive(Clone, Copy)]
struct DrawIndexedArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

struct DrawGroup {
    key: DrawKey,
    refs: usize,
    texture: Option<TextureHandle>,
    terrain_textures: Option<[Option<TextureHandle>; 4]>,
    batch: Option<BatchBinding>,
}

#[derive(Default)]
struct DrawInterner {
    by_key: HashMap<DrawKey, u32>,
    groups: Vec<Option<DrawGroup>>,
    free: Vec<u32>,
    batches_by_key: HashMap<DrawBatchKey, usize>,
    batches: Vec<Option<BatchGroup>>,
    free_batches: Vec<usize>,
}

impl DrawInterner {
    pub(super) fn intern(
        &mut self,
        key: DrawKey,
        texture: Option<&TextureHandle>,
        terrain_textures: Option<&[Option<TextureHandle>; 4]>,
    ) -> Option<u32> {
        if let Some(id) = self.by_key.get(&key).copied() {
            let group = self.groups.get_mut(id as usize).and_then(Option::as_mut)?;
            group.refs = group.refs.checked_add(1)?;
            if texture.is_some() {
                group.texture = texture.cloned();
            }
            if terrain_textures.is_some() {
                group.terrain_textures = terrain_textures.cloned();
            }
            return Some(id);
        }
        let id = match self.free.pop() {
            Some(id) => id,
            None => u32::try_from(self.groups.len()).ok()?,
        };
        if id as usize == self.groups.len() {
            self.groups.push(None);
        }
        let slot = self.groups.get_mut(id as usize)?;
        *slot = Some(DrawGroup { key, refs: 1, texture: texture.cloned(), terrain_textures: terrain_textures.cloned(), batch: None });
        self.by_key.insert(key, id);
        Some(id)
    }

    pub(super) fn key(&self, id: u32) -> Option<DrawKey> {
        self.groups.get(id as usize).and_then(Option::as_ref).map(|group| group.key)
    }

    fn group(&self, id: u32) -> Option<&DrawGroup> {
        self.groups.get(id as usize).and_then(Option::as_ref)
    }

    fn batch(&self, id: u32) -> Option<BatchBinding> {
        self.group(id).and_then(|group| group.batch)
    }

    fn set_batch(&mut self, id: u32, batch: BatchBinding) {
        if let Some(group) = self.groups.get_mut(id as usize).and_then(Option::as_mut) {
            group.batch = Some(batch);
        }
    }

    fn batch_group(&self, id: usize) -> Option<&BatchGroup> {
        self.batches.get(id).and_then(Option::as_ref)
    }

    fn batch_group_mut(&mut self, id: usize) -> Option<&mut BatchGroup> {
        self.batches.get_mut(id).and_then(Option::as_mut)
    }

    pub(super) fn release(&mut self, id: u32) {
        let batch = {
            let Some(group) = self.groups.get_mut(id as usize).and_then(Option::as_mut) else { return };
            if group.refs == 0 {
                return;
            }
            group.refs -= 1;
            if group.refs != 0 {
                return;
            }
            group.batch.map(|batch| batch.id)
        };
        let key = self.groups.get(id as usize).and_then(Option::as_ref).map(|group| group.key);
        let Some(key) = key else { return };
        self.by_key.remove(&key);
        if let Some(slot) = self.groups.get_mut(id as usize) {
            *slot = None;
            self.free.push(id);
        }
        if let Some(batch) = batch {
            self.release_batch(batch);
        }
    }

    fn intern_batch(&mut self, key: DrawBatchKey, buffers: &Arc<Buffers>) -> usize {
        if let Some(id) = self.batches_by_key.get(&key).copied() {
            if let Some(group) = self.batches.get_mut(id).and_then(Option::as_mut) {
                group.refs += 1;
            }
            return id;
        }
        let id = match self.free_batches.pop() {
            Some(id) => id,
            None => self.batches.len(),
        };
        if id == self.batches.len() {
            self.batches.push(None);
        }
        if let Some(slot) = self.batches.get_mut(id) {
            *slot = Some(BatchGroup { key, buffers: Arc::clone(buffers), commands: Vec::new(), refs: 1 });
        }
        self.batches_by_key.insert(key, id);
        id
    }

    fn release_batch(&mut self, id: usize) {
        let Some(group) = self.batches.get_mut(id).and_then(Option::as_mut) else { return };
        if group.refs == 0 {
            return;
        }
        group.refs -= 1;
        if group.refs != 0 {
            return;
        }
        self.batches_by_key.remove(&group.key);
        if let Some(slot) = self.batches.get_mut(id) {
            *slot = None;
            self.free_batches.push(id);
        }
    }

    fn invalidate_batches(&mut self) {
        self.batches_by_key.clear();
        self.batches.clear();
        self.free_batches.clear();
        for group in self.groups.iter_mut().flatten() {
            group.batch = None;
        }
    }

    pub(super) fn update_terrain(&mut self, region: Region, textures: &[Option<TextureHandle>; 4]) {
        for group in self.groups.iter_mut().flatten().filter(|group| group.key.terrain == Some(region)) {
            group.terrain_textures = Some(textures.clone());
        }
    }

    pub(super) fn clear(&mut self) {
        self.by_key.clear();
        self.groups.clear();
        self.free.clear();
        self.batches_by_key.clear();
        self.batches.clear();
        self.free_batches.clear();
    }
}

struct BlendedFace {
    distance_squared: f64,
    order: FaceOrder,
    item: ItemIndex,
    face: usize,
    group: u32,
}

#[derive(Default)]
pub(super) struct LayoutStats {
    pub(super) omitted_faces: usize,
    pub(super) blended: bool,
}

#[derive(Default)]
pub(super) struct PatchStats {
    pub(super) faces_patched: usize,
    pub(super) blended_sorted: bool,
}

#[derive(Default)]
pub(super) struct DrawList {
    groups: DrawInterner,
    draws: Vec<DrawCommand>,
    instance_bytes: Vec<u8>,
    active_groups: Vec<u32>,
    active_textures: Vec<TextureDemand>,
    active_terrain: Vec<TerrainDemand>,
    texture_seen: HashSet<TextureKey>,
    terrain_seen: HashSet<Region>,
    group_counts: Vec<usize>,
    group_offsets: Vec<usize>,
    group_cursors: Vec<usize>,
    layout_items: Vec<ItemIndex>,
    priority_items: Vec<ItemIndex>,
    blended: Vec<BlendedFace>,
    dirty_slots: Vec<u32>,
    opaque_draws: usize,
    opaque_instances: usize,
    instance_count: usize,
    visible_items: usize,
    omitted: usize,
    valid: bool,
    batch_order: Vec<usize>,
    draw_batches: Vec<DrawBatch>,
    indirect_bytes: Vec<u8>,
    first_instances: Vec<u32>,
}

impl DrawList {
    pub(super) fn layout(&mut self, previous: &[ItemIndex], visible: &[ItemIndex], items: &mut ItemSlab, camera: Camera) -> LayoutStats {
        self.draws.clear();
        self.active_groups.clear();
        self.active_textures.clear();
        self.active_terrain.clear();
        self.texture_seen.clear();
        self.terrain_seen.clear();
        self.blended.clear();
        self.layout_items.clear();
        self.priority_items.clear();
        self.dirty_slots.clear();
        self.opaque_draws = 0;
        self.opaque_instances = 0;
        self.instance_count = 0;
        self.visible_items = 0;
        self.omitted = 0;

        if previous != visible {
            for index in previous {
                if !contains_item(visible, items, *index)
                    && let Some(item) = items.get_mut(*index)
                {
                    for face in &mut item.faces {
                        face.slot = None;
                        face.blended_index = None;
                    }
                }
            }
        }

        self.group_counts.resize(self.groups.groups.len(), 0);
        self.group_offsets.resize(self.groups.groups.len(), 0);
        self.group_cursors.resize(self.groups.groups.len(), 0);
        let mut required = 0usize;
        for id in visible {
            let Some(item) = items.get(*id) else { continue };
            required = required.saturating_add(item.faces.len());
            let drawable = self.count_item_faces(item);
            self.visible_items += usize::from(drawable);
        }
        if required > MAX_FACE_INSTANCES {
            self.omitted = 0;
            self.visible_items = 0;
            self.clear_group_counts();
            self.priority_items.extend_from_slice(visible);
            self.priority_items.sort_unstable_by(|a, b| {
                let a_priority = items.get(*a).map_or((0.0, f64::INFINITY), |item| draw_priority(item, camera));
                let b_priority = items.get(*b).map_or((0.0, f64::INFINITY), |item| draw_priority(item, camera));
                b_priority
                    .0
                    .total_cmp(&a_priority.0)
                    .then_with(|| a_priority.1.total_cmp(&b_priority.1))
                    .then_with(|| items.get(*a).map(|item| item.id).cmp(&items.get(*b).map(|item| item.id)))
            });
            let mut remaining = MAX_FACE_INSTANCES;
            for id in &self.priority_items {
                let count = items.get(*id).map_or(0, |item| item.faces.len());
                if count <= remaining {
                    remaining -= count;
                    self.layout_items.push(*id);
                } else {
                    self.omitted = self.omitted.saturating_add(count);
                    if let Some(item) = items.get_mut(*id) {
                        for face in &mut item.faces {
                            face.slot = None;
                            face.blended_index = None;
                        }
                    }
                }
            }
            for index in 0..self.layout_items.len() {
                let id = self.layout_items[index];
                let Some(item) = items.get(id) else { continue };
                let drawable = self.count_item_faces(item);
                self.visible_items += usize::from(drawable);
            }
        }
        let layout_items = if required > MAX_FACE_INSTANCES { &self.layout_items } else { visible };

        let mut opaque_count = 0usize;
        for group_id in &self.active_groups {
            let index = *group_id as usize;
            let count = std::mem::take(&mut self.group_counts[index]);
            let Some(group) = self.groups.groups[index].as_ref() else { continue };
            if let Some(key) = group.key.texture
                && self.texture_seen.insert(key)
            {
                self.active_textures.push(TextureDemand { key, handle: group.texture.clone() });
            }
            if let Some(region) = group.key.terrain
                && self.terrain_seen.insert(region)
            {
                self.active_terrain.push(TerrainDemand {
                    region,
                    textures: group.terrain_textures.clone().unwrap_or_else(|| std::array::from_fn(|_| None)),
                });
            }
            if !group.key.blended() {
                self.group_offsets[index] = opaque_count;
                self.group_cursors[index] = 0;
                opaque_count = opaque_count.saturating_add(count);
                if count > 0
                    && let (Ok(first), Ok(count)) = (u32::try_from(opaque_count - count), u32::try_from(count))
                {
                    self.draws.push(group.key.command(*group_id, first, count));
                }
            }
        }
        self.active_groups.clear();
        self.opaque_draws = self.draws.len();
        self.opaque_instances = opaque_count;
        self.instance_bytes.resize(opaque_count.saturating_mul(INSTANCE_BYTES), 0);

        for id in layout_items {
            let Some(item) = items.get_mut(*id) else { continue };
            for (face_index, face) in item.faces.iter_mut().enumerate() {
                if face.blended {
                    self.blended.push(BlendedFace {
                        distance_squared: blend_distance(item.position, item.rotation, item.scale, face.center, camera),
                        order: face.order,
                        item: *id,
                        face: face_index,
                        group: face.group,
                    });
                } else {
                    let group_index = face.group as usize;
                    let cursor = &mut self.group_cursors[group_index];
                    let offset = self.group_offsets[group_index].saturating_add(*cursor);
                    let Ok(slot) = u32::try_from(offset) else { continue };
                    if write_instance(&mut self.instance_bytes, slot, &item.transform, &face.parameter_bytes) {
                        face.slot = Some(slot);
                        *cursor += 1;
                    }
                }
            }
        }
        self.reorder_blended(items);
        self.instance_count = self.instance_bytes.len() / INSTANCE_BYTES;
        self.valid = true;
        LayoutStats { omitted_faces: self.omitted, blended: !self.blended.is_empty() }
    }

    fn count_item_faces(&mut self, item: &CachedItem) -> bool {
        let mut drawable = false;
        for face in &item.faces {
            let index = face.group as usize;
            let count = &mut self.group_counts[index];
            if *count == 0 {
                self.active_groups.push(face.group);
            }
            *count += 1;
            drawable = true;
        }
        drawable
    }

    pub(super) fn patch(
        &mut self,
        items: &mut ItemSlab,
        faces: &[(ItemIndex, usize)],
        pose_items: &[ItemIndex],
        camera: Camera,
    ) -> PatchStats {
        self.dirty_slots.clear();
        let mut stats = PatchStats::default();
        for (item_id, face_index) in faces {
            let Some(item) = items.get_mut(*item_id) else { continue };
            let Some(face) = item.faces.get_mut(*face_index) else { continue };
            let Some(slot) = face.slot else { continue };
            if !write_instance(&mut self.instance_bytes, slot, &item.transform, &face.parameter_bytes) {
                continue;
            }
            self.dirty_slots.push(slot);
            stats.faces_patched += 1;
            if pose_items.binary_search(item_id).is_ok()
                && let Some(index) = face.blended_index
                && let Some(blended) = self.blended.get_mut(index)
            {
                blended.distance_squared = blend_distance(item.position, item.rotation, item.scale, face.center, camera);
                stats.blended_sorted = true;
            }
        }
        self.dirty_slots.sort_unstable();
        self.dirty_slots.dedup();
        stats
    }

    pub(super) fn sort_blended(&mut self, items: &mut ItemSlab, camera: Camera) -> bool {
        if self.blended.is_empty() {
            return false;
        }
        for blended in &mut self.blended {
            if let Some(item) = items.get(blended.item)
                && let Some(face) = item.faces.get(blended.face)
            {
                blended.distance_squared = blend_distance(item.position, item.rotation, item.scale, face.center, camera);
            }
        }
        self.reorder_blended(items);
        true
    }

    pub(super) fn reorder_blended(&mut self, items: &mut ItemSlab) {
        self.blended.sort_unstable_by(|a, b| b.distance_squared.total_cmp(&a.distance_squared).then_with(|| a.order.cmp(&b.order)));
        self.draws.truncate(self.opaque_draws);
        self.instance_bytes.truncate(self.opaque_instances.saturating_mul(INSTANCE_BYTES));
        for (index, blended) in self.blended.iter().enumerate() {
            let Ok(slot) = u32::try_from(self.opaque_instances.saturating_add(index)) else { continue };
            let Some(item) = items.get_mut(blended.item) else { continue };
            let Some(face) = item.faces.get_mut(blended.face) else { continue };
            face.slot = Some(slot);
            face.blended_index = Some(index);
            self.instance_bytes.extend_from_slice(&item.transform);
            self.instance_bytes.extend_from_slice(&face.parameter_bytes);
            if let Some(key) = self.groups.key(blended.group) {
                self.draws.push(key.command(blended.group, slot, 1));
            }
        }
        self.instance_count = self.instance_bytes.len() / INSTANCE_BYTES;
    }

    pub(super) fn upload_layout(&self, queue: &Queue, buffer: &Buffer) {
        if !self.instance_bytes.is_empty() {
            queue.write_buffer(buffer, 0, &self.instance_bytes);
        }
    }

    pub(super) fn upload_patch(&mut self, queue: &Queue, buffer: &Buffer, reordered_blended: bool) {
        let mut first = None;
        let mut last = 0u32;
        for slot in self.dirty_slots.iter().copied().filter(|slot| !reordered_blended || (*slot as usize) < self.opaque_instances) {
            if let Some(start) = first
                && slot != last.saturating_add(1)
            {
                self.write_slot_range(queue, buffer, start, last);
                first = None;
            }
            if first.is_none() {
                first = Some(slot);
            }
            last = slot;
        }
        if let Some(start) = first {
            self.write_slot_range(queue, buffer, start, last);
        }
        if reordered_blended {
            let start = self.opaque_instances.saturating_mul(INSTANCE_BYTES);
            if let Some(bytes) = self.instance_bytes.get(start..)
                && !bytes.is_empty()
            {
                queue.write_buffer(buffer, start as u64, bytes);
            }
        }
        self.dirty_slots.clear();
    }

    fn write_slot_range(&self, queue: &Queue, buffer: &Buffer, first: u32, last: u32) {
        let start = first as usize * INSTANCE_BYTES;
        let end = (last as usize + 1).saturating_mul(INSTANCE_BYTES);
        if let Some(bytes) = self.instance_bytes.get(start..end) {
            queue.write_buffer(buffer, start as u64, bytes);
        }
    }

    pub(super) fn intern_face(
        &mut self,
        key: DrawKey,
        texture: Option<&TextureHandle>,
        terrain_textures: Option<&[Option<TextureHandle>; 4]>,
    ) -> Option<u32> {
        self.groups.intern(key, texture, terrain_textures)
    }

    #[cfg(test)]
    pub(super) fn face_key(&self, group: u32) -> Option<DrawKey> {
        self.groups.key(group)
    }

    pub(super) fn clear_groups(&mut self) {
        self.groups.clear();
    }

    pub(super) fn refresh_winding(&mut self, faces: &mut [CachedFace], old_scale: Vec3, scale: Vec3) -> bool {
        if math::flips_winding(old_scale) == math::flips_winding(scale) {
            return false;
        }
        let mut changed = false;
        for face in faces {
            let Some(mut key) = self.groups.key(face.group) else { continue };
            let previous_pipeline = key.pipeline;
            key.pipeline = surface_pipeline(face.base_params.blend, face.base_params.double_sided, scale);
            if key.pipeline == previous_pipeline {
                continue;
            }
            if let Some(group) = self.groups.intern(key, face.texture.as_ref(), None) {
                self.groups.release(face.group);
                face.group = group;
                changed = true;
            }
        }
        changed
    }

    pub(super) fn release_faces(&mut self, faces: &[CachedFace]) {
        for face in faces {
            self.groups.release(face.group);
        }
    }

    pub(super) fn update_terrain(&mut self, region: Region, textures: &[Option<TextureHandle>; 4]) {
        self.groups.update_terrain(region, textures);
        for demand in self.active_terrain.iter_mut().filter(|demand| demand.region == region) {
            demand.textures.clone_from(textures);
        }
    }

    pub(super) fn invalidate_batches(&mut self) {
        self.groups.invalidate_batches();
    }

    pub(super) fn texture_demands(&self) -> &[TextureDemand] {
        &self.active_textures
    }

    pub(super) fn terrain_demands(&self) -> &[TerrainDemand] {
        &self.active_terrain
    }

    pub(super) fn terrain_regions(&self) -> &HashSet<Region> {
        &self.terrain_seen
    }

    pub(super) fn draws(&self) -> &[DrawCommand] {
        &self.draws
    }

    pub(super) fn draw_batches(&self) -> &[DrawBatch] {
        &self.draw_batches
    }

    pub(super) fn indirect_bytes(&self) -> &[u8] {
        &self.indirect_bytes
    }

    pub(super) fn first_instance(&self, offset: u64, command: u32) -> Option<u32> {
        let index = usize::try_from(offset).ok()?.checked_div(INDIRECT_DRAW_BYTES)?.checked_add(command as usize)?;
        self.first_instances.get(index).copied()
    }

    pub(super) fn visible_items(&self) -> usize {
        self.visible_items
    }

    pub(super) fn valid(&self) -> bool {
        self.valid
    }

    pub(super) fn has_blended(&self) -> bool {
        !self.blended.is_empty()
    }

    pub(super) fn clear(&mut self) {
        self.clear_group_counts();
        self.draws.clear();
        self.instance_bytes.clear();
        self.active_textures.clear();
        self.active_terrain.clear();
        self.blended.clear();
        self.dirty_slots.clear();
        self.opaque_draws = 0;
        self.opaque_instances = 0;
        self.instance_count = 0;
        self.visible_items = 0;
        self.omitted = 0;
        self.valid = false;
        self.batch_order.clear();
        self.draw_batches.clear();
        self.indirect_bytes.clear();
        self.first_instances.clear();
        for group in self.groups.batches.iter_mut().flatten() {
            group.commands.clear();
        }
    }

    pub(super) fn build_draw_batches(&mut self, meshes: &HashMap<MeshId, GpuMesh>, indirect_first_instance: bool) {
        for group in self.groups.batches.iter_mut().flatten() {
            group.commands.clear();
        }
        self.batch_order.clear();
        self.draw_batches.clear();
        self.indirect_bytes.clear();
        self.first_instances.clear();

        for draw in &self.draws[..self.opaque_draws] {
            let (binding, args) = ensure_batch(&mut self.groups, draw, meshes);
            let group = self.groups.batch_group_mut(binding.id).expect("interned batch group");
            if group.commands.is_empty() {
                self.batch_order.push(binding.id);
            }
            group.commands.push(args);
        }
        for id in &self.batch_order {
            let Some(group) = self.groups.batch_group(*id) else { continue };
            let offset = self.indirect_bytes.len() as u64;
            for args in &group.commands {
                self.first_instances.push(args.first_instance);
                encode_draw(args, &mut self.indirect_bytes, indirect_first_instance);
            }
            self.draw_batches.push(DrawBatch {
                id: *id,
                key: group.key,
                buffers: Arc::clone(&group.buffers),
                offset,
                count: group.commands.len() as u32,
            });
        }

        let blended_start = self.draw_batches.len();
        for draw in &self.draws[self.opaque_draws..] {
            let (binding, args) = ensure_batch(&mut self.groups, draw, meshes);
            let merge = self.draw_batches.len() > blended_start && self.draw_batches.last().is_some_and(|batch| batch.id == binding.id);
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
                let group = self.groups.batch_group(binding.id).expect("interned batch group");
                self.draw_batches.push(DrawBatch { id: binding.id, key: group.key, buffers: Arc::clone(&group.buffers), offset, count: 1 });
            }
        }
    }

    fn clear_group_counts(&mut self) {
        for group in self.active_groups.drain(..) {
            let index = group as usize;
            if let Some(count) = self.group_counts.get_mut(index) {
                *count = 0;
            }
        }
    }
}

fn write_instance(bytes: &mut [u8], slot: u32, transform: &[u8; TRANSFORM_BYTES], parameters: &[u8; PARAMETER_BYTES]) -> bool {
    let start = slot as usize * INSTANCE_BYTES;
    let Some(destination) = bytes.get_mut(start..start + INSTANCE_BYTES) else { return false };
    let (transform_destination, parameter_destination) = destination.split_at_mut(TRANSFORM_BYTES);
    transform_destination.copy_from_slice(transform);
    parameter_destination.copy_from_slice(parameters);
    true
}

fn contains_item(visible: &[ItemIndex], items: &ItemSlab, index: ItemIndex) -> bool {
    let Some(id) = items.get(index).map(|item| item.id) else { return false };
    visible.binary_search_by(|visible| items.get(*visible).map_or(std::cmp::Ordering::Less, |item| item.id.cmp(&id))).is_ok()
}

fn blend_distance(position: DVec3, rotation: Quat, scale: Vec3, center: Vec3, camera: Camera) -> f64 {
    let relative = position - camera.position() + (rotation * (center * scale)).as_dvec3();
    relative.length_squared()
}

fn ensure_batch(groups: &mut DrawInterner, draw: &DrawCommand, meshes: &HashMap<MeshId, GpuMesh>) -> (BatchBinding, DrawIndexedArgs) {
    let binding = if let Some(binding) = groups.batch(draw.group) {
        binding
    } else {
        let mesh = meshes.get(&draw.mesh).expect("draw command mesh is resident");
        let allocation = &mesh.allocation;
        let buffers = allocation.buffers();
        let key =
            DrawBatchKey { pipeline: draw.pipeline, texture: draw.texture, terrain: draw.terrain, page: Arc::as_ptr(buffers) as usize };
        let binding = BatchBinding {
            id: groups.intern_batch(key, buffers),
            index_offset: allocation.indices().start,
            base_vertex: i32::try_from(allocation.vertices().start).expect("mesh page fits the GPU budget"),
        };
        groups.set_batch(draw.group, binding);
        binding
    };
    let args = DrawIndexedArgs {
        index_count: draw.indices.end - draw.indices.start,
        instance_count: draw.count,
        first_index: binding.index_offset.checked_add(draw.indices.start).expect("mesh page index range fits the GPU budget"),
        base_vertex: binding.base_vertex,
        first_instance: draw.first,
    };
    (binding, args)
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
#[path = "draw_list_tests.rs"]
mod tests;
