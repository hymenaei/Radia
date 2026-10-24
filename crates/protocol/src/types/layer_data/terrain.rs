use std::{
    f32::consts::{FRAC_1_SQRT_2, PI},
    iter::FusedIterator,
    sync::OnceLock,
};

use super::{Bits, Error, MAX_BYTES};

pub const PATCH_SIZE: usize = 16;
pub const PATCH_SAMPLES: usize = PATCH_SIZE * PATCH_SIZE;
pub const MAX_PATCHES: usize = 256;

#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    x: u8,
    y: u8,
    heights: Box<[f32; PATCH_SAMPLES]>,
}

impl Patch {
    pub fn coordinates(&self) -> (u8, u8) {
        (self.x, self.y)
    }

    pub fn heights(&self) -> &[f32; PATCH_SAMPLES] {
        &self.heights
    }

    pub fn into_heights(self) -> Box<[f32; PATCH_SAMPLES]> {
        self.heights
    }
}

pub struct Decoder<'a> {
    bits: Bits<'a>,
    patches: usize,
    finished: bool,
}

impl<'a> Decoder<'a> {
    pub fn new(layer_type: u8, data: &'a [u8]) -> Result<Self, Error> {
        if layer_type != b'L' {
            return Err(Error::UnsupportedLayer);
        }
        if data.len() > MAX_BYTES {
            return Err(Error::PayloadLimit);
        }
        let mut bits = Bits { data, position: 0 };
        let _stride = bits.read(16)?;
        if bits.read(8)? != PATCH_SIZE as u32 {
            return Err(Error::UnsupportedPatchSize);
        }
        if bits.read(8)? != u32::from(b'L') {
            return Err(Error::UnsupportedLayer);
        }
        Ok(Self { bits, patches: 0, finished: false })
    }

    fn patch(&mut self) -> Result<Option<Patch>, Error> {
        let quant = self.bits.read(8)?;
        if quant == 97 {
            self.finished = true;
            return Ok(None);
        }
        if self.patches == MAX_PATCHES {
            return Err(Error::PatchLimit);
        }
        self.patches += 1;
        let dc = f32::from_bits(self.bits.read(32)?);
        let range = self.bits.read(16)? as f32;
        let id = self.bits.read(10)?;
        let mut coefficients = [0.0; PATCH_SAMPLES];
        let tables = tables();
        for index in &tables.zigzag {
            if self.bits.read(1)? == 0 {
                continue;
            }
            if self.bits.read(1)? == 0 {
                break;
            }
            let negative = self.bits.read(1)? != 0;
            let magnitude = self.bits.read((quant & 15) as usize + 2)? as f32;
            let x = index % PATCH_SIZE;
            let y = index / PATCH_SIZE;
            coefficients[*index] = if negative { -magnitude } else { magnitude } * (1 + 2 * (x + y)) as f32;
        }
        if !dc.is_finite() {
            return Err(Error::NonFinite);
        }
        let scale = range / (1u32 << ((quant >> 4) + 2)) as f32;
        let offset = range * 0.5 + dc;
        let mut columns = [0.0; PATCH_SAMPLES];
        for y in 0..PATCH_SIZE {
            for x in 0..PATCH_SIZE {
                let mut value = FRAC_1_SQRT_2 * coefficients[x];
                for v in 1..PATCH_SIZE {
                    value += coefficients[v * PATCH_SIZE + x] * tables.cosines[y][v];
                }
                columns[y * PATCH_SIZE + x] = value;
            }
        }
        let mut heights = Box::new([0.0; PATCH_SAMPLES]);
        for y in 0..PATCH_SIZE {
            for x in 0..PATCH_SIZE {
                let mut value = FRAC_1_SQRT_2 * columns[y * PATCH_SIZE];
                for u in 1..PATCH_SIZE {
                    value += columns[y * PATCH_SIZE + u] * tables.cosines[x][u];
                }
                let height = value * (2.0 / PATCH_SIZE as f32) * scale + offset;
                if !height.is_finite() {
                    return Err(Error::NonFinite);
                }
                heights[y * PATCH_SIZE + x] = height;
            }
        }
        Ok(Some(Patch { x: (id >> 5) as u8, y: (id & 31) as u8, heights }))
    }
}

impl Iterator for Decoder<'_> {
    type Item = Result<Patch, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        match self.patch() {
            Ok(Some(patch)) => Some(Ok(patch)),
            Ok(None) => None,
            Err(error) => {
                self.finished = error != Error::NonFinite;
                Some(Err(error))
            }
        }
    }
}

impl FusedIterator for Decoder<'_> {}

struct Tables {
    cosines: [[f32; PATCH_SIZE]; PATCH_SIZE],
    zigzag: [usize; PATCH_SAMPLES],
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let cosines =
            std::array::from_fn(|n| std::array::from_fn(|u| ((2 * n + 1) as f32 * u as f32 * PI / (2 * PATCH_SIZE) as f32).cos()));
        let mut zigzag = [0; PATCH_SAMPLES];
        let mut index = 0;
        for diagonal in 0..(2 * PATCH_SIZE - 1) {
            for offset in 0..=diagonal {
                let (x, y) = if diagonal % 2 == 0 { (offset, diagonal - offset) } else { (diagonal - offset, offset) };
                if x < PATCH_SIZE && y < PATCH_SIZE {
                    zigzag[index] = y * PATCH_SIZE + x;
                    index += 1;
                }
            }
        }
        Tables { cosines, zigzag }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(bits: &str) -> Vec<u8> {
        let bits: Vec<_> = bits.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        assert!(bits.iter().all(|b| matches!(b, b'0' | b'1')));
        let mut data = vec![8, 1, 16, b'L'];
        data.extend(bits.chunks(8).map(|chunk| chunk.iter().enumerate().fold(0, |byte, (i, bit)| byte | ((bit - b'0') << (7 - i)))));
        data
    }

