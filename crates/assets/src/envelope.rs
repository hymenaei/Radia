use std::io::{Read, Write};

use base::AssetId;
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use llsd::{Binary, Limits, Value, binary, xml};

use crate::{Failure, Kind, Resource};

const MAX_ENVELOPE_BYTES: usize = 4 * 1024 * 1024;

fn limits(input: usize) -> Limits {
    Limits {
        max_input_bytes: input,
        max_depth: 16,
        max_values: 4096,
        max_container_items: 512,
        max_string_bytes: 2 * 1024 * 1024,
        max_map_key_bytes: 128,
        max_binary_bytes: 2 * 1024 * 1024,
    }
}

pub fn legacy_request(id: AssetId) -> Result<Value, Failure> {
    let request = Value::Array(vec![Value::Binary(Binary::new(id.uuid().as_bytes().to_vec()))]);
    let bytes = binary::to_vec(&request).map_err(|_| Failure::Malformed)?;
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&bytes).map_err(|_| Failure::Malformed)?;
    let zipped = encoder.finish().map_err(|_| Failure::Malformed)?;
    Ok(Value::Map([("Zipped".into(), Value::Binary(Binary::new(zipped)))].into()))
}

pub(crate) fn extract(resource: Resource, bytes: Vec<u8>) -> Result<Vec<u8>, Failure> {
    if bytes.len() > resource.kind.max_response_bytes() {
        return Err(Failure::TooLarge);
    }
    match resource.kind {
        Kind::Texture | Kind::Environment => {
            if bytes.is_empty() {
                Err(Failure::Malformed)
            } else {
                Ok(bytes)
            }
        }
        Kind::PbrMaterial => {
            let Value::Map(mut fields) = binary::from_slice_with_limits(&bytes, limits(bytes.len())).map_err(|_| Failure::Malformed)?
            else {
                return Err(Failure::Malformed);
            };
            if !matches!(fields.get("version"), Some(Value::String(version)) if matches!(version.as_str(), "1.0" | "1.1"))
                || !matches!(fields.get("type"), Some(Value::String(kind)) if kind == "GLTF 2.0")
            {
                return Err(Failure::Unsupported);
            }
            match fields.remove("data") {
                Some(Value::String(json)) => Ok(json.into_bytes()),
                _ => Err(Failure::Malformed),
            }
        }
        Kind::LegacyMaterial => {
            let Value::Map(mut fields) = xml::from_slice_with_limits(&bytes, limits(bytes.len())).map_err(|_| Failure::Malformed)? else {
                return Err(Failure::Malformed);
            };
            let Some(Value::Binary(zipped)) = fields.remove("Zipped") else { return Err(Failure::Malformed) };
            let mut decoder = ZlibDecoder::new(zipped.as_slice());
            let mut inflated = Vec::with_capacity(MAX_ENVELOPE_BYTES);
            let mut chunk = [0; 16 * 1024];
            loop {
                let count = decoder.read(&mut chunk).map_err(|_| Failure::Malformed)?;
                if count == 0 {
                    break;
                }
                if inflated.len() + count > MAX_ENVELOPE_BYTES {
                    return Err(Failure::TooLarge);
                }
                inflated.extend_from_slice(&chunk[..count]);
            }
            let Value::Array(materials) =
                binary::from_slice_with_limits(&inflated, limits(MAX_ENVELOPE_BYTES)).map_err(|_| Failure::Malformed)?
            else {
                return Err(Failure::Malformed);
            };
            for material in materials {
                let Value::Map(mut material) = material else { continue };
                if !matches!(material.get("ID"), Some(Value::Binary(id)) if id.as_slice() == resource.id.uuid().as_bytes()) {
                    continue;
                }
                let Some(Value::Map(value)) = material.remove("Material") else { return Err(Failure::Malformed) };
                let value = binary::to_vec(&Value::Map(value)).map_err(|_| Failure::Malformed)?;
                if value.len() > MAX_ENVELOPE_BYTES {
                    return Err(Failure::TooLarge);
                }
                return Ok(value);
            }
            Err(Failure::NotFound)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base::UUID;

    fn resource(kind: Kind) -> Resource {
        Resource { id: AssetId::new(UUID::from_u128(1)), kind }
    }

    #[test]
    fn literal_pbr_asset_requires_supported_type_and_version() {
        let bytes = b"{\0\0\0\x03k\0\0\0\x07versions\0\0\0\x031.1k\0\0\0\x04types\0\0\0\x08GLTF 2.0k\0\0\0\x04datas\0\0\0\x02{}}";
        assert_eq!(extract(resource(Kind::PbrMaterial), bytes.to_vec()), Ok(b"{}".to_vec()));
        let invalid = bytes.windows(3).position(|window| window == b"1.1").unwrap();
        let mut unsupported = bytes.to_vec();
        unsupported[invalid..invalid + 3].copy_from_slice(b"9.9");
        assert_eq!(extract(resource(Kind::PbrMaterial), unsupported), Err(Failure::Unsupported));
        assert_eq!(extract(resource(Kind::PbrMaterial), bytes[..bytes.len() - 1].to_vec()), Err(Failure::Malformed));
    }

    fn stored_zlib(bytes: &[u8]) -> Vec<u8> {
        let length = u16::try_from(bytes.len()).unwrap();
        let mut encoded = vec![0x78, 0x01, 0x01];
        encoded.extend(length.to_le_bytes());
        encoded.extend((!length).to_le_bytes());
        encoded.extend_from_slice(bytes);
        let (mut a, mut b) = (1u32, 0u32);
        for byte in bytes {
            a = (a + u32::from(*byte)) % 65521;
            b = (b + a) % 65521;
        }
        encoded.extend(((b << 16) | a).to_be_bytes());
        encoded
    }

    fn outer(zipped: Vec<u8>) -> Vec<u8> {
        xml::to_vec(&Value::Map([("Zipped".into(), Value::Binary(Binary::new(zipped)))].into())).unwrap()
    }

    #[test]
    fn legacy_request_and_literal_response_use_binary_ids_and_select_requested_material() {
        let resource = resource(Kind::LegacyMaterial);
        let Value::Map(request) = legacy_request(resource.id).unwrap() else { unreachable!() };
        let Some(Value::Binary(zipped)) = request.get("Zipped") else { unreachable!() };
        let mut decoded = Vec::new();
        ZlibDecoder::new(zipped.as_slice()).read_to_end(&mut decoded).unwrap();
        assert_eq!(
            binary::from_slice::<Value>(&decoded).unwrap(),
            Value::Array(vec![Value::Binary(Binary::new(resource.id.uuid().as_bytes().to_vec()))])
        );
        let mut record = b"[\0\0\0\x01{\0\0\0\x02k\0\0\0\x02IDb\0\0\0\x10".to_vec();
        record.extend_from_slice(resource.id.uuid().as_bytes());
        record.extend_from_slice(b"k\0\0\0\x08Material{\0\0\0\x01k\0\0\0\x04Names\0\0\0\x03oak}}]");
        let response = outer(stored_zlib(&record));
        let selected = extract(resource, response.clone()).unwrap();
        assert_eq!(binary::from_slice::<Value>(&selected).unwrap(), Value::Map([("Name".into(), Value::String("oak".into()))].into()));
        let other = Resource { id: AssetId::new(UUID::from_u128(2)), ..resource };
        assert_eq!(extract(other, response), Err(Failure::NotFound));
        assert_eq!(extract(resource, outer(vec![0, 1, 2])), Err(Failure::Malformed));
    }

    #[test]
    fn compressed_expansion_and_raw_input_are_bounded() {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&vec![b' '; MAX_ENVELOPE_BYTES + 1]).unwrap();
        assert_eq!(extract(resource(Kind::LegacyMaterial), outer(encoder.finish().unwrap())), Err(Failure::TooLarge));
        assert_eq!(extract(resource(Kind::PbrMaterial), vec![0; Kind::PbrMaterial.max_response_bytes() + 1]), Err(Failure::TooLarge));
    }
}
