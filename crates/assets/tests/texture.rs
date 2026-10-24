use assets::Priority;
use std::time::Instant;
use test_support::texture::{
    GRAY_4096_J2C, GRAY_ALPHA_J2C, GRAY_ALPHA_RAW, GRAY_J2C, GRAY_RAW, RGB_J2C, RGB_RAW, RGBA_J2C, RGBA_JP2, RGBA_LOSSY_J2C,
    RGBA_LOSSY_RAW, RGBA_RAW,
};

use assets::{Alpha, Budget, ColorSpace, Content, Failure, Image, Kind, PixelFormat, Request, Resource, Status, Store};
use utils::AssetId;
use uuid::Uuid;

fn decode(bytes: &[u8]) -> Result<Image, Failure> {
    let now = Instant::now();
    let mut store = Store::new(Budget::default());
    let route = store.register_route().unwrap();
    let resource = Resource { id: AssetId::new(Uuid::from_u128(1)), kind: Kind::Texture };
    let demand = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    let result = job.prepare(bytes.to_vec());
    store.complete(job, result, now);
    match demand.status() {
        Status::Ready(Content::Texture(image)) => Ok(image.image().unwrap()),
        Status::Failed { failure, retry_at: None } => Err(failure),
        status => panic!("unexpected texture status: {status:?}"),
    }
}

#[test]
fn lossless_channels_match_openjpeg() {
    let cases: &[(&[u8], &[u8], usize, bool)] = &[
        (GRAY_J2C, GRAY_RAW, 1, false),
        (GRAY_ALPHA_J2C, GRAY_ALPHA_RAW, 2, true),
        (RGB_J2C, RGB_RAW, 3, false),
        (RGBA_J2C, RGBA_RAW, 4, true),
        (RGBA_JP2, RGBA_RAW, 4, true),
    ];
    for (encoded, expected, channels, alpha) in cases {
        let image = decode(encoded).unwrap();
        assert_eq!(image.size(), [8, 8]);
        assert_eq!(image.alpha() == Alpha::Straight, *alpha);
        assert_eq!(image.color_space(), ColorSpace::Srgb);
        assert_eq!(image.format().channels(), *channels);
        assert_eq!(image.pixels(), *expected);
    }
}

#[test]
fn lossy_error_stays_bounded() {
    let image = decode(RGBA_LOSSY_J2C).unwrap();
    let output = image.pixels();
    let source = RGBA_LOSSY_RAW;
    assert_eq!(output.len(), source.len());
    for (actual, expected) in output.iter().zip(source) {
        assert!(actual.abs_diff(*expected) <= 2, "actual {actual}, source {expected}");
    }
}

#[test]
fn reduced_gray_matches_openjpeg() {
    let image = decode(GRAY_4096_J2C).unwrap();
    assert_eq!(image.size(), [2048, 2048]);
    assert_eq!(image.format(), PixelFormat::Gray8);
    assert!(image.pixels().iter().all(|sample| *sample == 73));
}

#[test]
fn invalid_headers_fail_early() {
    let valid = RGBA_J2C;
    for length in 0..valid.len() {
        assert!(decode(&valid[..length]).is_err(), "accepted truncation at {length}");
    }
    let mut huge = valid.to_vec();
    huge[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(matches!(decode(&huge), Err(Failure::TooLarge)));
    let mut components = valid.to_vec();
    components[40..42].copy_from_slice(&u16::MAX.to_be_bytes());
    assert!(matches!(decode(&components), Err(Failure::Unsupported)));
    let mut expensive = valid.to_vec();
    for offset in [8, 12, 24, 28] {
        expensive[offset..offset + 4].copy_from_slice(&2048u32.to_be_bytes());
    }
    let cod = expensive.windows(2).position(|bytes| bytes == [0xff, 0x52]).unwrap();
    expensive[cod + 6..cod + 8].copy_from_slice(&32u16.to_be_bytes());
    expensive[cod + 10] = 0;
    expensive[cod + 11] = 0;
    assert!(matches!(decode(&expensive), Err(Failure::TooLarge)));
    assert!(matches!(decode(b"\x89PNG\r\n\x1a\n"), Err(Failure::Unsupported)));
    assert!(matches!(decode(&vec![0; 32 * 1024 * 1024 + 1]), Err(Failure::TooLarge)));
}

#[test]
fn reduced_rgb_matches_openjpeg() {
    use jpeg2k::{DecodeParameters, ImagePixelData};
    let image =
        jpeg2k::Image::from_bytes_with(include_bytes!("fixtures/rgb-reduced.j2c"), DecodeParameters::new().reduce(1).strict(true)).unwrap();
    let ImagePixelData::Rgb8(pixels) = image.get_pixels(None).unwrap().data else { panic!("expected RGB fixture") };
    assert_eq!(pixels, include_bytes!("fixtures/rgb-reduced-half.raw"));
}

#[test]
fn jp2_metadata_must_match() {
    let mut bytes = RGBA_JP2.to_vec();
    assert_eq!(decode(&bytes).unwrap().pixels(), RGBA_RAW);
    let definitions = bytes.windows(4).position(|tag| tag == b"cdef").unwrap();
    let alpha_type = definitions + 4 + 2 + 3 * 6 + 2;
    bytes[alpha_type..alpha_type + 2].copy_from_slice(&2u16.to_be_bytes());
    assert!(matches!(decode(&bytes), Err(Failure::Unsupported)), "premultiplied alpha requires an explicit conversion policy");
    let mut wrong_dimensions = RGBA_JP2.to_vec();
    let ihdr = wrong_dimensions.windows(4).position(|tag| tag == b"ihdr").unwrap();
    wrong_dimensions[ihdr + 4..ihdr + 8].copy_from_slice(&999u32.to_be_bytes());
    assert!(matches!(decode(&wrong_dimensions), Err(Failure::Malformed)));
}

proptest::proptest! {
    #[test]
    fn mutations_never_publish_an_invalid_image(offset in 0usize..470, byte in proptest::prelude::any::<u8>()) {
        let mut encoded = RGBA_J2C.to_vec();
        encoded[offset] = byte;
        if let Ok(image) = decode(&encoded) {
            let [width, height] = image.size();
            proptest::prop_assert!((1..=2048).contains(&width) && (1..=2048).contains(&height));
            proptest::prop_assert_eq!(image.pixels().len(), width as usize * height as usize * image.format().channels());
        }
    }
}