    #[test]
    fn flat_patch_preserves_coordinates() {
        let data = [8, 1, 16, b'L', 0, 0, 0, 32, 65, 20, 0, 101, 38, 16];
        let mut layer = Decoder::new(b'L', &data).unwrap();
        let patch = layer.next().unwrap().unwrap();
        assert_eq!(patch.coordinates(), (3, 5));
        assert_eq!(patch.heights(), &[20.0; PATCH_SAMPLES]);
        assert_eq!(layer.next(), None);
        assert_eq!(layer.next(), None);
    }

    #[test]
    fn frequency_vectors_preserve_orientation() {
        for (coefficients, axis, sign) in [("11001 10", None, 1.0), ("0 11001 10", Some(true), 1.0), ("0 0 11101 10", Some(false), -1.0)] {
            let data =
                payload(&format!("00000000 00000000 00000000 00000000 00000000 00001000 00000000 00000000 00 {coefficients} 01100001"));
            let mut layer = Decoder::new(b'L', &data).unwrap();
            let patch = layer.next().unwrap().unwrap();
            for y in 0..PATCH_SIZE {
                for x in 0..PATCH_SIZE {
                    let expected = match axis {
                        None => 4.125,
                        Some(horizontal) => {
                            let n = if horizontal { x } else { y };
                            4.0 + sign * 0.75 / 2.0f32.sqrt() * (((2 * n + 1) as f32 * PI) / 32.0).cos()
                        }
                    };
                    assert!((patch.heights()[y * PATCH_SIZE + x] - expected).abs() < 0.00001);
                }
            }
            assert_eq!(layer.next(), None);
        }
    }

    #[test]
    fn chunked_fields_are_little_endian() {
        let data =
            payload("00001111 00000000 00000000 00000000 00000000 00000100 00000000 00000001 11 110 01000101 00100011 1 10 01100001");
        let mut layer = Decoder::new(b'L', &data).unwrap();
        let patch = layer.next().unwrap().unwrap();
        assert_eq!(patch.coordinates(), (24, 1));
        assert!(patch.heights().iter().all(|height| (*height - (2.0 + 74565.0 / 16.0)).abs() < 0.001));
        assert_eq!(layer.next(), None);
    }

    #[test]
    fn invalid_patches_preserve_later_data() {
        let data = payload(
            "00000000 00000000 00000000 10000000 01111111 00001000 00000000 00000000 00 10
                                00000000 00000000 00000000 00000000 00000000 00001000 00000000 00000000 00 10",
        );
        let mut layer = Decoder::new(b'L', &data).unwrap();
        assert_eq!(layer.next(), Some(Err(Error::NonFinite)));
        assert_eq!(layer.next().unwrap().unwrap().heights(), &[4.0; PATCH_SAMPLES]);
        assert_eq!(layer.next(), Some(Err(Error::Truncated)));
        assert_eq!(layer.next(), None);
    }

    #[test]
    fn headers_and_work_are_bounded() {
        for (case, actual, expected) in [
            ("unsupported layer", Decoder::new(b'7', &[8, 1, 16, b'L']).map(|_| ()), Err(Error::UnsupportedLayer)),
            ("unsupported patch size", Decoder::new(b'L', &[8, 1, 32, b'L']).map(|_| ()), Err(Error::UnsupportedPatchSize)),
            ("payload limit", Decoder::new(b'L', &[0; MAX_BYTES + 1]).map(|_| ()), Err(Error::PayloadLimit)),
        ] {
            assert_eq!(actual, expected, "reject {case}");
        }
        let patch = "00000000 00000000 00000000 10000000 01111111 00001000 00000000 00000000 00 10 ";
        let data = payload(&patch.repeat(MAX_PATCHES + 1));
        let mut layer = Decoder::new(b'L', &data).unwrap();
        for _ in 0..MAX_PATCHES {
            assert_eq!(layer.next(), Some(Err(Error::NonFinite)));
        }
        assert_eq!(layer.next(), Some(Err(Error::PatchLimit)));
        assert_eq!(layer.next(), None);
    }

    proptest::proptest! {
        #[test]
        fn streams_remain_bounded_and_finite(body in proptest::collection::vec(proptest::num::u8::ANY, 0..2048)) {
            let mut data = vec![8, 1, 16, b'L'];
            data.extend(body);
            let mut layer = Decoder::new(b'L', &data).unwrap();
            let mut count = 0;
            for result in layer.by_ref() {
                count += 1;
                proptest::prop_assert!(count <= MAX_PATCHES + 1);
                if let Ok(patch) = result {
                    proptest::prop_assert!(patch.heights().iter().all(|height| height.is_finite()));
                }
            }
            proptest::prop_assert_eq!(layer.next(), None);
        }
    }
}
