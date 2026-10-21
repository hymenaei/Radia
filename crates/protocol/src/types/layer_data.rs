use std::{
    f32::consts::{FRAC_1_SQRT_2, PI},
    iter::FusedIterator,
    sync::OnceLock,
};

#[cfg(test)]
#[path = "layer_data_tests.rs"]
mod tests;

pub const PATCH_SIZE: usize = 16;
pub const PATCH_SAMPLES: usize = PATCH_SIZE * PATCH_SIZE;
pub const MAX_LAYER_PATCHES: usize = 256;
pub const MAX_LAYER_BYTES: usize = 8192;

#[derive(Clone, Debug, PartialEq)]
pub struct LandPatch {
    x: u8,
    y: u8,
    heights: Box<[f32; PATCH_SAMPLES]>,
}

impl LandPatch {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LandError {
    UnsupportedLayer,
    UnsupportedPatchSize,
    PayloadLimit,
    PatchLimit,
    Truncated,
    NonFinite,
}

pub struct LandLayer<'a> {
    bits: Bits<'a>,
    patches: usize,
    finished: bool,
}

impl<'a> LandLayer<'a> {
    pub fn new(layer_type: u8, data: &'a [u8]) -> Result<Self, LandError> {
        if layer_type != b'L' {
            return Err(LandError::UnsupportedLayer);
        }
        if data.len() > MAX_LAYER_BYTES {
            return Err(LandError::PayloadLimit);
        }
        let mut bits = Bits { data, position: 0 };
        let _stride = bits.read(16)?;
        if bits.read(8)? != PATCH_SIZE as u32 {
            return Err(LandError::UnsupportedPatchSize);
        }
        if bits.read(8)? != u32::from(b'L') {
            return Err(LandError::UnsupportedLayer);
        }
        Ok(Self { bits, patches: 0, finished: false })
    }

    fn patch(&mut self) -> Result<Option<LandPatch>, LandError> {
        let quant = self.bits.read(8)?;
        if quant == 97 {
            self.finished = true;
            return Ok(None);
        }
        if self.patches == MAX_LAYER_PATCHES {
            return Err(LandError::PatchLimit);
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
            return Err(LandError::NonFinite);
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
                    return Err(LandError::NonFinite);
                }
                heights[y * PATCH_SIZE + x] = height;
            }
        }
        Ok(Some(LandPatch { x: (id >> 5) as u8, y: (id & 31) as u8, heights }))
    }
}

impl Iterator for LandLayer<'_> {
    type Item = Result<LandPatch, LandError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        match self.patch() {
            Ok(Some(patch)) => Some(Ok(patch)),
            Ok(None) => None,
            Err(error) => {
                self.finished = error != LandError::NonFinite;
                Some(Err(error))
            }
        }
    }
}

impl FusedIterator for LandLayer<'_> {}

struct Bits<'a> {
    data: &'a [u8],
    position: usize,
}

impl Bits<'_> {
    fn read(&mut self, count: usize) -> Result<u32, LandError> {
        if self.position + count > self.data.len() * 8 {
            return Err(LandError::Truncated);
        }
        let mut value = 0;
        let mut shift = 0;
        while shift < count {
            let width = (count - shift).min(8);
            let mut byte = 0;
            for _ in 0..width {
                byte = (byte << 1) | u32::from((self.data[self.position / 8] >> (7 - self.position % 8)) & 1);
                self.position += 1;
            }
            value |= byte << shift;
            shift += width;
        }
        Ok(value)
    }
}

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
