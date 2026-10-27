use std::sync::mpsc;

use assets::{Budget as AssetBudget, LoadJob, Priority, Request, Store};
use futures::executor::block_on;
use math::{DVec3, Quat, Vec3};
use uuid::Uuid;
use wgpu::{
    Backends, BufferDescriptor, BufferUsages, Color, DeviceDescriptor, Extent3d, InstanceDescriptor, LoadOp, MapMode, Operations, PollType,
    RenderPassColorAttachment, RenderPassDepthStencilAttachment, RenderPassDescriptor, StoreOp, TexelCopyBufferInfo, TexelCopyBufferLayout,
    TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
};
use world::{
    Appearance, FaceAppearance, FieldUpdate, Kind, ObjectObservation, OrderEvidence, ParentUpdate, Pose, Region, SculptFlags, Shape,
    SurfaceAppearance, UpdateSource, World, WorldLimits,
};

use super::*;
use crate::scene::{AssetChange, AssetRequest, Camera, ReadFailure, ReadQuery, ReadReply, ReconcileStep, Reconciler, SceneView};

fn mesh_scene() -> (Reconciler, SceneView, Camera) {
    let mut world = World::new(WorldLimits {
        max_region_registrations: 1,
        max_entities: 2048,
        max_entities_per_region: 2048,
        max_tombstones_per_region: 1,
        max_tombstones: 1,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(1),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(1),
        max_journal_entries: 16,
        max_journal_changes: 64,
        max_region_name_bytes: 64,
        max_recovery_targets: 4,
    });
    let region = Region::new(1);
    let _ = world.adopt_region(region).unwrap();
    let _ = world.patch_region(region, world::RegionInfo { origin: Some(DVec3::ZERO), ..Default::default() }).unwrap();
    let mesh_id = utils::AssetId::new(Uuid::from_u128(800));
    let red_id = utils::AssetId::new(Uuid::from_u128(801));
    let green_id = utils::AssetId::new(Uuid::from_u128(802));
    for id in 1..=2000_u32 {
        let mut object = ObjectObservation::new(
            UpdateSource::new(region, id, OrderEvidence::Ordered(u64::from(id)), 0),
            Uuid::from_u128(u128::from(id)),
            std::num::NonZeroU32::new(id).unwrap(),
            Kind::Object,
            ParentUpdate::Root,
        );
        let y = if id == 1 { 0.0 } else { 20.0 };
        object.pose = Some(Pose { position: Some(Vec3::new(10.0, y, 0.0)), rotation: Some(Quat::IDENTITY) });
        object.scale = Some(Vec3::splat(2.0));
        object.shape = FieldUpdate::Set(Shape::Mesh { asset_id: mesh_id, flags: SculptFlags::default() });
        object.appearance = FieldUpdate::Set(Appearance::Surface);
        object.surface_appearance = Some(
            SurfaceAppearance::new(
                FaceAppearance { texture: Some(red_id), color: [255, 0, 0, 255], ..FaceAppearance::default() },
                vec![(1, FaceAppearance { texture: Some(green_id), color: [0, 255, 0, 128], ..FaceAppearance::default() })],
            )
            .unwrap(),
        );
        let _ = world.observe_object(object).unwrap();
    }
    let (mut scene, mut view) = Reconciler::new(crate::scene::Budget::default());
    let camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [128, 128]).unwrap();
    scene.set_camera(camera);
    let mut now = std::time::Instant::now();

    let requested = drive_scene(&mut scene, &mut view, &world, &mut now);
    let header = requested.iter().find(|request| matches!(request.request, Request::MeshHeader(_))).copied().unwrap();
    let mesh_bytes = test_support::mesh::asset(&[2], 2);
    scene.complete_asset(header.ticket, prepared_asset(header.request, mesh_bytes.clone()));
    let requested = drive_scene(&mut scene, &mut view, &world, &mut now);
    let lod = requested
        .iter()
        .find(|request| matches!(request.request, Request::MeshLod(block) if block.lod() == assets::mesh::MeshLod::Medium))
        .copied()
        .unwrap();
    scene.complete_asset(lod.ticket, prepared_asset(lod.request, mesh_bytes));
    let requested = drive_scene(&mut scene, &mut view, &world, &mut now);
    for request in requested {
        if request.request.resource().kind == assets::Kind::Texture {
            let bytes =
                if request.request.resource().id == red_id { test_support::texture::RGB_J2C } else { test_support::texture::GRAY_J2C };
            scene.complete_asset(request.ticket, prepared_asset(request.request, bytes.to_vec()));
        }
    }
    let _ = drive_scene(&mut scene, &mut view, &world, &mut now);
    let object = view.objects().next().unwrap();
    assert_eq!(object.mesh_lod(), Some((mesh_id, assets::mesh::MeshLod::Medium)));
    let faces = object.materials().unwrap();
    assert_eq!(faces.len(), 2);
    assert_eq!(faces[0].base_color_texture.as_ref().unwrap().state().asset, red_id);
    assert_eq!(faces[1].base_color_texture.as_ref().unwrap().state().asset, green_id);
    assert_eq!(faces[0].alpha, crate::material::AlphaBehavior::Unknown);
    assert_eq!(faces[1].alpha, crate::material::AlphaBehavior::Blend);
    (scene, view, camera)
}

