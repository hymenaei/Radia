use std::sync::OnceLock;

use assets::{Failure, MAX_TEXTURE_EDGE};
use intel_tex_2::{RgbaSurface, bc1, bc3, bc7};
use math::{linear_to_srgb8, srgb8_to_linear};
use wgpu::TextureFormat;

#[cfg(test)]
#[path = "texture_compression_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompressionQuality {
    #[default]
    Fast,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompressionFormat {
    Bc1,
    Bc3,
    Bc7,
}

impl CompressionFormat {
    pub(crate) fn gpu(self) -> TextureFormat {
        match self {
            Self::Bc1 => TextureFormat::Bc1RgbaUnormSrgb,
            Self::Bc3 => TextureFormat::Bc3RgbaUnormSrgb,
            Self::Bc7 => TextureFormat::Bc7RgbaUnormSrgb,
        }
    }

    pub(crate) fn block_bytes(self) -> usize {
        match self {
            Self::Bc1 => 8,
            Self::Bc3 | Self::Bc7 => 16,
        }
    }
}

pub(crate) struct Level {
    pub(crate) size: [u32; 2],
    pub(crate) bytes: Vec<u8>,
}

pub(crate) struct Image {
    pub(crate) format: CompressionFormat,
    pub(crate) levels: Vec<Level>,
}

pub(crate) fn supported(size: [u32; 2]) -> bool {
    size.into_iter().all(|edge| edge > 0 && edge <= MAX_TEXTURE_EDGE && edge.is_multiple_of(4))
        && size.into_iter().max().is_some_and(|edge| edge >= 128)
}

pub(crate) fn work_bytes(size: [u32; 2]) -> usize {
    size[0] as usize * size[1] as usize * 10 + 4096
}

pub(crate) fn encode(rgba: &[u8], size: [u32; 2], quality: CompressionQuality, cancelled: impl Fn() -> bool) -> Result<Image, Failure> {
    if size.into_iter().any(|edge| edge == 0 || edge > MAX_TEXTURE_EDGE) || rgba.len() != size[0] as usize * size[1] as usize * 4 {
        return Err(Failure::Malformed);
    }
    let opaque = rgba.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255);
    let format = select_compression_format(opaque, quality);
    let bc7_settings = if opaque { bc7::opaque_ultra_fast_settings() } else { bc7::alpha_ultra_fast_settings() };
    let mut levels = Vec::new();
    let mut current = size;
    let mut reduced = Vec::new();
    loop {
        if cancelled() {
            return Err(Failure::Cancelled);
        }
        let source = if reduced.is_empty() { rgba } else { &reduced };
        let padded_size = current.map(|edge| edge.next_multiple_of(4));
        let padded = (padded_size != current).then(|| pad(source, current, padded_size));
        let source = padded.as_deref().unwrap_or(source);
        let stride = padded_size[0] as usize * 4;
        let row_bytes = padded_size[0] as usize / 4 * format.block_bytes();
        let mut bytes = vec![0; row_bytes * (padded_size[1] as usize / 4)];
        for row in (0..padded_size[1] as usize).step_by(64) {
            if cancelled() {
                return Err(Failure::Cancelled);
            }
            let rows = (padded_size[1] as usize - row).min(64);
            let surface = RgbaSurface {
                data: &source[row * stride..(row + rows) * stride],
                width: padded_size[0],
                height: rows as u32,
                stride: stride as u32,
            };
            let blocks = &mut bytes[row / 4 * row_bytes..(row + rows) / 4 * row_bytes];
            match format {
                CompressionFormat::Bc1 => bc1::compress_blocks_into(&surface, blocks),
                CompressionFormat::Bc3 => bc3::compress_blocks_into(&surface, blocks),
                CompressionFormat::Bc7 => bc7::compress_blocks_into(&bc7_settings, &surface, blocks),
            }
        }
        levels.push(Level { size: current, bytes });
        if current == [1, 1] {
            return Ok(Image { format, levels });
        }
        let source = if reduced.is_empty() { rgba } else { &reduced };
        reduced = downsample(source, current, &cancelled)?;
        current = current.map(|edge| (edge / 2).max(1));
    }
}

fn select_compression_format(opaque: bool, quality: CompressionQuality) -> CompressionFormat {
    match (quality, opaque) {
        (CompressionQuality::Fast, true) => CompressionFormat::Bc1,
        (CompressionQuality::Fast, false) => CompressionFormat::Bc3,
        (CompressionQuality::High, _) => CompressionFormat::Bc7,
    }
}

