use std::cell::Cell;

use block_compression::decode::{decode_block_bc1, decode_block_bc3, decode_block_bc7};

use super::*;

#[test]
fn color_and_alpha_survive() {
    for (quality, alpha) in [
        (CompressionQuality::Fast, false),
        (CompressionQuality::Fast, true),
        (CompressionQuality::High, false),
        (CompressionQuality::High, true),
    ] {
        let size = [12, 20];
        let source =
            (0..size[0] * size[1]).flat_map(|index| [255, 0, 0, if alpha && index % size[0] < 4 { 0 } else { 255 }]).collect::<Vec<_>>();
        let image = encode(&source, size, quality, || false).unwrap();
        assert_eq!(
            image.format,
            match (quality, alpha) {
                (CompressionQuality::Fast, false) => CompressionFormat::Bc1,
                (CompressionQuality::Fast, true) => CompressionFormat::Bc3,
                (CompressionQuality::High, _) => CompressionFormat::Bc7,
            }
        );
        assert_eq!(image.levels.iter().map(|level| level.size).collect::<Vec<_>>(), [[12, 20], [6, 10], [3, 5], [1, 2], [1, 1]]);
        for (mip, level) in image.levels.iter().enumerate() {
            assert_eq!(
                level.bytes.len(),
                level.size[0].div_ceil(4) as usize * level.size[1].div_ceil(4) as usize * image.format.block_bytes()
            );
            for (index, block) in level.bytes.chunks_exact(image.format.block_bytes()).enumerate() {
                let mut decoded = [0; 64];
                match image.format {
                    CompressionFormat::Bc1 => decode_block_bc1(block, &mut decoded, 16),
                    CompressionFormat::Bc3 => decode_block_bc3(block, &mut decoded, 16),
                    CompressionFormat::Bc7 => decode_block_bc7(block, &mut decoded, 16),
                }
                for pixel in decoded.as_chunks::<4>().0 {
                    assert!(pixel[0] >= 247 && pixel[1] <= 8 && pixel[2] <= 8, "red must survive padding and each mip");
                    if mip == 0 {
                        let expected = if alpha && index % 3 == 0 { 0 } else { 255 };
                        let tolerance = u8::from(image.format == CompressionFormat::Bc7);
                        assert!(pixel[3].abs_diff(expected) <= tolerance, "alpha endpoints must survive compression");
                    } else if !alpha {
                        let tolerance = u8::from(image.format == CompressionFormat::Bc7);
                        assert!(pixel[3].abs_diff(255) <= tolerance, "opaque mip tails must stay opaque within codec precision");
                    }
                }
            }
        }
    }
}

#[test]
fn mips_average_linear_color() {
    let source = [0, 0, 0, 0, 255, 255, 255, 255];
    assert_eq!(downsample(&source, [2, 1], || false).unwrap(), [188, 188, 188, 128]);
    assert_eq!(downsample(&source, [1, 2], || false).unwrap(), [188, 188, 188, 128]);
    let colored = [255, 0, 0, 0, 0, 255, 0, 255, 0, 0, 255, 10, 255, 255, 255, 246];
    assert_eq!(downsample(&colored, [2, 2], || false).unwrap(), [188, 188, 188, 128]);
    let odd = [0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255];
    assert_eq!(downsample(&odd, [3, 1], || false).unwrap(), [156, 156, 156, 255], "odd source edges contribute to the area filter");
}

#[test]
fn malformed_and_cancelled_fail() {
    for size in [[0, 4], [4, 0], [MAX_TEXTURE_EDGE + 1, 4], [4, 4]] {
        assert!(matches!(encode(&[], size, CompressionQuality::Fast, || false), Err(Failure::Malformed)));
    }
    let source = vec![255; 128 * 128 * 4];
    assert!(matches!(encode(&source, [128, 128], CompressionQuality::Fast, || true), Err(Failure::Cancelled)));
    let calls = Cell::new(0);
    let result = encode(&source, [128, 128], CompressionQuality::Fast, || {
        calls.set(calls.get() + 1);
        calls.get() == 3
    });
    assert!(matches!(result, Err(Failure::Cancelled)), "encoding checks cancellation between native stripes");
    assert!(supported([128, 4]));
    assert!(supported([132, 20]));
    for size in [[0, 128], [127, 128], [32, 32], [MAX_TEXTURE_EDGE + 4, 128]] {
        assert!(!supported(size), "{size:?} must retain RGBA");
    }
}
