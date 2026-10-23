#![no_main]

use libfuzzer_sys::fuzz_target;
use llsd::{Error, Limits, Value, binary, notation, xml};

const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_ROUND_TRIP_BYTES: usize = 512 * 1024;
const INPUT_LIMITS: Limits = Limits {
    max_bytes: MAX_INPUT_BYTES,
    max_depth: 16,
    max_values: 512,
    max_container_items: 128,
    max_string_bytes: 4 * 1024,
    max_map_key_bytes: 256,
    max_binary_bytes: 8 * 1024,
};
const ROUND_TRIP_LIMITS: Limits = Limits { max_bytes: MAX_ROUND_TRIP_BYTES, ..INPUT_LIMITS };

type Decoder = fn(&[u8], Limits) -> llsd::Result<Value>;

fn semantically_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Real(left), Value::Real(right)) => left == right || (left.is_nan() && right.is_nan()),
        (Value::Date(left), Value::Date(right)) => {
            let left = left.unix_seconds();
            let right = right.unix_seconds();
            (left - right).abs() <= left.abs().max(right.abs()) * f64::EPSILON * 2.0 + 1e-9
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(left, right)| semantically_equal(left, right))
        }
        (Value::Map(left), Value::Map(right)) => {
            left.len() == right.len() && left.iter().all(|(key, left)| right.get(key).is_some_and(|right| semantically_equal(left, right)))
        }
        _ => left == right,
    }
}

fn check_round_trip(value: &Value, format: &str, encoded: Vec<u8>, decode: Decoder) {
    assert!(encoded.len() <= ROUND_TRIP_LIMITS.max_input_bytes, "{format} output exceeded the round-trip input limit");
    let decoded = decode(&encoded, ROUND_TRIP_LIMITS).unwrap_or_else(|error| panic!("{format} output did not decode: {error}"));
    assert!(semantically_equal(value, &decoded), "{format} round trip changed {value:?} to {decoded:?}");
}

fn check_all_round_trips(value: &Value) {
    match xml::to_vec_with_max_bytes(value, MAX_ROUND_TRIP_BYTES) {
        Ok(encoded) => check_round_trip(value, "XML", encoded, xml::from_slice_with_limits::<Value>),
        Err(llsd::Error::InvalidValue(_)) => {}
        Err(error) => panic!("XML encoder rejected a decoded value: {error}"),
    }
    check_round_trip(
        value,
        "binary",
        binary::to_vec(value).expect("binary encoder accepts decoded LLSD values"),
        binary::from_slice_with_limits::<Value>,
    );
    check_round_trip(
        value,
        "notation",
        notation::to_vec(value).expect("notation encoder accepts decoded LLSD values"),
        notation::from_slice_with_limits::<Value>,
    );
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        let limits = Limits { max_bytes: MAX_INPUT_BYTES, ..INPUT_LIMITS };
        assert!(matches!(xml::from_slice_with_limits::<Value>(data, limits), Err(Error::LimitExceeded("input bytes"))));
        assert!(matches!(binary::from_slice_with_limits::<Value>(data, limits), Err(Error::LimitExceeded("input bytes"))));
        assert!(matches!(notation::from_slice_with_limits::<Value>(data, limits), Err(Error::LimitExceeded("input bytes"))));
        return;
    }

    if let Ok(value) = xml::from_slice_with_limits::<Value>(data, INPUT_LIMITS) {
        xml::value_allocation_bound(data, INPUT_LIMITS).expect("accepted XML has an allocation bound");
        check_all_round_trips(&value);
    }

    if let Ok(value) = binary::from_slice_with_limits::<Value>(data, INPUT_LIMITS) {
        check_all_round_trips(&value);
    }

    if let Ok(value) = notation::from_slice_with_limits::<Value>(data, INPUT_LIMITS) {
        check_all_round_trips(&value);
    }
});
