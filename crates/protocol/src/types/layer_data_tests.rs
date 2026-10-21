use super::*;

fn payload(bits: &str) -> Vec<u8> {
    let bits: Vec<_> = bits.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    assert!(bits.iter().all(|b| matches!(b, b'0' | b'1')));
    let mut data = vec![8, 1, 16, b'L'];
    data.extend(bits.chunks(8).map(|chunk| chunk.iter().enumerate().fold(0, |byte, (i, bit)| byte | ((bit - b'0') << (7 - i)))));
    data
}

#[test]
fn flat_patch_uses_range_midpoint_and_wire_coordinates() {
    let data = [8, 1, 16, b'L', 0, 0, 0, 32, 65, 20, 0, 101, 38, 16];
    let mut layer = LandLayer::new(b'L', &data).unwrap();
    let patch = layer.next().unwrap().unwrap();
    assert_eq!(patch.coordinates(), (3, 5));
    assert_eq!(patch.heights(), &[20.0; PATCH_SAMPLES]);
    assert_eq!(layer.next(), None);
    assert_eq!(layer.next(), None);
}

#[test]
fn independent_single_frequency_vectors_fix_scaling_sign_and_orientation() {
    for (coefficients, axis, sign) in [("11001 10", None, 1.0), ("0 11001 10", Some(true), 1.0), ("0 0 11101 10", Some(false), -1.0)] {
        let data = payload(&format!("00000000 00000000 00000000 00000000 00000000 00001000 00000000 00000000 00 {coefficients} 01100001"));
        let mut layer = LandLayer::new(b'L', &data).unwrap();
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
fn chunked_magnitudes_and_patch_ids_are_little_endian() {
    let data = payload("00001111 00000000 00000000 00000000 00000000 00000100 00000000 00000001 11 110 01000101 00100011 1 10 01100001");
    let mut layer = LandLayer::new(b'L', &data).unwrap();
    let patch = layer.next().unwrap().unwrap();
    assert_eq!(patch.coordinates(), (24, 1));
    assert!(patch.heights().iter().all(|height| (*height - (2.0 + 74565.0 / 16.0)).abs() < 0.001));
    assert_eq!(layer.next(), None);
}

#[test]
fn preserves_valid_patches_before_truncation_and_after_invalid_values() {
    let data = payload(
        "00000000 00000000 00000000 10000000 01111111 00001000 00000000 00000000 00 10
                            00000000 00000000 00000000 00000000 00000000 00001000 00000000 00000000 00 10",
    );
    let mut layer = LandLayer::new(b'L', &data).unwrap();
    assert_eq!(layer.next(), Some(Err(LandError::NonFinite)));
    assert_eq!(layer.next().unwrap().unwrap().heights(), &[4.0; PATCH_SAMPLES]);
    assert_eq!(layer.next(), Some(Err(LandError::Truncated)));
    assert_eq!(layer.next(), None);
}

#[test]
fn rejects_unsupported_headers_and_caps_work_even_for_rejected_patches() {
    assert!(matches!(LandLayer::new(b'7', &[8, 1, 16, b'L']), Err(LandError::UnsupportedLayer)));
    assert!(matches!(LandLayer::new(b'L', &[8, 1, 32, b'L']), Err(LandError::UnsupportedPatchSize)));
    assert!(matches!(LandLayer::new(b'L', &[0; MAX_LAYER_BYTES + 1]), Err(LandError::PayloadLimit)));
    let patch = "00000000 00000000 00000000 10000000 01111111 00001000 00000000 00000000 00 10 ";
    let data = payload(&patch.repeat(MAX_LAYER_PATCHES + 1));
    let mut layer = LandLayer::new(b'L', &data).unwrap();
    for _ in 0..MAX_LAYER_PATCHES {
        assert_eq!(layer.next(), Some(Err(LandError::NonFinite)));
    }
    assert_eq!(layer.next(), Some(Err(LandError::PatchLimit)));
    assert_eq!(layer.next(), None);
}

proptest::proptest! {
    #[test]
    fn arbitrary_patch_streams_stay_bounded_and_return_finite_samples(body in proptest::collection::vec(proptest::num::u8::ANY, 0..2048)) {
        let mut data = vec![8, 1, 16, b'L'];
        data.extend(body);
        let mut layer = LandLayer::new(b'L', &data).unwrap();
        let mut count = 0;
        for result in layer.by_ref() {
            count += 1;
            proptest::prop_assert!(count <= MAX_LAYER_PATCHES + 1);
            if let Ok(patch) = result {
                proptest::prop_assert!(patch.heights().iter().all(|height| height.is_finite()));
            }
        }
        proptest::prop_assert_eq!(layer.next(), None);
    }
}
