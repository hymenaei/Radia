use super::{
    BindGroupDescriptor, BindGroupEntry, BindingResource, BufferDescriptor, BufferUsages, COPY_BYTES_PER_ROW_ALIGNMENT, Color,
    CommandEncoder, Device, Extent3d, GpuCharge, GpuTexture, HashMap, LoadOp, Operations, Origin3d, RenderPassColorAttachment,
    RenderPassDescriptor, StoreOp, TexelCopyBufferInfo, TexelCopyBufferLayout, TexelCopyTextureInfo, TextureDescriptor, TextureDimension,
    TextureEntry, TextureFormat, TextureUploadState, TextureUsages, TextureViewDescriptor, Textures, UPLOAD_BYTES, Upload, UploadData,
    mip_count, raw_channel_view_format, raw_channel_view_formats,
};

impl Textures {
    pub(crate) fn upload(&mut self, device: &Device, encoder: &mut CommandEncoder, epoch: u64) {
        self.releases();
        let abandoned: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| matches!(&entry.state, TextureUploadState::Preparing { life, .. } if life.upgrade().is_none()))
            .map(|(id, _)| *id)
            .collect();
        for id in abandoned {
            self.retire_entry(id, false);
        }
        self.trim(epoch);
        let prepared: Vec<_> = self
            .entries
            .iter()
            .filter_map(|(id, e)| match &e.state {
                TextureUploadState::Prepared(pixels) => Some((*id, pixels.size)),
                _ => None,
            })
            .collect();
        for (id, size) in prepared {
            let entry = self.entries.remove(&id).expect("prepared texture");
            let TextureUploadState::Prepared(pixels) = entry.state else { unreachable!("prepared texture") };
            let format = pixels.format();
            let descriptor = TextureDescriptor {
                label: Some("Scene image"),
                size: Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: mip_count(size),
                sample_count: 1,
                dimension: TextureDimension::D2,
                format,
                usage: TextureUsages::TEXTURE_BINDING
                    | TextureUsages::COPY_DST
                    | if matches!(pixels.data, UploadData::Rgba(_)) { TextureUsages::RENDER_ATTACHMENT } else { TextureUsages::empty() },
                view_formats: raw_channel_view_formats(format),
            };
            let charge = GpuCharge::new(self.memory.clone(), self.cap_bytes.clone(), &descriptor);
            let texture = device.create_texture(&descriptor);
            let view = texture.create_view(&TextureViewDescriptor::default());
            let water_view =
                texture.create_view(&TextureViewDescriptor { format: Some(raw_channel_view_format(format)), ..Default::default() });
            self.entries.insert(
                id,
                TextureEntry {
                    used: entry.used,
                    fallback: entry.fallback,
                    owner: entry.owner,
                    revision: entry.revision,
                    evicted: false,
                    geometry: entry.geometry,
                    state: TextureUploadState::Uploading {
                        gpu: GpuTexture { texture, view, water_view, bindings: HashMap::new(), charge },
                        pixels,
                        row: 0,
                        mip: 0,
                    },
                },
            );
        }
        self.uploaded = 0;
        let mut mip_work = 0;
        let mut mip_passes = 0;
        let mut ready = Vec::new();
        for (id, entry) in &mut self.entries {
            let TextureUploadState::Uploading { gpu, pixels, row, mip } = &mut entry.state else { continue };
            let [width, height] = pixels.size;
            while *mip < mip_count(pixels.size) {
                if let Some(level) = pixels.level(*mip) {
                    let pitch = (level.row_bytes as u32).next_multiple_of(COPY_BYTES_PER_ROW_ALIGNMENT);
                    let rows = ((UPLOAD_BYTES - self.uploaded) / pitch as usize).min((level.rows - *row) as usize) as u32;
                    if rows == 0 {
                        break;
                    }
                    let length = rows as usize * pitch as usize;
                    let Some(staging) = self.memory.reserve(length) else { break };
                    let buffer = device.create_buffer(&BufferDescriptor {
                        label: Some("Texture staging"),
                        size: length as u64,
                        usage: BufferUsages::MAP_WRITE | BufferUsages::COPY_SRC,
                        mapped_at_creation: true,
                    });
                    {
                        let mut mapped = buffer.get_mapped_range_mut(..).expect("new staging buffer is mapped and unborrowed");
                        let start = *row as usize * level.row_bytes;
                        let end = start + rows as usize * level.row_bytes;
                        for (index, src) in level.bytes[start..end].chunks_exact(level.row_bytes).enumerate() {
                            let offset = index * pitch as usize;
                            mapped.slice(offset..offset + src.len()).copy_from_slice(src);
                        }
                    }
                    buffer.unmap();
                    encoder.copy_buffer_to_texture(
                        TexelCopyBufferInfo {
                            buffer: &buffer,
                            layout: TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(pitch), rows_per_image: Some(rows) },
                        },
                        TexelCopyTextureInfo {
                            mip_level: *mip,
                            origin: Origin3d { x: 0, y: *row * level.block, z: 0 },
                            ..gpu.texture.as_image_copy()
                        },
                        Extent3d { width: level.width, height: rows * level.block, depth_or_array_layers: 1 },
                    );
                    self.staged.push(Upload { _buffer: buffer, _lease: staging });
                    self.uploaded += length;
                    *row += rows;
                    if *row != level.rows {
                        break;
                    }
                    *row = 0;
                    *mip += 1;
                    continue;
                }
                let texels = (width >> *mip).max(1) as usize * (height >> *mip).max(1) as usize;
                if mip_work + texels > 1_400_000 || mip_passes == 16 {
                    break;
                }
                let source = gpu.texture.create_view(&TextureViewDescriptor {
                    format: Some(TextureFormat::Rgba8Unorm),
                    base_mip_level: *mip - 1,
                    mip_level_count: Some(1),
                    ..Default::default()
                });
                let dest = gpu.texture.create_view(&TextureViewDescriptor {
                    format: Some(TextureFormat::Rgba8Unorm),
                    base_mip_level: *mip,
                    mip_level_count: Some(1),
                    ..Default::default()
                });
                let binding = device.create_bind_group(&BindGroupDescriptor {
                    label: Some("Mipmap"),
                    layout: &self.mip_layout,
                    entries: &[BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&source) }],
                });
                let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                    label: Some("Mipmap"),
                    color_attachments: &[Some(RenderPassColorAttachment {
                        view: &dest,
                        depth_slice: None,
                        resolve_target: None,
                        ops: Operations { load: LoadOp::Clear(Color::TRANSPARENT), store: StoreOp::Store },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(&self.mip_pipeline);
                pass.set_bind_group(0, &binding, &[]);
                pass.draw(0..3, 0..1);
                *mip += 1;
                mip_work += texels;
                mip_passes += 1;
            }
            if *mip == mip_count(pixels.size) {
                ready.push(*id);
            }
        }
        for id in ready {
            let entry = self.entries.remove(&id).expect("completed upload");
            let TextureUploadState::Uploading { gpu, pixels, .. } = entry.state else { unreachable!("upload completed") };
            self.pixels.push(pixels);
            if let Some(old) = entry.fallback {
                self.retired.push(old);
            }
            self.completed.push((entry.owner.clone(), entry.revision));
            self.entries.insert(
                id,
                TextureEntry {
                    used: entry.used,
                    fallback: None,
                    owner: entry.owner,
                    revision: entry.revision,
                    evicted: false,
                    geometry: entry.geometry,
                    state: TextureUploadState::Ready(gpu),
                },
            );
            self.changed(id);
        }
        self.trim(epoch);
    }
}

