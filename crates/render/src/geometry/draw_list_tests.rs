use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use assets::{Budget, Content, LoadJob, Priority, Request, Resource, Sampler, Status, Store};
use math::{DVec3, Quat, Vec3};
use proptest::{prelude::*, test_runner::RngSeed};
use utils::AssetId;
use uuid::Uuid;
use world::Region;

use super::{DrawKey, DrawList};
use crate::{
    geometry::{
        ShadingMode,
        cache::FrameCache,
        instance::{instance_base, parameter_part},
        items::{CachedFace, CachedItem, ItemId, ItemIndex, ItemSlab, MeshInfo},
    },
    material::FaceParams,
    pipeline::Pipeline,
    scene::{Camera, SharedMesh},
    texture::TextureKey,
};

type OpaqueOutput = HashMap<(DrawKey, Vec<u8>), usize>;
type BlendedOutput = Vec<(DrawKey, Vec<u8>)>;

#[test]
fn stateful_draw_changes_match_fresh_layouts() {
    let operations = vec![
        (0, 0, 0, 0, 0),
        (0, 1, 0, 0, 0),
        (0, 2, 5, 0, 0),
        (0, 3, 4, 0, 0),
        (2, 0, 1, 0, 0),
        (1, 1, 0, 0, 0),
        (0, 4, 0, 0, 0),
        (3, 2, 0, 1, -1),
        (4, 0, 0, 1, 0),
        (5, 0, 0, 0, 0),
    ];
    verify_operations(&operations);
}

#[test]
fn blended_parameter_patch_does_not_sort() {
    let (_scene, view, _) = super::super::fixtures::scene(255);
    let object = view.objects().next().expect("fixture object");
    let (_, mesh) = object.mesh().expect("fixture mesh");
    let mesh = mesh.clone();
    let textures = [loaded_texture(9_100_001), loaded_texture(9_100_002)];
    let camera = camera(0.0);
    let origin = super::super::instance::scene_origin(camera.position());
    let id = item_id(0);
    let mut items = FrameCache::default();
    let mut list = DrawList::default();
    let index = items.insert_item(new_item(id, 3, 0.0, origin, &mesh, &textures, &mut list)).expect("test item index");
    list.layout(&[], &[index], &mut items.items, camera);
    if let Some(face) = items.get_by_id_mut(id).and_then(|item| item.faces.first_mut()) {
        face.parameter_bytes[0] ^= 1;
    }
    let stats = list.patch(&mut items.items, &[(index, 0)], &[], camera);
    assert_eq!(stats.faces_patched, 1);
    assert!(!stats.blended_sorted);
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 48,
        max_shrink_iters: 20_000,
        rng_seed: RngSeed::Fixed(0x8d12_2d3a_a5c7_49e1),
        ..ProptestConfig::default()
    })]

    #[test]
    fn randomized_updates_match_fresh_layouts(
        operations in prop::collection::vec((0u8..6, 0u8..8, 0u8..8, -3i8..4, -3i8..4), 1..80)
    ) {
        let generated = operations;
        let mut operations = vec![
            (0, 0, 0, 0, 0),
            (0, 1, 3, 0, 0),
            (0, 2, 5, 0, 0),
            (2, 0, 1, 0, 0),
            (3, 1, 0, 1, -1),
            (4, 0, 0, 1, 0),
            (5, 0, 0, 0, 0),
            (1, 1, 0, 0, 0),
        ];
        operations.extend(generated);
        verify_operations(&operations);
    }
}

