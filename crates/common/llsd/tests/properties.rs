use std::collections::BTreeMap;

use base::UUID;
use llsd::{Binary, Date, URI, Value, binary, notation, xml};
use proptest::prelude::*;

const CASES: u32 = 96;

fn short_text() -> impl Strategy<Value = String> {
    prop::collection::vec(0x20u8..=0x7e, 0..17).prop_map(|bytes| String::from_utf8(bytes).expect("ASCII bytes are UTF-8"))
}

fn values() -> impl Strategy<Value = Value> {
    let leaves = prop_oneof![
        Just(Value::Undefined),
        any::<bool>().prop_map(Value::Boolean),
        any::<i32>().prop_map(Value::Integer),
        (-1_000_000.0f64..1_000_000.0).prop_map(Value::Real),
        short_text().prop_map(Value::String),
        short_text().prop_map(|value| Value::URI(URI::new(value))),
        prop::collection::vec(any::<u8>(), 0..17).prop_map(|bytes| Value::Binary(Binary::from(bytes))),
        (-2_000_000_000i64..=2_000_000_000)
            .prop_filter_map("valid whole-second date", |seconds| Date::from_unix_seconds(seconds as f64).map(Value::Date)),
        Just(Value::UUID(UUID::parse_str("12345678-90ab-cdef-0123-456789abcdef").expect("fixed UUID is valid"))),
    ];

    leaves.prop_recursive(3, 32, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::btree_map(short_text(), inner, 0..4).prop_map(|map| Value::Map(BTreeMap::from_iter(map))),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: CASES, .. ProptestConfig::default() })]

    #[test]
    fn xml_input_no_panic(input in prop::collection::vec(any::<u8>(), 0..257)) {
        let _ = xml::from_slice::<Value>(&input);
    }

    #[test]
    fn binary_input_no_panic(input in prop::collection::vec(any::<u8>(), 0..257)) {
        let _ = binary::from_slice::<Value>(&input);
    }

    #[test]
    fn notation_input_no_panic(input in prop::collection::vec(any::<u8>(), 0..257)) {
        let _ = notation::from_slice::<Value>(&input);
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

        let decoded_xml = xml::from_slice::<Value>(&xml).expect("decode generated XML");
        let decoded_binary = binary::from_slice::<Value>(&binary).expect("decode generated binary");
        let decoded_notation = notation::from_slice::<Value>(&notation).expect("decode generated notation");

        prop_assert_eq!(&decoded_xml, &value);
        prop_assert_eq!(&decoded_binary, &value);
        prop_assert_eq!(&decoded_notation, &value);
    }
}
