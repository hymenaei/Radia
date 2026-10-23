use super::MAX_SOURCE_EDGE;
use crate::Failure;

#[derive(Clone, Copy)]
pub(super) struct Header {
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub levels: u8,
    tiles: u32,
}

fn short(bytes: &[u8], offset: usize) -> Result<u16, Failure> {
    let bytes = bytes.get(offset..offset + 2).ok_or(Failure::Malformed)?;
    Ok(u16::from_be_bytes(bytes.try_into().expect("two bytes")))
}

fn word(bytes: &[u8], offset: usize) -> Result<u32, Failure> {
    let bytes = bytes.get(offset..offset + 4).ok_or(Failure::Malformed)?;
    Ok(u32::from_be_bytes(bytes.try_into().expect("four bytes")))
}

fn size(bytes: &[u8]) -> Result<Header, Failure> {
    if !bytes.starts_with(b"\xff\x4f\xff\x51") {
        return Err(Failure::Malformed);
    }
    let channels = short(bytes, 40)?;
    if !(1..=4).contains(&channels) {
        return Err(Failure::Unsupported);
    }
    if short(bytes, 4)? != 38 + 3 * channels {
        return Err(Failure::Malformed);
    }
    let width = word(bytes, 8)?;
    let height = word(bytes, 12)?;
    if width == 0 || height == 0 {
        return Err(Failure::Malformed);
    }
    if width > MAX_SOURCE_EDGE || height > MAX_SOURCE_EDGE {
        return Err(Failure::TooLarge);
    }
    for offset in [16, 20, 32, 36] {
        if word(bytes, offset)? != 0 {
            return Err(Failure::Unsupported);
        }
    }
    let tile_width = word(bytes, 24)?;
    let tile_height = word(bytes, 28)?;
    if tile_width == 0 || tile_height == 0 {
        return Err(Failure::Malformed);
    }
    if tile_width > MAX_SOURCE_EDGE || tile_height > MAX_SOURCE_EDGE {
        return Err(Failure::TooLarge);
    }
    let tiles = width.div_ceil(tile_width) * height.div_ceil(tile_height);
    if tiles > 256 {
        return Err(Failure::TooLarge);
    }
    for channel in 0..usize::from(channels) {
        if bytes.get(42 + 3 * channel..45 + 3 * channel).ok_or(Failure::Malformed)? != [7, 1, 1] {
            return Err(Failure::Unsupported);
        }
    }
    Ok(Header { width, height, channels: channels as u8, levels: 0, tiles })
}

fn coding(header: Header, bytes: &[u8], layers: u32, flags: u8) -> Result<(), Failure> {
    if bytes.len() < 5 {
        return Err(Failure::Malformed);
    }
    let levels = u32::from(bytes[0]);
    let block_x = u32::from(bytes[1]) + 2;
    let block_y = u32::from(bytes[2]) + 2;
    if levels > 12 || layers > 32 {
        return Err(Failure::TooLarge);
    }
    if layers == 0 || block_x > 10 || block_y > 10 || block_x + block_y > 12 {
        return Err(Failure::Malformed);
    }
    let mut blocks = 0u64;
    let mut precincts = 0u64;
    for level in 0..=levels {
        let shift = levels.saturating_sub(level) + u32::from(level != 0);
        let width = header.width.div_ceil(1 << shift);
        let height = header.height.div_ceil(1 << shift);
        let bands = if level == 0 { 1 } else { 3 };
        blocks += u64::from((width.div_ceil(1 << block_x) + 1) * (height.div_ceil(1 << block_y) + 1) * bands);
        let exponent = if flags & 1 != 0 { *bytes.get(5 + level as usize).ok_or(Failure::Malformed)? } else { 0xff };
        let x = u32::from(exponent & 15);
        let y = u32::from(exponent >> 4);
        precincts += u64::from(header.width.div_ceil(1 << (levels - level)).div_ceil(1 << x))
            * u64::from(header.height.div_ceil(1 << (levels - level)).div_ceil(1 << y));
    }
    blocks *= u64::from(header.channels);
    precincts *= u64::from(header.channels);
    if blocks > 65_536 || blocks * u64::from(layers) > 262_144 || precincts * u64::from(layers) > 65_536 {
        return Err(Failure::TooLarge);
    }
    Ok(())
}