fn verify_operations(operations: &[(u8, u8, u8, i8, i8)]) {
    let (_scene, view, _) = super::super::fixtures::scene(255);
    let first_object = view.objects().next().expect("fixture object");
    let (_, mesh) = first_object.mesh().expect("fixture mesh");
    let mesh = mesh.clone();
    let textures = [loaded_texture(9_100_001), loaded_texture(9_100_002)];
    let camera_x = 0.0;
    let current_camera = camera(camera_x);
    let origin = super::super::instance::scene_origin(current_camera.position());
    let mut items = FrameCache::default();
    let mut list = DrawList::default();
    let mut visible = visible_items(&items.items, camera_x);
    list.layout(&[], &visible, &mut items.items, current_camera);
    assert_matches_fresh(&list, &items.items, &visible, current_camera);
    let mut camera_x = camera_x;
    let mut origin = origin;

    for (kind, item_index, style, delta_x, delta_y) in operations {
        let id = item_id(*item_index);
        let current_camera = camera(camera_x);
        let previous_visible = visible.clone();
        let mut camera_changed = false;
        let mut layout = false;
        let mut blended_patch = false;
        match kind % 6 {
            0 => {
                if items.index(id).is_none() {
                    items.insert_item(new_item(id, *style, camera_x, origin, &mesh, &textures, &mut list)).expect("test item index");
                    layout = true;
                }
            }
            1 => {
                if let Some(index) = items.index(id)
                    && let Some(item) = items.remove_item(index)
                {
                    list.release_faces(&item.faces);
                    layout = true;
                }
            }
            2 => {
                if let Some(item) = items.get_by_id_mut(id) {
                    list.release_faces(&item.faces);
                    item.faces = vec![new_face(id, *style, &item.mesh, item.info.centers[0], item.scale, &textures, &mut list)];
                    layout = true;
                }
            }
            3 => {
                let changed = if let Some(item) = items.get_by_id_mut(id) {
                    item.position.x += f64::from(*delta_x);
                    item.position.y += f64::from(*delta_y);
                    item.rotation = Quat::from_rotation_y(f32::from(*delta_x) * 0.025);
                    item.transform = instance_base(item.position, item.rotation, item.scale, origin);
                    true
                } else {
                    false
                };
                if changed {
                    let index = items.index(id).expect("updated test item");
                    let stats = list.patch(&mut items.items, &[(index, 0)], &[index], current_camera);
                    blended_patch = stats.blended_sorted;
                }
            }
            4 => {
                camera_x += f64::from(*delta_x);
                camera_changed = true;
                let next_origin = super::super::instance::scene_origin(camera(camera_x).position());
                if next_origin != origin {
                    origin = next_origin;
                    for (_, item) in items.items.iter_mut() {
                        item.transform = instance_base(item.position, item.rotation, item.scale, origin);
                    }
                    layout = true;
                }
            }
            _ => {
                camera_x = (camera_x / 256.0).floor() * 256.0 + 256.0;
                camera_changed = true;
                origin = super::super::instance::scene_origin(camera(camera_x).position());
                for (_, item) in items.items.iter_mut() {
                    item.transform = instance_base(item.position, item.rotation, item.scale, origin);
                }
                layout = true;
            }
        }

        let camera = camera(camera_x);
        let next_visible = visible_items(&items.items, camera_x);
        layout |= previous_visible != next_visible;
        if layout {
            list.layout(&previous_visible, &next_visible, &mut items.items, camera);
        } else if camera_changed && list.has_blended() {
            list.sort_blended(&mut items.items, camera);
        } else if blended_patch {
            list.reorder_blended(&mut items.items);
        }
        visible = next_visible;
        assert_matches_fresh(&list, &items.items, &visible, camera);
    }
}

