use std::assert_matches;
use std::{fmt::Write, hint::black_box};

use llsd::{Limits, Value, xml};
use test_support::allocation::{self, CountingAllocator, live_bytes};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

fn measure<T>(operation: impl FnOnce() -> T) -> (T, i128, i128) {
    let (value, allocations) = allocation::measure(operation);
    (value, allocations.live_bytes, allocations.peak_bytes)
}

#[test]
fn xml_decoding_releases_allocations() {
    let pairs = 131_068;
    let mut large_map = String::from("<llsd><map><key>id</key><integer>1</integer><key>events</key><array/><key>extra</key><map>");
    for index in 0..pairs {
        write!(large_map, "<key>k{index}</key><integer>0</integer>").expect("append XML pair");
    }
    large_map.push_str("</map></map></llsd>");
    let mut flat_array = String::from("<llsd><array>");
    for _ in 0..131_073 {
        flat_array.push_str("<undef/>");
    }
    flat_array.push_str("</array></llsd>");
    let binary = format!("<llsd><binary>{}</binary></llsd>", "AAAA".repeat(4 * 1024 * 1024));
    let split_text = format!("<llsd><string>{}<![CDATA[{}]]></string></llsd>", "x".repeat(100_001), "x".repeat(99_999));
    let mut node_boundary = String::from("<llsd><map>");
    for index in 0..144 {
        write!(node_boundary, "<key>k{index}</key><map><key>v</key><string>text</string></map>").expect("append nested map");
    }
    node_boundary.push_str("</map></llsd>");
    let limits = Limits { max_bytes: 32 * 1024 * 1024, max_values: 262_144, max_container_items: 262_144, ..Limits::default() };
    for (name, input) in [
        ("empty", "<llsd><undef/></llsd>".to_owned()),
        ("depth limit", format!("<llsd>{}<integer>1</integer>{}</llsd>", "<array>".repeat(64), "</array>".repeat(64))),
        ("entities", format!("<llsd><string>{}</string></llsd>", "&#x1f600;&amp;".repeat(4096))),
        ("binary attributes", format!("<llsd><binary {} encoding=\"base64\">AAAA</binary></llsd>", (0..1024).map(|i| format!("x{i}=\"ok\" ")).collect::<String>())),
        ("EventQueue shape", "<llsd><map><key>id</key><integer>1</integer><key>events</key><array><map><key>message</key><string>ParcelProperties</string><key>body</key><map><key>name</key><string>parcel</string></map></map></array></map></llsd>".to_owned()),
        ("large extra map", large_map),
        ("flat array growth", flat_array),
        ("large binary", binary),
        ("split scalar growth", split_text),
        ("normalized line endings", format!("<llsd><string>{}</string></llsd>", "ab\r\n".repeat(50_000))),
        ("nested map nodes", node_boundary),
    ] {
        let baseline = live_bytes();
        let (value, retained, peak) = measure(|| xml::from_slice_with_limits::<Value>(black_box(input.as_bytes()), limits).expect("decode measured XML"));
        assert!(peak >= retained);
        drop(value);
        assert_eq!(live_bytes(), baseline, "decoded storage must return for {name}");
        eprintln!("{name}: input={} B, retained={retained} B, decode_peak={peak} B", input.len());
    }
}

#[test]
fn oversized_keys_stay_bounded() {
    type Decode = fn(&[u8], Limits) -> Result<Value, llsd::Error>;
    let large = "x".repeat(1024 * 1024);
    let binary_map = |key: &str| {
        let mut bytes = b"{\0\0\0\x01k".to_vec();
        bytes.extend_from_slice(&(key.len() as u32).to_be_bytes());
        bytes.extend_from_slice(key.as_bytes());
        bytes.extend_from_slice(b"!}");
        bytes
    };
    let cases: [(&str, Decode, Vec<u8>, Vec<u8>); 3] = [
        (
            "XML",
            xml::from_slice_with_limits,
            b"<llsd><map><key>xxx</key><undef/></map></llsd>".to_vec(),
            format!("<llsd><map><key>{large}</key><undef/></map></llsd>").into_bytes(),
        ),
        ("notation", llsd::notation::from_slice_with_limits, b"{'xxx':!}".to_vec(), format!("{{'{large}':!}}").into_bytes()),
        ("binary", llsd::binary::from_slice_with_limits, binary_map("xxx"), binary_map(&large)),
    ];
    for (name, decode, small, large) in cases {
        let limits = Limits { max_map_key_bytes: 2, ..Limits::default() };
        let (small_result, small_live, small_peak) = measure(|| decode(&small, limits));
        let (large_result, large_live, large_peak) = measure(|| decode(&large, limits));
        assert_matches!(small_result, Err(llsd::Error::LimitExceeded("map key bytes")), "small key for {name}");
        assert_matches!(large_result, Err(llsd::Error::LimitExceeded("map key bytes")), "large key for {name}");
        assert_eq!((small_live, large_live), (0, 0), "failed decode retained storage for {name}");
        assert!(large_peak <= small_peak + 4096, "oversized key was copied for {name}: {small_peak} -> {large_peak}");
    }
}