fn validate(stream: &[u8], header: Header, cancelled: &impl Fn() -> bool) -> Result<u8, Failure> {
    let mut offset = 2;
    let mut tile_end = None;
    let mut parts = [0u16; 256];
    let mut totals = [0u8; 256];
    let mut markers = 0;
    let mut header_bytes = 0;
    let mut layers: Option<u32> = None;
    let mut levels: Option<u8> = None;
    while offset < stream.len() {
        if cancelled() {
            return Err(Failure::Cancelled);
        }
        markers += 1;
        if markers > 4096 || header_bytes > 512 * 1024 {
            return Err(Failure::TooLarge);
        }
        let marker = short(stream, offset)?;
        if marker == 0xffd9 {
            if tile_end.is_some() || offset + 2 != stream.len() || parts[..header.tiles as usize].contains(&0) {
                return Err(Failure::Malformed);
            }
            if parts.iter().zip(totals).any(|(parts, total)| total != 0 && *parts != u16::from(total)) {
                return Err(Failure::Malformed);
            }
            return levels.ok_or(Failure::Malformed);
        }
        if marker == 0xff93 {
            let end = tile_end.take().ok_or(Failure::Malformed)?;
            if offset + 2 > end {
                return Err(Failure::Malformed);
            }
            offset = end;
            continue;
        }
        let length = usize::from(short(stream, offset + 2)?);
        if length < 2 {
            return Err(Failure::Malformed);
        }
        let end = offset.checked_add(2 + length).ok_or(Failure::TooLarge)?;
        let data = stream.get(offset + 4..end).ok_or(Failure::Malformed)?;
        if tile_end.is_some_and(|tile_end| end > tile_end) {
            return Err(Failure::Malformed);
        }
        header_bytes += length + 2;
        match marker {
            0xff51 if offset == 2 => {}
            0xff90 => {
                if tile_end.is_some() || data.len() != 8 {
                    return Err(Failure::Malformed);
                }
                let tile = usize::from(short(data, 0)?);
                if tile >= header.tiles as usize || u16::from(data[6]) != parts[tile] {
                    return Err(Failure::Malformed);
                }
                parts[tile] += 1;
                if parts[tile] > 32 {
                    return Err(Failure::TooLarge);
                }
                if data[7] != 0 {
                    if (totals[tile] != 0 && totals[tile] != data[7]) || parts[tile] > u16::from(data[7]) {
                        return Err(Failure::Malformed);
                    }
                    totals[tile] = data[7];
                }
                let length = word(data, 2)? as usize;
                let end = if length == 0 {
                    stream.len().checked_sub(2).ok_or(Failure::Malformed)?
                } else {
                    offset.checked_add(length).filter(|end| *end <= stream.len()).ok_or(Failure::Malformed)?
                };
                if end < offset + 14 {
                    return Err(Failure::Malformed);
                }
                tile_end = Some(end);
            }
            0xff52 => {
                if data.len() < 10 {
                    return Err(Failure::Malformed);
                }
                let count = u32::from(short(data, 2)?);
                coding(header, &data[5..], count, data[0])?;
                levels = Some(levels.map_or(data[5], |levels| levels.min(data[5])));
                layers = Some(layers.map_or(count, |layers| layers.max(count)));
            }
            0xff53 => {
                if data.len() < 7 || data[0] >= header.channels {
                    return Err(Failure::Malformed);
                }
                coding(header, &data[2..], layers.unwrap_or(32), data[1])?;
                levels = Some(levels.map_or(data[2], |levels| levels.min(data[2])));
            }
            0xff5f..=0xff61 => return Err(Failure::Unsupported),
            0xff55 | 0xff57 | 0xff58 | 0xff5c | 0xff5d | 0xff5e | 0xff63 | 0xff64 => {}
            _ => return Err(Failure::Unsupported),
        }
        offset = end;
    }
    Err(Failure::Malformed)
}

