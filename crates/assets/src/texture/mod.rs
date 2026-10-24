mod handle;
mod header;

pub use handle::{ImageInfo, TextureDecodeJob, TextureEvents, TextureHandle, TextureId, TextureUsage, WeakTexture};

use crate::{Blob, Failure, budget::Reservation};
use jpeg2k::DecodeParameters;

pub const MAX_TEXTURE_EDGE: u32 = 2048;
pub const MAX_SOURCE_EDGE: u32 = 4096;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TextureSize(u16);

impl TextureSize {
    pub fn new(edge: u32) -> Self {
        Self(edge.clamp(32, MAX_TEXTURE_EDGE).next_power_of_two() as u16)
    }

    pub const fn edge(self) -> u32 {
        self.0 as u32
    }

    pub fn discard_level(self, source: [u32; 2]) -> u8 {
        let mut edge = source[0].max(source[1]);
        let mut discard = 0;
        while edge > self.edge() {
            edge = edge.div_ceil(2);
            discard += 1;
        }
        discard
    }
}

impl Default for TextureSize {
    fn default() -> Self {
        Self(MAX_TEXTURE_EDGE as u16)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PixelFormat {
    Gray8,
    GrayAlpha8,
    Rgb8,
    Rgba8,
}

impl PixelFormat {
    pub const fn channels(self) -> usize {
        match self {
            Self::Gray8 => 1,
            Self::GrayAlpha8 => 2,
            Self::Rgb8 => 3,
            Self::Rgba8 => 4,
        }
    }

    pub const fn alpha(self) -> PixelAlpha {
        match self {
            Self::GrayAlpha8 | Self::Rgba8 => PixelAlpha::Straight,
            Self::Gray8 | Self::Rgb8 => PixelAlpha::Opaque,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PixelAlpha {
    Opaque,
    Straight,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorSpace {
    Srgb,
}

#[derive(Clone, Debug)]
pub struct Image {
    source_size: [u32; 2],
    discard: u8,
    size: [u32; 2],
    format: PixelFormat,
    pixels: Blob,
}

impl Image {
    pub const fn source_size(&self) -> [u32; 2] {
        self.source_size
    }
    pub const fn discard_level(&self) -> u8 {
        self.discard
    }
    pub const fn size(&self) -> [u32; 2] {
        self.size
    }
    pub const fn format(&self) -> PixelFormat {
        self.format
    }
    pub const fn color_space(&self) -> ColorSpace {
        ColorSpace::Srgb
    }
    pub const fn alpha(&self) -> PixelAlpha {
        self.format.alpha()
    }
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

pub(crate) struct Decoded {
    source_size: [u32; 2],
    discard: u8,
    size: [u32; 2],
    format: PixelFormat,
    pixels: Vec<u8>,
}

impl Decoded {
    pub(crate) fn finish(self, reservation: Reservation) -> Result<Image, Failure> {
        let pixels = reservation.finish(self.pixels).map_err(|_| Failure::TooLarge)?;
        Ok(Image { source_size: self.source_size, discard: self.discard, size: self.size, format: self.format, pixels })
    }
}

pub(crate) fn decode(bytes: &[u8], target: TextureSize, cancelled: impl Fn() -> bool) -> Result<Decoded, Failure> {
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(Failure::TooLarge);
    }
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    let (stream, header) = header::preflight(bytes, &cancelled)?;
    let source_size = [header.width, header.height];
    let discard = target.discard_level(source_size);
    let reduce = u32::from(discard.min(header.levels));
    let settings = DecodeParameters::new().reduce(reduce).strict(true);
    let image = jpeg2k::Image::from_bytes_with(stream, settings).map_err(|_| Failure::Malformed)?;
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    let size = [image.width(), image.height()];
    if size.contains(&0) || size[0] > MAX_SOURCE_EDGE || size[1] > MAX_SOURCE_EDGE {
        return Err(Failure::TooLarge);
    }
    let format = match header.channels {
        1 => PixelFormat::Gray8,
        2 => PixelFormat::GrayAlpha8,
        3 => PixelFormat::Rgb8,
        4 => PixelFormat::Rgba8,
        _ => unreachable!("validated components"),
    };
    let samples = usize::try_from(size[0]).ok().and_then(|width| width.checked_mul(size[1] as usize)).ok_or(Failure::TooLarge)?;
    let length = samples.checked_mul(format.channels()).filter(|bytes| *bytes <= 64 * 1024 * 1024).ok_or(Failure::TooLarge)?;
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    let components = image.components();
    if components.len() != format.channels()
        || components.iter().any(|c| c.precision() != 8 || c.is_signed() || [c.width(), c.height()] != size || c.data().len() != samples)
    {
        return Err(Failure::Malformed);
    }
    let mut pixels = vec![0; length];
    for (channel, component) in components.iter().enumerate() {
        for (row, dest) in
            component.data().chunks_exact(size[0] as usize).zip(pixels.chunks_exact_mut(size[0] as usize * format.channels()))
        {
            if cancelled() {
                return Err(Failure::Cancelled);
            }
            for (source, pixel) in row.iter().zip(dest.chunks_exact_mut(format.channels())) {
                pixel[channel] = u8::try_from(*source).map_err(|_| Failure::Malformed)?;
            }
        }
    }
    let desired = source_size.map(|edge| edge.div_ceil(1 << discard));
    if desired[0] > size[0] || desired[1] > size[1] {
        return Err(Failure::Malformed);
    }
    let pixels = if size == desired { pixels } else { downsample(&pixels, size, desired, format, &cancelled)? };
    Ok(Decoded { source_size, discard, size: desired, format, pixels })
}

fn downsample(
    pixels: &[u8],
    size: [u32; 2],
    dest: [u32; 2],
    format: PixelFormat,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<u8>, Failure> {
    let channels = format.channels();
    let mut output = vec![0; dest[0] as usize * dest[1] as usize * channels];
    for y in 0..dest[1] {
        if cancelled() {
            return Err(Failure::Cancelled);
        }
        for x in 0..dest[0] {
            let x0 = x * size[0] / dest[0];
            let x1 = ((x + 1) * size[0] / dest[0]).max(x0 + 1);
            let y0 = y * size[1] / dest[1];
            let y1 = ((y + 1) * size[1] / dest[1]).max(y0 + 1);
            let mut sum = [0u32; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let offset = (sy as usize * size[0] as usize + sx as usize) * channels;
                    for (sum, sample) in sum.iter_mut().zip(&pixels[offset..offset + channels]) {
                        *sum += u32::from(*sample);
                    }
                }
            }
            let count = (x1 - x0) * (y1 - y0);
            let offset = (y as usize * dest[0] as usize + x as usize) * channels;
            for (dest, sum) in output[offset..offset + channels].iter_mut().zip(sum) {
                *dest = ((sum + count / 2) / count) as u8;
            }
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::cell::Cell;

    use super::*;
    use crate::{Budget, budget::TEXTURE_WORK_BYTES};
    use test_support::texture::RGBA_J2C;

    #[test]
    fn detail_sizes_are_bounded() {
        for (edge, expected) in [(0, 32), (32, 32), (33, 64), (1000, 1024), (2049, 2048), (u32::MAX, 2048)] {
            assert_eq!(TextureSize::new(edge).edge(), expected);
        }
        let size = TextureSize::new(128);
        for (source, expected) in [([4096, 2048], 5), ([257, 1], 2), ([128, 64], 0), ([8, 8], 0)] {
            assert_eq!(size.discard_level(source), expected, "source {source:?}");
        }
    }

    #[test]
    fn fallback_handles_thin_and_odd() {
        let pixels = [0, 10, 20, 30, 40, 50, 60];
        assert_eq!(downsample(&pixels, [7, 1], [3, 1], PixelFormat::Gray8, || false).unwrap(), [5, 25, 50]);
        assert_eq!(downsample(&pixels, [1, 7], [1, 3], PixelFormat::Gray8, || false).unwrap(), [5, 25, 50]);
        assert_eq!(downsample(&[20, 0, 40, 100], [2, 1], [1, 1], PixelFormat::GrayAlpha8, || false).unwrap(), [30, 50]);
        assert_eq!(downsample(&pixels, [7, 1], [3, 1], PixelFormat::Gray8, || true), Err(Failure::Cancelled));
    }

    #[test]
    fn cancelled_decode_retains_work() {
        let bytes = RGBA_J2C;
        let checks = Cell::new(0);
        header::preflight(bytes, &|| {
            checks.set(checks.get() + 1);
            false
        })
        .unwrap();
        let after_codec = checks.get() + 3;
        checks.set(0);
        let budget = Budget::default();
        let reservation = budget.reserve(TEXTURE_WORK_BYTES).unwrap();
        let result = decode(bytes, TextureSize::default(), || {
            checks.set(checks.get() + 1);
            assert_eq!(budget.stats().charged_bytes, TEXTURE_WORK_BYTES);
            checks.get() == after_codec
        });
        assert_matches!(result.err(), Some(Failure::Cancelled));
        assert_eq!(checks.get(), after_codec);
        assert_eq!(budget.stats().charged_bytes, TEXTURE_WORK_BYTES);
        drop(reservation);
        assert_eq!(budget.stats().charged_bytes, 0);
    }
}
