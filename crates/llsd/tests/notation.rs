use std::assert_matches;

use llsd::{Binary, Error, Limits, Value, notation};

#[test]
fn raw_binary() {
    let spec_vector = notation::from_slice::<Value>(br#"b(10)"0123456789""#).expect("decode spec binary vector");
    assert_eq!(spec_vector, Value::Binary(Binary::from(b"0123456789".to_vec())));

    let encoded = b"[b(0)\"\",b(3)\"a\x00\xff\",b(3)\"a\"b\"]";
    let decoded = notation::from_slice::<Value>(encoded).expect("decode raw binary values");

    assert_eq!(
        decoded,
        Value::Array(vec![
            Value::Binary(Binary::from(vec![])),
            Value::Binary(Binary::from(vec![b'a', 0, 0xff])),
            Value::Binary(Binary::from(b"a\"b".to_vec())),
        ])
    );
    assert!(notation::from_slice::<Value>(b"b(2)\"x\"").is_err());
}

#[test]
fn raw_strings_preserve_payload_bytes() {
    let expected = "a\"b\\c,}:# é";
    let encoded = format!("s({})\"{expected}\"", expected.len());
    assert_eq!(notation::from_str::<String>(&encoded).expect("decode raw notation string"), expected);

    assert!(notation::from_slice::<String>(b"s(1)\"ab\"").is_err());
    assert_matches!(notation::from_slice::<String>(b"s(1)\"\xff\""), Err(Error::InvalidUtf8));
}

#[test]
fn hex_binary_requires_byte_pairs() {
    let expected = Value::Binary(Binary::from(vec![0, 255]));
    for input in [b"b16\"00ff\"".as_slice(), b"b16\"00FF\""] {
        assert_eq!(notation::from_slice::<Value>(input).expect("decode hexadecimal binary"), expected);
    }

    assert_eq!(notation::from_slice::<Value>(b"b16\"\"").expect("decode empty hexadecimal binary"), Value::Binary(Binary::from(vec![])));
    for input in [b"b16\"0\"".as_slice(), b"b16\"0g\"", "b16\"€0\"".as_bytes(), br#"b16"\u20ac0""#] {
        assert!(notation::from_slice::<Value>(input).is_err(), "{input:?}");
    }
}

#[test]
fn binary_text_uses_binary_limit() {
    let limits = Limits { max_string_bytes: 1, max_binary_bytes: 2, ..Limits::default() };
    let expected = Value::Binary(Binary::from(vec![0, 255]));

    for input in [b"b16\"00FF\"".as_slice(), b"b16\"\\x30\\x30\\x46\\x46\"", b"b64\"AP8=\"", b"b(2)\"\x00\xff\""] {
        assert_eq!(notation::from_slice_with_limits::<Value>(input, limits).expect("binary fits its limit"), expected);
    }

    for input in [b"b16\"000102\"".as_slice(), b"b64\"AAEC\"", b"b(3)\"abc\""] {
        assert_matches!(notation::from_slice_with_limits::<Value>(input, limits), Err(Error::LimitExceeded("binary bytes")));
    }

    assert_matches!(notation::from_slice_with_limits::<Value>(b"'ab'", limits), Err(Error::LimitExceeded("string bytes")));
}

#[test]
fn boolean_spellings() {
    for (encoded, expected) in [
        (b"1".as_slice(), true),
        (b"t", true),
        (b"T", true),
        (b"true", true),
        (b"TRUE", true),
        (b"0", false),
        (b"f", false),
        (b"F", false),
        (b"false", false),
        (b"FALSE", false),
    ] {
        assert_eq!(notation::from_slice::<Value>(encoded).expect("decode boolean spelling"), Value::Boolean(expected), "{encoded:?}");
    }
}

#[test]
fn control_escapes() {
    let decoded = notation::from_slice::<Value>(br#""\a\v""#).expect("decode control escapes");
    assert_eq!(decoded, Value::String("\u{0007}\u{000b}".into()));
}

#[test]
fn map_keys_obey_byte_limits() {
    let limits = Limits { max_map_key_bytes: 2, max_string_bytes: 64, ..Limits::default() };
    for input in [&b"{'long':i1}"[..], &b"{s(4)\"long\":i1}"[..], &b"{'\\x61\\x62\\x63':i1}"[..]] {
        assert_matches!(notation::from_slice_with_limits::<Value>(input, limits), Err(Error::LimitExceeded("map key bytes")));
    }
}

#[test]
fn escaped_whitespace_is_invalid() {
    assert!(notation::from_slice_with_limits::<Value>(b"'value\\ x'", Limits::default()).is_err());
}
