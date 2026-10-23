use assets::PixelFormat;

use super::{
    CompressionQuality, Conversion, Failure, Ordering, PixelSource, RgbaPixels, TextureCompletion, TextureJob, UploadData, UploadPixels,
    compression,
};

impl TextureJob {
    pub fn run(self, quality: CompressionQuality) -> TextureCompletion {
        let result = if self.cancelled.load(Ordering::Acquire) || !self.texture.is_alive() {
            Err(Failure::Cancelled)
        } else {
            match self.input {
                PixelSource::Image(image) => Ok(image),
                PixelSource::Decode(job) => job.run(),
            }
        };
        let result = result.and_then(|image| {
            let size = image.size();
            if image.format() == PixelFormat::Rgba8 {
                return Ok((size, RgbaPixels::Image(image)));
            }
            let mut rgba = vec![0; size[0] as usize * size[1] as usize * 4];
            let channels = image.format().channels();
            for (dest, src) in rgba.chunks_exact_mut(size[0] as usize * 4).zip(image.pixels().chunks_exact(size[0] as usize * channels)) {
                if self.cancelled.load(Ordering::Acquire) || !self.texture.is_alive() {
                    return Err(Failure::Cancelled);
                }
                expand(src, image.format(), dest);
            }
            Ok((size, RgbaPixels::Converted(rgba)))
        });
        let result = result.and_then(|(size, rgba)| {
            let data = if self.compress {
                UploadData::Bcn(compression::encode(rgba.bytes(), size, quality, || {
                    self.cancelled.load(Ordering::Acquire) || !self.texture.is_alive()
                })?)
            } else {
                UploadData::Rgba(rgba)
            };
            Ok((size, data))
        });
        let data = match result {
            Ok((size, data)) => Conversion::Ready(UploadPixels { size, data, _staging: self.staging, _work: self.work }),
            Err(failure) => Conversion::Failed { failure, _work: self.work },
        };
        TextureCompletion { id: self.id, revision: self.revision, cancelled: self.cancelled, life: self.life, data }
    }
}

fn expand(src: &[u8], format: PixelFormat, dest: &mut [u8]) {
    if format == PixelFormat::Rgba8 {
        dest.copy_from_slice(src);
        return;
    }
    for (src, dest) in src.chunks_exact(format.channels()).zip(dest.as_chunks_mut::<4>().0) {
        let pixel = match format {
            PixelFormat::Gray8 => [src[0], src[0], src[0], 255],
            PixelFormat::GrayAlpha8 => [src[0], src[0], src[0], src[1]],
            PixelFormat::Rgb8 => [src[0], src[1], src[2], 255],
            PixelFormat::Rgba8 => unreachable!("RGBA copied above"),
        };
        dest.copy_from_slice(&pixel);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, atomic::AtomicBool};

    use super::*;
    use crate::texture::fixtures::loaded;
    use crate::texture::{Accounting, STAGING_BYTES};

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
}