fn new_item(
    id: ItemId,
    style: u8,
    camera_x: f64,
    origin: DVec3,
    mesh: &SharedMesh,
    textures: &[assets::TextureHandle; 2],
    list: &mut DrawList,
) -> CachedItem {
    let scale = Vec3::ONE;
    let position = DVec3::new(camera_x + (id.1 % 3) as f64 - 1.0, (id.1 % 2) as f64, 0.0);
    let rotation = Quat::IDENTITY;
    let bounds = mesh.mesh().bounds();
    let center = bounds.min * 0.5 + bounds.max * 0.5;
    let info = Arc::new(MeshInfo { bounds, centers: vec![center] });
    let transform = instance_base(position, rotation, scale, origin);
    let face = new_face(id, style, mesh, center, scale, textures, list);
    CachedItem {
        id,
        entity: None,
        region: Region::new(1),
        mesh: mesh.clone(),
        info,
        position,
        rotation,
        scale,
        transform,
        pickable: true,
        shading: ShadingMode::Studio,
        terrain_textures: None,
        terrain_composition: None,
        surface_kind: None,
        source: None,
        faces: vec![face],
        animated_faces: Vec::new(),
        animation_deadline: None,
        parameters_dirty: false,
    }
}

fn new_face(
    id: ItemId,
    style: u8,
    mesh: &SharedMesh,
    center: Vec3,
    scale: Vec3,
    textures: &[assets::TextureHandle; 2],
    list: &mut DrawList,
) -> CachedFace {
    let mesh_face = mesh.mesh().faces().first().expect("fixture mesh face");
    let (pipeline, texture_index, blend, double_sided): (Pipeline, Option<usize>, bool, bool) = match style % 6 {
        0 => (Pipeline::Surface, None, false, false),
        1 => (Pipeline::Surface, Some(0), false, false),
        2 => (Pipeline::DoubleSided, Some(1), false, true),
        3 => (Pipeline::Blend, None, true, false),
        4 => (Pipeline::BlendDoubleSided, Some(0), true, true),
        _ => (Pipeline::BlendFlipped, Some(1), true, false),
    };
    let texture = texture_index.map(|index| textures[index].clone());
    let mut params = FaceParams::new(None);
    params.blend = blend;
    params.double_sided = double_sided;
    params.texture = texture.as_ref().map(|handle| TextureKey { id: handle.id(), sampler: Sampler::default() });
    params.color = [0.3 + f32::from(style) * 0.05, 0.4, 0.5, 1.0];
    let key = DrawKey {
        mesh: mesh.id(),
        start: mesh_face.indices.start,
        end: mesh_face.indices.end,
        pipeline,
        texture: params.texture,
        terrain: None,
    };
    let group = list.intern_face(key, texture.as_ref(), None).expect("test draw group ID");
    CachedFace {
        order: (id.0, id.1, id.2, mesh_face.slot),
        indices: mesh_face.indices.clone(),
        center,
        base_params: params,
        params,
        texture,
        material_index: None,
        texture_entry_transform: Default::default(),
        group,
        blended: key.blended(),
        parameter_bytes: parameter_part(&params, scale),
        slot: None,
        blended_index: None,
    }
}

fn assert_matches_fresh(list: &DrawList, items: &ItemSlab, visible: &[ItemIndex], camera: Camera) {
    let expected = fresh_output(list, items, visible, camera);
    let mut opaque = OpaqueOutput::new();
    let mut blended = BlendedOutput::new();
    let mut slots = HashSet::new();
    for (index, draw) in list.draws.iter().enumerate() {
        let key = DrawKey {
            mesh: draw.mesh,
            start: draw.indices.start,
            end: draw.indices.end,
            pipeline: draw.pipeline,
            texture: draw.texture,
            terrain: draw.terrain,
        };
        for slot in draw.first..draw.first.saturating_add(draw.count) {
            assert!(slots.insert(slot), "each draw command covers a unique instance slot");
            let bytes = instance_bytes(list, slot);
            if index < list.opaque_draws {
                *opaque.entry((key, bytes)).or_default() += 1;
            } else {
                blended.push((key, bytes));
            }
        }
    }
    let expected_slots =
        items.iter().filter(|(index, _)| contains_item(visible, items, *index)).map(|(_, item)| item.faces.len()).sum::<usize>();
    assert_eq!(slots.len(), expected_slots, "every visible face has one draw slot");
    assert_eq!(list.instance_count, expected_slots, "instance output contains only visible face bytes");
    assert!(opaque == expected.0, "opaque draw key and packed byte multiset differs from a fresh layout");
    assert!(blended == expected.1, "blended draw keys or back-to-front packed byte order differs from a fresh layout");
    for (index, item) in items.iter() {
        for face in &item.faces {
            assert_eq!(face.slot.is_some(), contains_item(visible, items, index), "hidden items cannot retain instance slots");
        }
    }
}

