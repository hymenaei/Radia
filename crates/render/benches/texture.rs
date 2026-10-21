use std::hint::black_box;

use block_compression::{BC7Settings, CompressionVariant, encode::compress_rgba8};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use intel_tex_2::{RgbaSurface, bc1, bc3, bc7};

#[path = "../src/texture_compression.rs"]
#[allow(unused_imports)]
mod texture_compression;

use texture_compression::{CompressionFormat, CompressionQuality};

fn textures(c: &mut Criterion) {
    let mut group = c.benchmark_group("texture/compression");
    for edge in [512u32, 1024, 2048] {
        let mut pixels = Vec::with_capacity(edge as usize * edge as usize * 4);
        for y in 0..edge {
            for x in 0..edge {
                pixels.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, ((x * 13 + y * 7) % 256) as u8, 255]);
            }
        }
        group.throughput(Throughput::Elements(u64::from(edge).pow(2)));
        assert!(texture_compression::supported([edge; 2]));
        black_box(texture_compression::work_bytes([edge; 2]));
        for (name, alpha, quality) in [
            ("bc1_with_mips", false, CompressionQuality::Fast),
            ("bc3_with_mips", true, CompressionQuality::Fast),
            ("bc7_opaque_with_mips", false, CompressionQuality::High),
            ("bc7_alpha_with_mips", true, CompressionQuality::High),
        ] {
            for (index, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let x = index as u32 % edge;
                let y = index as u32 / edge;
                pixel[3] = if alpha { ((x + y * 3) % 256) as u8 } else { 255 };
            }
            group.bench_function(BenchmarkId::new(name, edge), |b| {
                b.iter(|| {
                    let image = texture_compression::encode(black_box(&pixels), [edge; 2], quality, || false).unwrap();
                    black_box(image.format.gpu());
                    black_box(image.levels[0].size);
                    black_box(image.levels[0].bytes.len());
                    black_box(image);
                });
            });
        }
        for (name, variant, alpha) in [
            ("bc1", CompressionVariant::BC1, false),
            ("bc3", CompressionVariant::BC3, true),
            ("bc7_opaque_ultrafast", CompressionVariant::BC7(BC7Settings::opaque_ultra_fast()), false),
            ("bc7_alpha_ultrafast", CompressionVariant::BC7(BC7Settings::alpha_ultrafast()), true),
            ("bc7_alpha_fast", CompressionVariant::BC7(BC7Settings::alpha_fast()), true),
        ] {
            for (index, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let x = index as u32 % edge;
                let y = index as u32 / edge;
                pixel[3] = if alpha { ((x + y * 3) % 256) as u8 } else { 255 };
            }
            let mut blocks = vec![0; variant.blocks_byte_size(edge, edge)];
            group.bench_function(BenchmarkId::new(name, edge), |b| {
                b.iter(|| {
                    compress_rgba8(variant, black_box(&pixels), black_box(&mut blocks), edge, edge, edge * 4);
                    black_box(&blocks);
                });
            });
        }
        for (name, format, alpha) in [("ispc_bc1", CompressionFormat::Bc1, false), ("ispc_bc3", CompressionFormat::Bc3, true)] {
            for (index, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let x = index as u32 % edge;
                let y = index as u32 / edge;
                pixel[3] = if alpha { ((x + y * 3) % 256) as u8 } else { 255 };
            }
            let surface = RgbaSurface { data: &pixels, width: edge, height: edge, stride: edge * 4 };
            let mut blocks = vec![0; (edge / 4) as usize * (edge / 4) as usize * format.block_bytes()];
            group.bench_function(BenchmarkId::new(name, edge), |b| {
                b.iter(|| {
                    match format {
                        CompressionFormat::Bc1 => bc1::compress_blocks_into(black_box(&surface), black_box(&mut blocks)),
                        CompressionFormat::Bc3 => bc3::compress_blocks_into(black_box(&surface), black_box(&mut blocks)),
                        CompressionFormat::Bc7 => unreachable!(),
                    }
                    black_box(&blocks);
                });
            });
        }
        for (name, settings, alpha) in [
            ("ispc_bc7_opaque_ultrafast", bc7::opaque_ultra_fast_settings(), false),
            ("ispc_bc7_alpha_ultrafast", bc7::alpha_ultra_fast_settings(), true),
        ] {
            for (index, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let x = index as u32 % edge;
                let y = index as u32 / edge;
                pixel[3] = if alpha { ((x + y * 3) % 256) as u8 } else { 255 };
            }
            let surface = RgbaSurface { data: &pixels, width: edge, height: edge, stride: edge * 4 };
            let mut blocks = vec![0; bc7::calc_output_size(edge, edge)];
            group.bench_function(BenchmarkId::new(name, edge), |b| {
                b.iter(|| {
                    bc7::compress_blocks_into(&settings, black_box(&surface), black_box(&mut blocks));
                    black_box(&blocks);
                });
            });
        }
    }
    group.finish();
}

criterion_group!(benches, textures);
criterion_main!(benches);