#[cfg(test)]
mod tests {
    use assets::{Failure, Priority, TextureHandle};
    use std::sync::atomic::Ordering;
    use wgpu::{Queue, TextureFormat};

    use super::*;
    use crate::texture::fixtures::{descriptor, loaded, loaded_fixture};
    use crate::texture::{CompressionQuality, Conversion, PixelSource, Sampler, TextureKey, allocation_bytes};
    use assets::TextureUsage;
    use futures::executor::block_on;
    use test_support::texture::{GRAY_4096_J2C, RGB_J2C, RGBA_J2C};

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
        textures.touch(handles[0].id(), 4);
        textures.soft_cap = bytes;
        let real_stats = textures.memory.gpu.swap(usize::MAX, Ordering::AcqRel);
        textures.trim(4);
        textures.memory.gpu.store(real_stats, Ordering::Release);
        assert!(
            matches!(textures.entries[&handles[0].id()].state, TextureUploadState::Ready(_)),
            "a texture used by the current cached draw list survives LRU selection"
        );
        assert!(handles[1..].iter().all(|texture| matches!(textures.entries[&texture.id()].state, TextureUploadState::Dormant)));
        assert_eq!(textures.count(), 8, "deferred resources stay in statistics before synchronization");
        submit(&mut textures, &device, &queue, 4);
        assert_eq!((textures.count(), textures.bytes()), (1, bytes));
        assert!(handles[2].image().is_none());
        textures.request(&handles[2], 5, Priority::Visible);
        let job = textures.next_conversion_job().unwrap();
        assert!(matches!(job.input, PixelSource::Decode(_)), "an evicted render-only handle reloads from its encoded source");
        assert!(textures.resident(&device, TextureKey { id: handles[2].id(), sampler: Sampler::default() }).is_none());
        textures.complete(job.run(CompressionQuality::Fast));
        submit(&mut textures, &device, &queue, 5);
        assert!(textures.resident(&device, TextureKey { id: handles[2].id(), sampler: Sampler::default() }).is_some());
        textures.soft_cap = 0;
        textures.request(&handles[2], 6, Priority::Visible);
        submit(&mut textures, &device, &queue, 6);
        assert_eq!(textures.count(), 1, "a current-frame texture may exceed the soft cap");
        drop(handles);
        textures.releases();
        assert!(textures.entries.is_empty(), "final owners retire their textures without cache eviction");
        assert_eq!(textures.count(), 1, "GPU disposal is deferred");
        submit(&mut textures, &device, &queue, 7);
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
}
