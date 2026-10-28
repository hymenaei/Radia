use std::{collections::HashMap, ops::Range};

use crate::{
    pipeline::{INSTANCE_BYTES, Pipeline},
    scene::MeshId,
    texture::{TerrainTextureKey, TextureKey},
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
    pub terrain: Option<TerrainTextureKey>,
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
    pub terrain: Option<TerrainTextureKey>,
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
                && last.terrain == key.terrain
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
        let bytes = batches.finish(&mut draws);
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
        let bytes = batches.finish(&mut draws);
        assert_eq!(draws.iter().map(|draw| (draw.first, draw.count)).collect::<Vec<_>>(), [(0, 1), (1, 1), (2, 1), (3, 1)]);
        assert_matches!(draws[0].pipeline, Pipeline::Surface);
        for draw in &draws[1..] {
            assert_matches!(draw.pipeline, Pipeline::Blend);
        }
        assert_eq!(bytes, [40, 20, 30, 10].into_iter().flat_map(|value| [value; INSTANCE_BYTES]).collect::<Vec<_>>());
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
        let bytes = batches.finish(&mut draws);
        assert_eq!(draws.iter().map(|draw| draw.indices.clone()).collect::<Vec<_>>(), [0..12, 12..18, 18..30]);
        assert_eq!(draws.iter().map(|draw| (draw.first, draw.count)).collect::<Vec<_>>(), [(0, 1), (1, 1), (2, 1)]);
        assert_eq!(bytes, [1, 2, 2].into_iter().flat_map(|value| [value; INSTANCE_BYTES]).collect::<Vec<_>>());
    }

    #[test]
    fn adjacent_terrain_draws_keep_distinct_detail_bindings() {
        let key = key();
        let terrain = DrawKey { pipeline: Pipeline::Terrain, terrain: Some(TerrainTextureKey([None; 4])), ..key };
        let other = DrawKey { start: 6, end: 12, terrain: None, ..terrain };
        let mut batches = Batches::default();
        assert!(batches.push(terrain, (1, 1, 0, 0), None, || [0; INSTANCE_BYTES]));
        assert!(batches.push(other, (1, 1, 0, 1), None, || [0; INSTANCE_BYTES]));
        let mut draws = Vec::new();
        let _ = batches.finish(&mut draws);
        assert_eq!(draws.len(), 2);
        assert_eq!(draws[0].terrain, terrain.terrain);
        assert_eq!(draws[1].terrain, other.terrain);
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
        let bytes = batches.finish(&mut draws);
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
    fn draw_limit_allows_existing_groups() {
        let key = key();
        let order = (2, 1, 0, 0);
        let mut batches = Batches::default();
        assert!(batches.push(key, order, None, || [1; INSTANCE_BYTES]));
        for index in 1..MAX_DRAWS {
            assert!(batches.push(key, (2, index as u128, 0, 0), Some(1.0), || [2; INSTANCE_BYTES]));
        }
        assert!(!batches.push(key, order, Some(1.0), || panic!("rejected faces must not be packed")));
        assert!(!batches.push(DrawKey { end: 12, ..key }, order, None, || panic!("rejected faces must not be packed")));
        assert!(batches.push(key, order, None, || [3; INSTANCE_BYTES]));
        assert_eq!(batches.omitted(), 2);
        let mut draws = Vec::new();
        let bytes = batches.finish(&mut draws);
        assert_eq!(draws.len(), MAX_DRAWS);
        assert_eq!(draws[0].count, 2);
        assert_eq!(bytes.len(), (MAX_DRAWS + 1) * INSTANCE_BYTES);
        assert_eq!(&bytes[INSTANCE_BYTES..2 * INSTANCE_BYTES], &[3; INSTANCE_BYTES]);
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
        let bytes = batches.finish(&mut draws);
        assert_eq!(draws.len(), 1);
        assert_eq!((draws[0].first, draws[0].count), (0, MAX_FACE_INSTANCES as u32));
        assert_eq!(bytes.len(), MAX_FACE_INSTANCES * INSTANCE_BYTES);
        assert!(bytes.iter().all(|byte| *byte == 7), "accepted face bytes remain packed");
    }
}
