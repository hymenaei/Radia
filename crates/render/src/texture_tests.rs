use std::time::Instant;

use assets::{Budget, Content, Kind, Request, Resource, Status, TextureUsage};
use futures::executor::block_on;
use utils::AssetId;
use uuid::Uuid;

use super::*;
use test_support::texture::{GRAY_4096_J2C, GRAY_ALPHA_J2C, RGB_J2C, RGBA_J2C};

fn loaded(id: u128, rgba: bool) -> TextureHandle {
    let bytes: &[u8] = if rgba { RGBA_J2C } else { GRAY_ALPHA_J2C };
    loaded_fixture(id, bytes)
}

fn loaded_fixture(id: u128, bytes: &[u8]) -> TextureHandle {
    let mut store = assets::Store::new(Budget::default());
    let route = store.register_route().unwrap();
    let now = Instant::now();
    let resource = Resource { id: AssetId::new(Uuid::from_u128(id)), kind: Kind::Texture };
    let demand = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    let result = job.prepare(bytes.to_vec());
    store.complete(job, result, now);
    let Status::Ready(Content::Texture(texture)) = demand.status() else { panic!("fixture must decode") };
    texture
}

fn descriptor(size: [u32; 2], format: TextureFormat) -> TextureDescriptor<'static> {
    TextureDescriptor {
        label: None,
        size: Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        mip_level_count: mip_count(size),
        sample_count: 1,
        dimension: TextureDimension::D2,
        format,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    }
}

#[test]
fn descriptor_storage_includes_mips() {
    for (size, levels, bytes) in [([1, 1], 1, 4), ([1, 8], 4, 60), ([7, 3], 3, 100), ([2048, 2048], 12, 22_369_620)] {
        assert_eq!(mip_count(size), levels);
        assert_eq!(allocation_bytes(&descriptor(size, TextureFormat::Rgba8UnormSrgb)), Some(bytes));
    }
    for (format, bytes) in
        [(TextureFormat::Bc1RgbaUnormSrgb, 56), (TextureFormat::Bc3RgbaUnormSrgb, 112), (TextureFormat::Bc7RgbaUnormSrgb, 112)]
    {
        assert_eq!(allocation_bytes(&descriptor([8, 8], format)), Some(bytes), "block-compressed mip tails still occupy whole blocks");
    }
    let mut array = descriptor([8, 8], TextureFormat::Rgba8Unorm);
    array.size.depth_or_array_layers = 3;
    assert_eq!(allocation_bytes(&array), Some(1020));
    array.sample_count = 4;
    assert_eq!(allocation_bytes(&array), Some(4080));
}

#[test]
fn completion_retains_only_work() {
    for (cancelled, rgba) in [(false, false), (false, true), (true, true)] {
        let texture = loaded(1, rgba);
        let image = texture.image().unwrap();
        let original = image.pixels().as_ptr();
        let memory = Arc::new(Accounting::default());
        let job = TextureJob {
            id: texture.id(),
            revision: texture.info().unwrap().revision,
            texture: texture.downgrade(),
            input: PixelSource::Image(image),
            cancelled: Arc::new(AtomicBool::new(cancelled)),
            life: Arc::new(()),
            staging: memory.reserve(8 * 8 * 4).unwrap(),
            work: memory.work().unwrap(),
            compress: false,
        };
        let other = memory.work().unwrap();
        assert!(memory.work().is_none());
        let completion = job.run(CompressionQuality::Fast);
        match &completion.data {
            Conversion::Ready(pixels) => {
                assert!(!cancelled);
                let UploadData::Rgba(rgba_pixels) = &pixels.data else { panic!("compression disabled") };
                assert_eq!(rgba_pixels.bytes().len(), 8 * 8 * 4);
                if rgba {
                    assert_eq!(rgba_pixels.bytes().as_ptr(), original, "RGBA upload reuses the decoded allocation");
                }
                assert!(memory.reserve(STAGING_BYTES).is_none());
            }
            Conversion::Failed { failure, .. } => {
                assert!(cancelled);
                assert_eq!(*failure, Failure::Cancelled);
                assert!(memory.reserve(STAGING_BYTES).is_some());
            }
        }
        assert!(memory.work().is_none(), "queued completions retain admission through delivery");
        drop(completion);
        drop(other);
        assert!(memory.work().is_some());
        assert!(memory.reserve(STAGING_BYTES).is_some());
    }
}