fn boxes<'a>(mut bytes: &'a [u8], mut visit: impl FnMut(&[u8], &'a [u8]) -> Result<(), Failure>) -> Result<(), Failure> {
    let mut count = 0;
    while !bytes.is_empty() {
        count += 1;
        if count > 64 {
            return Err(Failure::TooLarge);
        }
        let length = word(bytes, 0)?;
        let tag = bytes.get(4..8).ok_or(Failure::Malformed)?;
        let (length, start) = match length {
            0 => (bytes.len(), 8),
            1 => {
                let long = bytes.get(8..16).ok_or(Failure::Malformed)?;
                (usize::try_from(u64::from_be_bytes(long.try_into().expect("eight bytes"))).map_err(|_| Failure::TooLarge)?, 16)
            }
            _ => (length as usize, 8),
        };
        if length < start {
            return Err(Failure::Malformed);
        }
        visit(tag, bytes.get(start..length).ok_or(Failure::Malformed)?)?;
        bytes = &bytes[length..];
    }
    Ok(())
}

fn jp2(bytes: &[u8]) -> Result<&[u8], Failure> {
    if !bytes.starts_with(b"\0\0\0\x0cjP  \r\n\x87\n") {
        return Err(Failure::Unsupported);
    }
    let mut codestream = None;
    let mut image_header = None;
    let mut color = None;
    let mut definitions = None;
    let mut brand = false;
    boxes(bytes, |tag, data| {
        match tag {
            b"ftyp" => {
                if data.get(..4) != Some(b"jp2 ") {
                    return Err(Failure::Unsupported);
                }
                brand = true;
            }
            b"jp2c" => {
                if codestream.replace(data).is_some() {
                    return Err(Failure::Unsupported);
                }
            }
            b"jp2h" => boxes(data, |tag, data| {
                match tag {
                    b"ihdr" => {
                        if image_header.replace(data).is_some() || data.len() != 14 {
                            return Err(Failure::Malformed);
                        }
                    }
                    b"colr" => {
                        if color.is_some() || data.len() != 7 || data[0] != 1 {
                            return Err(Failure::Unsupported);
                        }
                        color = Some(word(data, 3)?);
                    }
                    b"cdef" => {
                        if definitions.replace(data).is_some() {
                            return Err(Failure::Malformed);
                        }
                    }
                    b"pclr" | b"cmap" | b"bpcc" => return Err(Failure::Unsupported),
                    _ => {}
                }
                Ok(())
            })?,
            _ => {}
        }
        Ok(())
    })?;
    let stream = codestream.ok_or(Failure::Malformed)?;
    let header = size(stream)?;
    let image = image_header.ok_or(Failure::Malformed)?;
    if !brand
        || word(image, 0)? != header.height
        || word(image, 4)? != header.width
        || short(image, 8)? != u16::from(header.channels)
        || image[10] != 7
        || image[11] != 7
    {
        return Err(Failure::Malformed);
    }
    match (color, header.channels) {
        (Some(16), 3 | 4) | (Some(17), 1 | 2) => {}
        _ => return Err(Failure::Unsupported),
    }
    if let Some(definitions) = definitions {
        let count = usize::from(short(definitions, 0)?);
        if count != usize::from(header.channels) || definitions.len() != 2 + count * 6 {
            return Err(Failure::Unsupported);
        }
        for channel in 0..count {
            let offset = 2 + channel * 6;
            let alpha = count.is_multiple_of(2) && channel == count - 1;
            if usize::from(short(definitions, offset)?) != channel
                || short(definitions, offset + 2)? != u16::from(alpha)
                || usize::from(short(definitions, offset + 4)?) != if alpha { 0 } else { channel + 1 }
            {
                return Err(Failure::Unsupported);
            }
        }
    }
    Ok(stream)
}

pub(super) fn preflight<'a>(bytes: &'a [u8], cancelled: &impl Fn() -> bool) -> Result<(&'a [u8], Header), Failure> {
    let stream = if bytes.starts_with(b"\xff\x4f") { bytes } else { jp2(bytes)? };
    let mut header = size(stream)?;
    header.levels = validate(stream, header, cancelled)?;
    Ok((stream, header))
}
