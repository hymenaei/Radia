use std::{collections::HashMap, ops::Range};

use crate::{
    pipeline::{INSTANCE_BYTES, Pipeline},
    scene::MeshId,
    texture::TextureKey,
};

pub(super) const MAX_FACE_INSTANCES: usize = 65_536;
const MAX_DRAWS: usize = 8192;

type FaceOrder = (u8, u128, u16, u8);

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) struct DrawKey {
    pub mesh: MeshId,
    pub start: u32,
    pub end: u32,
    pub pipeline: Pipeline,
    pub texture: Option<TextureKey>,
}

impl DrawKey {
    fn command(self, first: u32, count: u32) -> DrawCommand {
        DrawCommand { mesh: self.mesh, pipeline: self.pipeline, first, count, indices: self.start..self.end, texture: self.texture }
    }
}

pub(super) struct DrawCommand {
    pub mesh: MeshId,
    pub pipeline: Pipeline,
    pub first: u32,
    pub count: u32,
    pub indices: Range<u32>,
    pub texture: Option<TextureKey>,
}

struct Group {
    first: FaceOrder,
    bytes: Vec<u8>,
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
    blended: Vec<Blended>,
    count: usize,
    omitted: usize,
}

impl Batches {
    pub fn push(
        &mut self,
        key: DrawKey,
        order: FaceOrder,
        distance_squared: Option<f64>,
        encode: impl FnOnce() -> [u8; INSTANCE_BYTES],
    ) -> bool {
        if self.count == MAX_FACE_INSTANCES
            || (self.groups.len() + self.blended.len() >= MAX_DRAWS && (distance_squared.is_some() || !self.groups.contains_key(&key)))
        {
            self.omitted += 1;
            return false;
        }
        self.count += 1;
        let bytes = encode();
        if let Some(distance_squared) = distance_squared {
            self.blended.push(Blended { distance_squared, order, key, bytes });
        } else {
            let group = self.groups.entry(key).or_insert_with(|| Group { first: order, bytes: Vec::new(), count: 0 });
            group.first = group.first.min(order);
            group.bytes.extend(bytes);
            group.count += 1;
        }
        true
    }

    pub fn omitted(&self) -> usize {
        self.omitted
    }

    pub fn finish(mut self, draws: &mut Vec<DrawCommand>) -> Vec<u8> {
        draws.clear();
        let mut bytes = Vec::with_capacity(self.count * INSTANCE_BYTES);
        let mut groups: Vec<_> = self.groups.into_iter().collect();
        groups.sort_unstable_by_key(|(_, group)| group.first);
        for (key, group) in groups {
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
                continue;
            }
            let first = (bytes.len() / INSTANCE_BYTES) as u32;
            bytes.extend(group.bytes);
            draws.push(key.command(first, group.count));
        }
        self.blended.sort_unstable_by(|a, b| b.distance_squared.total_cmp(&a.distance_squared).then_with(|| a.order.cmp(&b.order)));
        for Blended { key, bytes: instance, .. } in self.blended {
            let first = (bytes.len() / INSTANCE_BYTES) as u32;
            bytes.extend(instance);
            draws.push(key.command(first, 1));
        }
        bytes
    }
}

#[cfg(test)]
#[path = "batching_tests.rs"]
mod tests;
