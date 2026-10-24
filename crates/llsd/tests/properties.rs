use std::collections::BTreeMap;

use llsd::{Binary, Date, Limits, Uri, Value, binary, notation, xml};
use proptest::prelude::*;
use uuid::Uuid;

const CASES: u32 = 96;
type Decoder = fn(&[u8]) -> Result<Value, llsd::Error>;

fn short_text() -> impl Strategy<Value = String> {
    prop::collection::vec(0x20u8..=0x7e, 0..17).prop_map(|bytes| String::from_utf8(bytes).expect("ASCII bytes are UTF-8"))
}

fn xml_character() -> impl Strategy<Value = char> {
    prop_oneof![
        Just('\t'),
        Just('\n'),
        Just('\r'),
        (0x20u32..=0xd7ff).prop_map(|value| char::from_u32(value).expect("valid XML character")),
        (0xe000u32..=0xfffd).prop_map(|value| char::from_u32(value).expect("valid XML character")),
        (0x10000u32..=0x10ffff).prop_map(|value| char::from_u32(value).expect("valid XML character")),
    ]
}

fn xml_text() -> impl Strategy<Value = String> {
    prop::collection::vec(xml_character(), 0..17).prop_map(String::from_iter)
}

fn finite_real() -> impl Strategy<Value = f64> {
    prop_oneof![
        8 => any::<u64>().prop_map(f64::from_bits).prop_filter("finite real", |value| value.is_finite()),
        1 => Just(0.0),
        1 => Just(-0.0),
        1 => Just(f64::from_bits(1)),
        1 => Just(f64::from_bits((1u64 << 63) | 1)),
        1 => Just(f64::from_bits(0x000f_ffff_ffff_ffff)),
        1 => Just(f64::from_bits(0x800f_ffff_ffff_ffff)),
        1 => Just(f64::MIN_POSITIVE),
        1 => Just(-f64::MIN_POSITIVE),
        1 => Just(f64::MAX),
        1 => Just(-f64::MAX),
    ]
}

fn real_bits_match(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Real(actual), Value::Real(expected)) => actual.to_bits() == expected.to_bits(),
        (Value::Array(actual), Value::Array(expected)) => {
            actual.len() == expected.len() && actual.iter().zip(expected).all(|(actual, expected)| real_bits_match(actual, expected))
        }
        (Value::Map(actual), Value::Map(expected)) => {
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(expected)
                    .all(|((actual_key, actual), (expected_key, expected))| actual_key == expected_key && real_bits_match(actual, expected))
        }
        _ => true,
    }
}

fn values() -> impl Strategy<Value = Value> {
    let leaves = prop_oneof![
        Just(Value::Undefined),
        any::<bool>().prop_map(Value::Boolean),
        any::<i32>().prop_map(Value::Integer),
        finite_real().prop_map(Value::Real),
        xml_text().prop_map(Value::String),
        prop_oneof![short_text(), xml_text()].prop_map(|value| Value::Uri(Uri::new(value))),
        prop::collection::vec(any::<u8>(), 0..17).prop_map(|bytes| Value::Binary(Binary::from(bytes))),
        (-2_000_000_000i64..=2_000_000_000)
            .prop_filter_map("valid whole-second date", |seconds| Date::from_unix_seconds(seconds as f64).map(Value::Date)),
        any::<[u8; 16]>().prop_map(|bytes| Value::Uuid(Uuid::from_bytes(bytes))),
    ];

    leaves.prop_recursive(3, 32, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::btree_map(xml_text(), inner, 0..4).prop_map(|map| Value::Map(BTreeMap::from_iter(map))),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: CASES, .. ProptestConfig::default() })]

    #[test]
    fn inputs_do_not_panic(input in prop::collection::vec(any::<u8>(), 0..257)) {
        let decoders: [(&str, Decoder); 3] = [
            ("XML", xml::from_slice::<Value>),
            ("binary", binary::from_slice::<Value>),
            ("notation", notation::from_slice::<Value>),
        ];

        for (format, decode) in decoders {
            let decoded = decode(&input);
            if format == "XML" && decoded.is_ok() {
                prop_assert!(xml::value_allocation_bound(&input, Limits::default()).is_ok(), "{format} accepted input rejected by the allocation bound");
            }
        }
    }

    #[test]
    fn framed_b16_payload_no_panic(payload in prop::collection::vec(any::<char>(), 0..17)) {
        let mut input = b"b16\"".to_vec();
        input.extend(payload.iter().copied().collect::<String>().as_bytes());
        input.push(b'"');
        let _ = notation::from_slice::<Value>(&input);
    }

    #[test]
    fn framed_b16_valid_bytes_roundtrip(bytes in prop::collection::vec(any::<u8>(), 0..33)) {
        let mut input = b"b16\"".to_vec();
        for byte in &bytes {
            input.extend_from_slice(format!("{byte:02X}").as_bytes());
        }
        input.push(b'"');
        let decoded = notation::from_slice::<Value>(&input).expect("decode generated b16 binary");
        prop_assert_eq!(decoded, Value::Binary(Binary::from(bytes)));
    }

    #[test]
    fn values_roundtrip(value in values()) {
        let xml = xml::to_vec(&value).expect("generated LLSD value encodes as XML");
        let binary = binary::to_vec(&value).expect("generated LLSD value encodes as binary");
        let notation = notation::to_vec(&value).expect("generated LLSD value encodes as notation");
        let limits = Limits::default();

        let decoded_xml = xml::from_slice_with_limits::<Value>(&xml, limits).expect("decode generated XML within limits");
        prop_assert!(xml::value_allocation_bound(&xml, limits).is_ok());
        let decoded_binary = binary::from_slice_with_limits::<Value>(&binary, limits).expect("decode generated binary within limits");
        let decoded_notation = notation::from_slice_with_limits::<Value>(&notation, limits).expect("decode generated notation within limits");

        prop_assert_eq!(&decoded_xml, &value);
        prop_assert_eq!(&decoded_binary, &value);
        prop_assert_eq!(&decoded_notation, &value);
        prop_assert!(real_bits_match(&decoded_xml, &value), "XML real bits changed: {value:?}");
        prop_assert!(real_bits_match(&decoded_binary, &value), "binary real bits changed: {value:?}");
        prop_assert!(real_bits_match(&decoded_notation, &value), "notation real bits changed: {value:?}");
    }

    #[test]
    fn xml_allocation_accepts_decoder_limits(
        value in values(),
        (max_bytes, max_depth, max_values, max_container_items, max_string_bytes, max_map_key_bytes, max_binary_bytes)
            in (0usize..2048, 0usize..5, 0usize..40, 0usize..8, 0usize..32, 0usize..16, 0usize..32)
    ) {
        let xml = xml::to_vec(&value).expect("generated LLSD value encodes as XML");
        let limits = Limits {
            max_bytes,
            max_depth,
            max_values,
            max_container_items,
            max_string_bytes,
            max_map_key_bytes,
            max_binary_bytes,
        };

        if xml::from_slice_with_limits::<Value>(&xml, limits).is_ok() {
            prop_assert!(xml::value_allocation_bound(&xml, limits).is_ok(), "decoder accepted XML rejected by bound scanner: {xml:?}, {limits:?}");
        }
    }

    #[test]
    fn xml_mutation_bound(value in values(), offset in any::<usize>(), replacement in any::<u8>()) {
        let mut input = xml::to_vec(&value).expect("encode mutation input");
        let index = offset % input.len();
        input[index] = replacement;
        if xml::from_slice::<Value>(&input).is_ok() {
            prop_assert!(xml::value_allocation_bound(&input, Limits::default()).is_ok());
        }
    }
}