fn pad(source: &[u8], size: [u32; 2], padded: [u32; 2]) -> Vec<u8> {
    let mut result = vec![0; padded[0] as usize * padded[1] as usize * 4];
    for y in 0..padded[1] {
        for x in 0..padded[0] {
            let from = ((y.min(size[1] - 1) * size[0] + x.min(size[0] - 1)) * 4) as usize;
            let to = ((y * padded[0] + x) * 4) as usize;
            result[to..to + 4].copy_from_slice(&source[from..from + 4]);
        }
    }
    result
}

struct ColorTables {
    linear: [f32; 256],
    srgb: [u8; 4096],
}

fn downsample(source: &[u8], size: [u32; 2], cancelled: impl Fn() -> bool) -> Result<Vec<u8>, Failure> {
    static COLORS: OnceLock<ColorTables> = OnceLock::new();
    let colors = COLORS.get_or_init(|| ColorTables {
        linear: std::array::from_fn(|index| srgb8_to_linear(index as u8)),
        srgb: std::array::from_fn(|index| linear_to_srgb8(index as f32 / 4095.0)),
    });
    let next = size.map(|edge| (edge / 2).max(1));
    if size.into_iter().all(|edge| edge == 1 || edge.is_multiple_of(2)) {
        return downsample_even(source, size, next, colors, cancelled);
    }
    let scale = [size[0] as f32 / next[0] as f32, size[1] as f32 / next[1] as f32];
    let mut result = vec![0; next[0] as usize * next[1] as usize * 4];
    for y in 0..next[1] {
        if cancelled() {
            return Err(Failure::Cancelled);
        }
        for x in 0..next[0] {
            let lo = [x as f32 * scale[0], y as f32 * scale[1]];
            let hi = [lo[0] + scale[0], lo[1] + scale[1]];
            let mut sum = [0.0; 4];
            for sy in lo[1].floor() as u32..(hi[1].ceil() as u32).min(size[1]) {
                for sx in lo[0].floor() as u32..(hi[0].ceil() as u32).min(size[0]) {
                    let weight = (hi[0].min(sx as f32 + 1.0) - lo[0].max(sx as f32)) * (hi[1].min(sy as f32 + 1.0) - lo[1].max(sy as f32));
                    let index = ((sy * size[0] + sx) * 4) as usize;
                    for channel in 0..3 {
                        sum[channel] += colors.linear[source[index + channel] as usize] * weight;
                    }
                    sum[3] += f32::from(source[index + 3]) * weight;
                }
            }
            let area = scale[0] * scale[1];
            let index = ((y * next[0] + x) * 4) as usize;
            for channel in 0..3 {
                let linear = (sum[channel] / area).clamp(0.0, 1.0);
                result[index + channel] = colors.srgb[((linear * 4095.0 + 0.5) as usize).min(4095)];
            }
            result[index + 3] = (sum[3] / area + 0.5) as u8;
        }
    }
    Ok(result)
}

fn downsample_even(
    source: &[u8],
    size: [u32; 2],
    next: [u32; 2],
    colors: &ColorTables,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<u8>, Failure> {
    let source = source.as_chunks::<4>().0;
    let width = size[0] as usize;
    let mut result = vec![0; next[0] as usize * next[1] as usize * 4];
    for (y, row) in result.chunks_exact_mut(next[0] as usize * 4).enumerate() {
        if cancelled() {
            return Err(Failure::Cancelled);
        }
        let y0 = if size[1] == 1 { 0 } else { y * 2 };
        let y1 = (y0 + 1).min(size[1] as usize - 1);
        let top = &source[y0 * width..(y0 + 1) * width];
        let bottom = &source[y1 * width..(y1 + 1) * width];
        for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x0 = if width == 1 { 0 } else { x * 2 };
            let x1 = (x0 + 1).min(width - 1);
            let [a, b, c, d] = [top[x0], top[x1], bottom[x0], bottom[x1]];
            for channel in 0..3 {
                let sum = colors.linear[a[channel] as usize]
                    + colors.linear[b[channel] as usize]
                    + colors.linear[c[channel] as usize]
                    + colors.linear[d[channel] as usize];
                pixel[channel] = colors.srgb[((sum * (4095.0 / 4.0) + 0.5) as usize).min(4095)];
            }
            pixel[3] = ((u16::from(a[3]) + u16::from(b[3]) + u16::from(c[3]) + u16::from(d[3]) + 2) / 4) as u8;
        }
    }
    Ok(result)
}
