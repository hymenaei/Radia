pub mod terrain;

pub const MAX_BYTES: usize = 8192;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    UnsupportedLayer,
    UnsupportedPatchSize,
    PayloadLimit,
    PatchLimit,
    Truncated,
    NonFinite,
}

struct Bits<'a> {
    data: &'a [u8],
    position: usize,
}

impl Bits<'_> {
    fn read(&mut self, count: usize) -> Result<u32, Error> {
        if self.position + count > self.data.len() * 8 {
            return Err(Error::Truncated);
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