#[test]
fn staging_release_restores_admission() {
    let memory = Arc::new(Accounting::default());
    let held = memory.reserve(STAGING_BYTES).unwrap();
    assert!(memory.reserve(1).is_none());
    assert!(memory.reserve(usize::MAX).is_none());
    drop(held);
    assert!(memory.reserve(STAGING_BYTES).is_some());
}

fn submit(textures: &mut Textures, device: &Device, queue: &Queue, epoch: u64) {
    let mut encoder = device.create_command_encoder(&Default::default());
    textures.upload(device, &mut encoder, epoch);
    queue.submit([encoder.finish()]);
    textures.submitted(queue);
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
}

fn upload(textures: &mut Textures, device: &Device, queue: &Queue, texture: &TextureHandle, epoch: u64) {
    textures.request(texture, epoch, Priority::Visible);
    let job = textures.next_conversion_job().expect("texture upload requested");
    textures.complete(job.run(CompressionQuality::Fast));
    submit(textures, device, queue, epoch);
}

#[test]
#[ignore = "requires a Vulkan adapter; run explicitly with cargo nextest run -p render --run-ignored ignored-only gpu_texture_lifetimes"]
fn gpu_texture_lifetimes() {
    let (device, queue) = block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC,
                ..Default::default()
            })
            .await
            .unwrap()
    });
    let mut textures = Textures::new(&device, &queue);
    let handles: Vec<_> = (1..=8).map(|id| loaded(id, true)).collect();
    for texture in &handles {
        upload(&mut textures, &device, &queue, texture, 1);
    }
    let bytes = allocation_bytes(&descriptor([8, 8], TextureFormat::Rgba8UnormSrgb)).unwrap();
    assert_eq!(textures.count(), 8);
    assert_eq!(textures.bytes(), 8 * bytes);
    assert!(textures.retired.is_empty(), "under the cap every texture remains resident");
    assert!(handles.iter().all(|texture| texture.image().is_none()), "render-only pixels are released after successful submission");
    textures.request(&handles[0], 2, Priority::Visible);
    textures.request(&handles[1], 3, Priority::Visible);
    textures.soft_cap = 2 * bytes;
    let real_stats = textures.memory.gpu.swap(usize::MAX, Ordering::AcqRel);
    textures.trim(3);
    textures.memory.gpu.store(real_stats, Ordering::Release);
    assert!(
        matches!(textures.entries[&handles[0].id()].state, TextureUploadState::Ready(_)),
        "the newest unused resource survives LRU selection"
    );
    assert!(matches!(textures.entries[&handles[1].id()].state, TextureUploadState::Ready(_)), "the current frame is protected");
    assert!(handles[2..].iter().all(|texture| matches!(textures.entries[&texture.id()].state, TextureUploadState::Dormant)));
    assert_eq!(textures.count(), 8, "deferred resources stay in statistics before synchronization");
    submit(&mut textures, &device, &queue, 3);
    assert_eq!((textures.count(), textures.bytes()), (2, 2 * bytes));
    assert!(handles[2].image().is_none());
    textures.request(&handles[2], 4, Priority::Visible);
    let job = textures.next_conversion_job().unwrap();
    assert!(matches!(job.input, PixelSource::Decode(_)), "an evicted render-only handle reloads from its encoded source");
    assert!(textures.resident(&device, TextureKey { id: handles[2].id(), sampler: Sampler::default() }).is_none());
    textures.complete(job.run(CompressionQuality::Fast));
    submit(&mut textures, &device, &queue, 4);
    assert!(textures.resident(&device, TextureKey { id: handles[2].id(), sampler: Sampler::default() }).is_some());
    textures.soft_cap = 0;
    textures.request(&handles[2], 5, Priority::Visible);
    submit(&mut textures, &device, &queue, 5);
    assert_eq!(textures.count(), 1, "a current-frame texture may exceed the soft cap");
    drop(handles);
    textures.releases();
    assert!(textures.entries.is_empty(), "final owners retire their textures without cache eviction");
    assert_eq!(textures.count(), 1, "GPU disposal is deferred");
    submit(&mut textures, &device, &queue, 6);
    assert_eq!((textures.count(), textures.bytes()), (0, 0), "dropping all N handles releases every world GPU texture and binding");

    let pending = loaded(99, true);
    textures.request(&pending, 7, Priority::Visible);
    let job = textures.next_conversion_job().unwrap();
    let id = pending.id();
    drop(pending);
    textures.releases();
    textures.complete(job.run(CompressionQuality::Fast));
    assert!(!textures.entries.contains_key(&id), "a late completion cannot resurrect a released handle");
    assert_eq!((textures.count(), textures.bytes()), (0, 0));

    let failed = loaded(101, true);
    textures.request(&failed, 8, Priority::Visible);
    let mut completion = textures.next_conversion_job().unwrap().run(CompressionQuality::Fast);
    let Conversion::Ready(pixels) = completion.data else { panic!("fixture must prepare") };
    completion.data = Conversion::Failed { failure: Failure::Malformed, _work: pixels._work };
    textures.complete(completion);
    assert_eq!(textures.failure(), Some(Failure::Malformed));
    assert!(failed.image().is_some(), "a failed upload must not acknowledge or release CPU pixels");
    assert_eq!((textures.count(), textures.bytes()), (0, 0));
    drop(failed);
    textures.release_resources();
    assert_eq!(textures.failure(), None);

    let retained = loaded(100, true);
    retained.set_usage(TextureUsage::MainAndRender);
    upload(&mut textures, &device, &queue, &retained, 8);
    assert!(retained.image().is_some());
    drop(retained);
    textures.releases();
    submit(&mut textures, &device, &queue, 9);
    assert_eq!((textures.count(), textures.bytes()), (0, 0));

    let live = loaded(102, true);
    upload(&mut textures, &device, &queue, &live, 10);
    let stats = textures.memory.clone();
    let pending = loaded(103, true);
    textures.request(&pending, 10, Priority::Visible);
    let late = textures.next_conversion_job().unwrap();
    drop(textures);
    assert!(matches!(late.run(CompressionQuality::Fast).data, Conversion::Failed { failure: Failure::Cancelled, .. }));
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
    assert_eq!(stats.textures.load(Ordering::Acquire), 0, "shutdown releases GPU resources while live asset handles remain valid");
    assert!(live.info().is_some());
    assert!(live.image().is_none());
    let mut recreated = Textures::new(&device, &queue);
    upload(&mut recreated, &device, &queue, &live, 11);
    assert_eq!(recreated.count(), 1, "device recreation can rebuild render-only data from source");
    drop(live);
    recreated.release_resources();
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
    assert_eq!(recreated.count(), 0, "cleanup does not require another visible frame");
    assert!(device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC), "this manual check requires BC support");
    for (id, bytes, quality, format) in [
        (110, RGB_J2C, CompressionQuality::Fast, TextureFormat::Bc1RgbaUnormSrgb),
        (111, RGBA_J2C, CompressionQuality::Fast, TextureFormat::Bc3RgbaUnormSrgb),
        (112, RGB_J2C, CompressionQuality::High, TextureFormat::Bc7RgbaUnormSrgb),
        (113, RGBA_J2C, CompressionQuality::High, TextureFormat::Bc7RgbaUnormSrgb),
    ] {
        let texture = loaded_fixture(id, bytes);
        recreated.request(&texture, 12, Priority::Visible);
        let mut job = recreated.next_conversion_job().unwrap();
        job.compress = true;
        recreated.complete(job.run(quality));
        submit(&mut recreated, &device, &queue, 12);
        let TextureUploadState::Ready(gpu) = &recreated.entries[&texture.id()].state else { panic!("all compressed mips uploaded") };
        assert_eq!(gpu.texture.format(), format);
        assert_eq!(recreated.bytes(), allocation_bytes(&descriptor([8, 8], format)).unwrap());
        assert!(texture.image().is_none());
        drop(texture);
        recreated.release_resources();
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
        assert_eq!((recreated.count(), recreated.bytes()), (0, 0), "compressed resources release after the final owner");
    }
    let texture = loaded_fixture(114, GRAY_4096_J2C);
    assert_eq!(texture.info().unwrap().size, [2048; 2]);
    upload(&mut recreated, &device, &queue, &texture, 13);
    let TextureUploadState::Ready(gpu) = &recreated.entries[&texture.id()].state else { panic!("automatic compression uploaded") };
    assert_eq!(gpu.texture.format(), TextureFormat::Bc1RgbaUnormSrgb, "eligible decoded images select compression automatically");
    drop(texture);
    recreated.release_resources();
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
    assert_eq!((recreated.count(), recreated.bytes()), (0, 0));
}
