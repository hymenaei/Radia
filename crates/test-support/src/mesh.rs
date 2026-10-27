use std::io::Write;

use flate2::{Compression, write::ZlibEncoder};

pub fn asset(lods: &[usize], face_count: usize) -> Vec<u8> {
    let block = block(face_count);
    let fields = ["lowest_lod", "low_lod", "medium_lod", "high_lod"];
    let mut offset = 0_usize;
    let lod_entries: Vec<_> = fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let (block_offset, size) = if lods.contains(&index) {
                let block_offset = i32::try_from(offset).expect("mesh fixture offset fits the wire format");
                offset += block.len();
                (block_offset, i32::try_from(block.len()).expect("mesh fixture block fits the wire format"))
            } else {
                (-1, 0)
            };
            (*field, entry(block_offset, size))
        })
        .collect();
    let mut entries = vec![("version", integer(1))];
    entries.extend(lod_entries);
    entries.push(("physics_mesh", entry(-1, 0)));
    entries.push(("skin", entry(-1, 0)));
    let mut bytes = b"<?llsd/binary?>\n".to_vec();
    bytes.extend(map(&entries));
    for index in lods {
        if *index < fields.len() {
            bytes.extend_from_slice(&block);
        }
    }
    bytes
}

fn block(face_count: usize) -> Vec<u8> {
    let mut faces = Vec::with_capacity(face_count);
    for index in 0..face_count {
        let min_y = -1.0 + 2.0 * index as f64 / face_count as f64;
        let max_y = -1.0 + 2.0 * (index + 1) as f64 / face_count as f64;
        let position = [0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00, 0xff, 0xff, 0x80, 0x00, 0xff, 0xff, 0x00, 0x00];
        let uv = [0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff];
        let triangles = [0x00, 0x00, 0x01, 0x00, 0x02, 0x00];
        let domain = map(&[("Min", array(&[real(-0.01), real(min_y), real(-1.0)])), ("Max", array(&[real(0.01), real(max_y), real(1.0)]))]);
        let uv_domain = map(&[("Min", array(&[real(0.0), real(0.0)])), ("Max", array(&[real(1.0), real(1.0)]))]);
        faces.push(map(&[
            ("Position", binary(&position)),
            ("PositionDomain", domain),
            ("TexCoord0", binary(&uv)),
            ("TexCoord0Domain", uv_domain),
            ("TriangleList", binary(&triangles)),
        ]));
    }
    let mut payload = b"<?llsd/binary?>\n".to_vec();
    payload.extend(array(&faces));
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&payload).expect("mesh fixture compression accepts bounded bytes");
    encoder.finish().expect("mesh fixture compression finishes")
}

fn entry(offset: i32, size: i32) -> Vec<u8> {
    map(&[("offset", integer(offset)), ("size", integer(size))])
}

fn integer(value: i32) -> Vec<u8> {
    let mut bytes = vec![b'i'];
    bytes.extend_from_slice(&value.to_be_bytes());
    bytes
}

fn real(value: f64) -> Vec<u8> {
    let mut bytes = vec![b'r'];
    bytes.extend_from_slice(&value.to_be_bytes());
    bytes
}

fn binary(value: &[u8]) -> Vec<u8> {
    let mut bytes = vec![b'b'];
    bytes.extend_from_slice(&u32::try_from(value.len()).expect("mesh fixture binary size fits the wire format").to_be_bytes());
    bytes.extend_from_slice(value);
    bytes
}

fn array(values: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = vec![b'['];
    bytes.extend_from_slice(&u32::try_from(values.len()).expect("mesh fixture array size fits the wire format").to_be_bytes());
    for value in values {
        bytes.extend_from_slice(value);
    }
    bytes.push(b']');
    bytes
}

fn map(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut bytes = vec![b'{'];
    bytes.extend_from_slice(&u32::try_from(entries.len()).expect("mesh fixture map size fits the wire format").to_be_bytes());
    for (key, value) in entries {
        bytes.push(b'k');
        bytes.extend_from_slice(&u32::try_from(key.len()).expect("mesh fixture key fits the wire format").to_be_bytes());
        bytes.extend_from_slice(key.as_bytes());
        bytes.extend_from_slice(value);
    }
    bytes.push(b'}');
    bytes
}
