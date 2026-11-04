use math::{DVec3, Quat, Vec3};
use world::Region;

use super::{FrameCache, ItemId};
use crate::geometry::{
    ShadingMode,
    instance::instance_base,
    items::{CachedItem, CachedReplacement, mesh_info},
};
use crate::scene::SharedMesh;

#[test]
fn item_order_tracks_lifecycle() {
    let (_scene, view, _) = crate::geometry::fixtures::scene(255);
    let mesh = view.objects().next().expect("fixture object").mesh().expect("fixture mesh").1.clone();
    let mut cache = FrameCache::default();
    let high = (2, 9, 0);
    let middle = (1, 9, 0);
    let low = (0, 9, 0);
    let high_index = cache.insert_item(item(high, &mesh)).expect("high item index");
    let middle_index = cache.insert_item(item(middle, &mesh)).expect("middle item index");
    let low_index = cache.insert_item(item(low, &mesh)).expect("low item index");

    assert_eq!(ordered_ids(&mut cache), vec![low, middle, high]);

    assert_eq!(cache.insert_item(item(high, &mesh)), Some(high_index));
    assert_eq!(ordered_ids(&mut cache), vec![low, middle, high]);

    assert!(cache.remove_item(middle_index).is_some());
    let earlier = (0, 1, 0);
    assert_eq!(cache.insert_item(item(earlier, &mesh)), Some(middle_index));
    assert_eq!(ordered_ids(&mut cache), vec![earlier, low, high]);

    cache.items.get_mut(high_index).expect("high item").replacement = Some(Box::new(CachedReplacement { item: item(high, &mesh) }));
    cache.replacements.insert(high_index);
    cache.replacement_visible.insert(high_index);
    cache.next_replacement_visible.insert(high_index);
    assert_eq!(cache.promote_replacement(high_index).expect("promoted item").id, high);
    assert!(cache.items.get(high_index).is_some_and(|item| item.replacement.is_none()));
    assert!(!cache.replacements.contains(&high_index));
    assert!(!cache.replacement_visible.contains(&high_index));
    assert!(!cache.next_replacement_visible.contains(&high_index));
    assert_eq!(ordered_ids(&mut cache), vec![earlier, low, high]);

    cache.clear_items();
    assert!(ordered_ids(&mut cache).is_empty());
    cache.insert_item(item(high, &mesh)).expect("recovered high item index");
    cache.insert_item(item(low, &mesh)).expect("recovered low item index");
    assert_eq!(ordered_ids(&mut cache), vec![low, high]);

    assert!(cache.items.get(low_index).is_none());
}

fn ordered_ids(cache: &mut FrameCache) -> Vec<ItemId> {
    cache.rebuild_item_order();
    cache.item_order.iter().map(|index| cache.items.get(*index).expect("ordered item exists").id).collect()
}

fn item(id: ItemId, mesh: &SharedMesh) -> CachedItem {
    let position = DVec3::ZERO;
    let rotation = Quat::IDENTITY;
    let scale = Vec3::ONE;
    CachedItem {
        id,
        entity: None,
        region: Region::new(1),
        mesh: mesh.clone(),
        info: mesh_info(mesh),
        position,
        rotation,
        scale,
        invert_normals: false,
        replacement: None,
        transform: instance_base(position, rotation, scale, false, DVec3::ZERO),
        pickable: true,
        shading: ShadingMode::Surface,
        terrain_textures: None,
        terrain_composition: None,
        surface_kind: None,
        source: None,
        faces: Vec::new(),
        animated_faces: Vec::new(),
        animation_deadline: None,
        parameters_dirty: false,
    }
}
