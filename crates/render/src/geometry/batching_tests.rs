use super::*;
use crate::geometry::fixtures::scene;

fn key() -> DrawKey {
    let (_scene, view, _camera) = scene(255);
    let mesh = view.objects().find_map(|object| object.mesh()).expect("fixture has geometry").1;
    DrawKey { mesh: mesh.id(), start: 0, end: 6, pipeline: Pipeline::Surface, texture: None }
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
    assert!(matches!(draws[2].pipeline, Pipeline::FlippedWinding));
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
    assert!(matches!(draws[0].pipeline, Pipeline::Surface));
    assert!(draws[1..].iter().all(|draw| matches!(draw.pipeline, Pipeline::Blend)));
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
        assert!(batches.push(DrawKey { start, end: start + 6, pipeline, ..key }, (2, 1, 0, start as u8), None, || [value; INSTANCE_BYTES]));
    }
    let mut draws = Vec::new();
    let bytes = batches.finish(&mut draws);
    assert_eq!(draws.iter().map(|draw| draw.indices.clone()).collect::<Vec<_>>(), [0..12, 12..18, 18..30]);
    assert_eq!(draws.iter().map(|draw| (draw.first, draw.count)).collect::<Vec<_>>(), [(0, 1), (1, 1), (2, 1)]);
    assert_eq!(bytes, [1, 2, 2].into_iter().flat_map(|value| [value; INSTANCE_BYTES]).collect::<Vec<_>>());
}

#[test]
fn merging_preserves_instance_order() {
    let key = key();
    let mut batches = Batches::default();
    for start in [0, 6] {
        for id in [1, 2] {
            assert!(batches.push(DrawKey { start, end: start + 6, ..key }, (2, id, 0, start as u8), None, || [7; INSTANCE_BYTES]));
        }
    }
    let mut draws = Vec::new();
    let bytes = batches.finish(&mut draws);
    assert_eq!(draws.iter().map(|draw| (draw.first, draw.count, draw.indices.clone())).collect::<Vec<_>>(), [(0, 2, 0..6), (2, 2, 6..12)]);
    assert_eq!(bytes.len(), 4 * INSTANCE_BYTES, "merging repeated instances could change coplanar triangle ordering");
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
    assert_eq!(bytes, vec![7; MAX_FACE_INSTANCES * INSTANCE_BYTES]);
}
