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