fn fresh_output(list: &DrawList, items: &ItemSlab, visible: &[ItemIndex], camera: Camera) -> (OpaqueOutput, BlendedOutput) {
    let mut opaque = OpaqueOutput::new();
    let mut blended = Vec::new();
    for id in visible {
        let Some(item) = items.get(*id) else { continue };
        for face in &item.faces {
            let Some(key) = list.face_key(face.group) else { continue };
            let bytes = instance_from_parts(&item.transform, &face.parameter_bytes);
            if key.blended() {
                let relative = item.position - camera.position() + (item.rotation * (face.center * item.scale)).as_dvec3();
                blended.push((relative.length_squared(), face.order, key, bytes));
            } else {
                *opaque.entry((key, bytes)).or_default() += 1;
            }
        }
    }
    blended.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    (opaque, blended.into_iter().map(|(_, _, key, bytes)| (key, bytes)).collect())
}

fn instance_bytes(list: &DrawList, slot: u32) -> Vec<u8> {
    let start = slot as usize * crate::pipeline::INSTANCE_BYTES;
    let end = start + crate::pipeline::INSTANCE_BYTES;
    list.instance_bytes.get(start..end).expect("draw slot has packed instance bytes").to_vec()
}

fn instance_from_parts(
    transform: &[u8; super::super::TRANSFORM_BYTES],
    parameters: &[u8; super::super::instance::PARAMETER_BYTES],
) -> Vec<u8> {
    transform.iter().chain(parameters).copied().collect()
}

fn visible_items(items: &ItemSlab, camera_x: f64) -> Vec<ItemIndex> {
    let mut visible =
        items.iter().filter(|(_, item)| (item.position.x - camera_x).abs() <= 6.0).map(|(index, _)| index).collect::<Vec<_>>();
    visible.sort_unstable_by_key(|index| items.get(*index).map(|item| item.id));
    visible
}

fn contains_item(visible: &[ItemIndex], items: &ItemSlab, index: ItemIndex) -> bool {
    let Some(id) = items.get(index).map(|item| item.id) else { return false };
    visible.binary_search_by(|visible| items.get(*visible).map_or(std::cmp::Ordering::Less, |item| item.id.cmp(&id))).is_ok()
}

fn camera(camera_x: f64) -> Camera {
    Camera::new(DVec3::new(camera_x, 0.0, 0.0), Vec3::X, 1.0, [128, 128]).expect("test camera")
}

fn item_id(index: u8) -> ItemId {
    (2, u128::from(index), 0)
}

fn loaded_texture(id: u128) -> assets::TextureHandle {
    let mut store = Store::new(Budget::default());
    let route = store.register_route().expect("test asset route");
    let now = Instant::now();
    let resource = Resource { id: AssetId::new(Uuid::from_u128(id)), kind: assets::Kind::Texture };
    let demand = store.request(route, Request::new(resource), Priority::Visible, now).expect("test texture request");
    let LoadJob::Fetch(fetch) = store.next_job(now).expect("fresh test texture fetch") else {
        panic!("fresh test texture used a cache entry")
    };
    let input = fetch.complete(Ok(test_support::texture::GRAY_ALPHA_J2C.to_vec())).expect("test texture response");
    let completion = store.start_decode(input).expect("test decode admission").run();
    store.complete(completion, now);
    let Status::Ready(Content::Texture(texture)) = demand.status() else { panic!("test texture did not decode") };
    texture
}