fn drive_scene(scene: &mut Reconciler, view: &mut SceneView, world: &World, now: &mut std::time::Instant) -> Vec<AssetRequest> {
    let mut requested = Vec::new();
    for _ in 0..20_000 {
        match scene.poll(*now) {
            ReconcileStep::Asset(AssetChange::Request(request)) => requested.push(request),
            ReconcileStep::Asset(AssetChange::Release(_)) => {}
            ReconcileStep::Read(job) => {
                let reply = match job.query() {
                    ReadQuery::Barrier => Ok(ReadReply::Barrier(world.read_barrier())),
                    ReadQuery::Objects(cursor) => world.object_list_page(*cursor, 256).map(ReadReply::Objects),
                    ReadQuery::Regions(cursor) => world.region_page(*cursor, 64).map(ReadReply::Regions),
                    ReadQuery::Details(entities) => world.object_detail_batch(entities).map(ReadReply::Details),
                    ReadQuery::Changes(cursor) => world.change_page(*cursor, 256).map(ReadReply::Changes),
                    ReadQuery::Terrain(region, cursor) => world.terrain_page(*region, *cursor, 8).map(ReadReply::Terrain),
                    ReadQuery::Environment(region) => Ok(ReadReply::Environment(world.environment(*region))),
                };
                scene.complete_read(job, reply.map_err(ReadFailure::World), *now);
            }
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => *now = until,
            ReconcileStep::Idle => return requested,
            ReconcileStep::Closed => panic!("unexpected scene closure"),
        }
    }
    panic!("mesh scene did not converge");
}

fn prepared_asset(request: Request, bytes: Vec<u8>) -> assets::Status {
    let now = std::time::Instant::now();
    let mut store = Store::new(AssetBudget::default());
    let route = store.register_route().unwrap();
    let demand = store.request(route, request, Priority::Visible, now).unwrap();
    let LoadJob::Fetch(job) = store.next_job(now).unwrap() else { panic!("fixture request unexpectedly used a cached source") };
    let input = job.complete(Ok(bytes)).unwrap();
    let completion = store.start_decode(input).unwrap().run();
    store.complete(completion, now);
    demand.status()
}

#[test]
#[ignore = "requires a Vulkan adapter"]
fn gpu_mesh_faces_and_alpha() {
    let (device, queue) = block_on(async {
        let instance =
            wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        adapter.request_device(&DeviceDescriptor::default()).await.unwrap()
    });
    let (_scene, view, camera) = mesh_scene();
    assert_eq!(view.object_count(), 2000);
    let mut geometry = GeometryRenderer::new(&device, &queue, TextureFormat::Rgba8UnormSrgb);
    let mut encoder = device.create_command_encoder(&Default::default());
    let depth = geometry.depth(&device, [128, 128]).unwrap();
    geometry.prepare(&device, &mut encoder, &view, camera, [128, 128], None);
    assert_eq!(geometry.meshes.len(), 1, "objects sharing one LOD use one GPU mesh allocation");
    assert!(geometry.memory.mesh_state().bytes <= MESH_BYTES);
    assert_eq!(geometry.stats.instances, 1, "offscreen objects do not consume GPU instances");
    assert_eq!(geometry.stats.draw_calls, 2);
    let color = device.create_texture(&TextureDescriptor {
        label: None,
        size: Extent3d { width: 128, height: 128, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8UnormSrgb,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color_view = color.create_view(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &color_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: LoadOp::Clear(Color::BLACK), store: StoreOp::Store },
            })],
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: &depth,
                depth_ops: Some(Operations { load: LoadOp::Clear(0.0), store: StoreOp::Store }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        geometry.draw(&mut pass);
    }
    let readback = device.create_buffer(&BufferDescriptor {
        label: None,
        size: 128 * 128 * 4,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        color.as_image_copy(),
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(512), rows_per_image: Some(128) },
        },
        Extent3d { width: 128, height: 128, depth_or_array_layers: 1 },
    );
    let frame = geometry.admit().unwrap();
    queue.submit([encoder.finish()]);
    geometry.submitted(frame);
    let (mapped, complete) = mpsc::channel();
    readback.slice(..).map_async(MapMode::Read, move |result| mapped.send(result).unwrap());
    device.poll(PollType::Wait { submission_index: None, timeout: None }).unwrap();
    complete.recv().unwrap().unwrap();
    let pixels = readback.get_mapped_range(..).unwrap();
    let mut red = 0;
    let mut green = 0;
    let mut green_intensity = 0_u8;
    let mut red_intensity = 0_u8;
    for pixel in pixels.as_chunks::<4>().0 {
        if pixel[0] > pixel[1].saturating_add(24) {
            red += 1;
            red_intensity = red_intensity.max(pixel[0]);
        }
        if pixel[1] > pixel[0].saturating_add(24) {
            green += 1;
            green_intensity = green_intensity.max(pixel[1]);
        }
    }
    assert!(red > 50, "the opaque face texture must render red: {red} pixels");
    assert!(green > 50, "the blended face texture must render green: {green} pixels");
    assert!(green_intensity < red_intensity, "the green face alpha must blend against the black clear color");
}
