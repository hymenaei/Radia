use std::borrow::Cow;
use std::rc::Rc;
use std::sync::Arc;

use crate::traits::{TryFromValue, TryToValue};
use crate::types::Value;

#[test]
fn to_i32() {
    let value = 42i32;
    let expected = Value::Integer(42);

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn from_i32() {
    let value = Value::Integer(42);
    let expected = 42i32;

    assert_eq!(i32::try_from_value(&value).unwrap(), expected);
}

#[test]
fn from_i32_fail() {
    let value = Value::Boolean(false);
    assert!(i32::try_from_value(&value).unwrap_err().is_wrong_type());
}

#[cfg(feature = "i8")]
#[test]
fn to_i64() {
    let value = 42i64;
    let expected = Value::Long(42);

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[cfg(feature = "i8")]
#[test]
fn from_i64() {
    let value = Value::Long(42);
    let expected = 42i64;

    assert_eq!(i64::try_from_value(&value).unwrap(), expected);
}

#[cfg(feature = "i8")]
#[test]
fn from_i64_fail() {
    let value = Value::Boolean(false);
    assert!(i64::try_from_value(&value).unwrap_err().is_wrong_type());
}

#[test]
fn to_boolean() {
    let value = true;
    let expected = Value::Boolean(true);

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn from_boolean() {
    let value = Value::Boolean(false);
    let expected = false;

    assert_eq!(bool::try_from_value(&value).unwrap(), expected);
}

#[test]
fn from_boolean_fail() {
    let value = Value::Integer(0);
    assert!(bool::try_from_value(&value).unwrap_err().is_wrong_type());
}

#[test]
fn to_string() {
    let value = "Hello, World!";
    let expected = Value::String(String::from("Hello, World!"));

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn from_string() {
    let value = Value::String(String::from("Hello, World!"));
    let expected = "Hello, World!";

    assert_eq!(String::try_from_value(&value).unwrap(), expected);
}

#[test]
fn from_string_fail() {
    let value = Value::Boolean(false);
    assert!(String::try_from_value(&value).unwrap_err().is_wrong_type());
}

#[test]
fn to_double() {
    let value = 1.5f64;
    let expected = Value::Double(1.5);

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[allow(clippy::float_cmp)]
#[test]
fn from_double() {
    let value = Value::Double(1.5);
    let expected = 1.5f64;

    assert_eq!(f64::try_from_value(&value).unwrap(), expected);
}

#[test]
fn from_double_fail() {
    let value = Value::Boolean(false);
    assert!(f64::try_from_value(&value).unwrap_err().is_wrong_type());
}

#[cfg(feature = "chrono")]
#[test]
fn to_datetime() {
    use crate::datetime::DateTime;
    use chrono::Utc;

    let now = DateTime::from(Utc::now().naive_utc());

    let value = now;
    let expected = Value::DateTime(now);

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[cfg(feature = "chrono")]
#[test]
fn from_datetime() {
    use crate::datetime::DateTime;
    use chrono::Utc;

    let now = DateTime::from(Utc::now().naive_utc());

    let value = Value::DateTime(now);
    let expected = now;

    assert_eq!(DateTime::try_from_value(&value).unwrap(), expected);
}

#[cfg(feature = "chrono")]
#[test]
fn from_datetime_fail() {
    use crate::datetime::DateTime;

    let value = Value::Boolean(false);
    assert!(DateTime::try_from_value(&value).unwrap_err().is_wrong_type());
}

#[test]
fn to_base64() {
    let data = b"You can't read this!".to_vec();

    let value = data.clone();
    let expected = Value::Base64(data);

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn from_base64() {
    let data = b"You can't read this!".to_vec();

    let value = Value::Base64(data.clone());
    let expected = data;

    assert_eq!(<Vec<u8>>::try_from_value(&value).unwrap(), expected);
}

#[test]
fn from_base64_fail() {
    let value = Value::Boolean(false);
    assert!(<Vec<u8>>::try_from_value(&value).unwrap_err().is_wrong_type());
}

#[test]
fn to_array() {
    let value = [1, 2, 3];
    let expected = vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)]
        .try_to_value()
        .unwrap();

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn from_vec() {
    let value = vec![1, 2, 3].try_to_value().unwrap();
    let expected = vec![1, 2, 3];

    assert_eq!(<Vec<i32>>::try_from_value(&value).unwrap(), expected);
}

#[test]
fn from_vec_fail_type() {
    let value = Value::Boolean(false).try_to_value().unwrap();
    assert!(<Vec<bool>>::try_from_value(&value).unwrap_err().is_wrong_type());
}

#[test]
fn from_vec_fail_inner_type() {
    let value = vec![1, 2, 3].try_to_value().unwrap();
    assert!(<Vec<bool>>::try_from_value(&value).unwrap_err().is_wrong_type());
}

#[test]
fn from_array() {
    let value = [1, 2, 3].try_to_value().unwrap();
    let expected = [1, 2, 3];

    assert_eq!(<[i32; 3]>::try_from_value(&value).unwrap(), expected);
}

#[test]
fn from_array_fail_type() {
    let value = Value::Boolean(false).try_to_value().unwrap();
    assert!(<[i32; 3]>::try_from_value(&value).unwrap_err().is_wrong_type());
}

#[test]
fn from_array_fail_inner_type() {
    let value = vec![1, 2, 3].try_to_value().unwrap();
    assert!(<[bool; 3]>::try_from_value(&value).unwrap_err().is_wrong_type());
}

#[test]
fn from_array_fail_length() {
    let value = vec![1, 2, 3].try_to_value().unwrap();
    assert!(<[i32; 2]>::try_from_value(&value).unwrap_err().is_parameter_mismatch());
}

#[test]
fn to_cow_owned() {
    let value: Cow<'_, str> = Cow::Owned(String::from("Hello, World!"));
    let expected = Value::String(String::from("Hello, World!"));

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn to_cow_borrowed() {
    let value = Cow::Borrowed("Hello, World!");
    let expected = Value::String(String::from("Hello, World!"));

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn to_cow_owned_bytes() {
    #[allow(clippy::owned_cow)]
    let value: Cow<'_, Vec<u8>> = Cow::Owned(b"123".to_vec());
    let expected = Value::Base64(b"123".to_vec());

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn to_cow_borrowed_bytes() {
    let bytes = b"123".to_vec();
    let value = Cow::Borrowed(&bytes);
    let expected = Value::Base64(b"123".to_vec());

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn from_cow() {
    let value = Value::String(String::from("MOO!"));
    let expected: Cow<'_, str> = Cow::Owned(String::from("MOO!"));

    assert_eq!(<Cow<'_, str>>::try_from_value(&value).unwrap(), expected);
}

#[test]
fn to_box() {
    let value = Box::new(-12);
    let expected = Value::Integer(-12);

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn from_box() {
    let value = Value::Integer(-12);
    let expected = Box::new(-12);

    assert_eq!(<Box<i32>>::try_from_value(&value).unwrap(), expected);
}

#[test]
fn to_rc() {
    let value = Rc::new(-12);
    let expected = Value::Integer(-12);

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn from_rc() {
    let value = Value::Integer(-12);
    let expected = Rc::new(-12);

    assert_eq!(<Rc<i32>>::try_from_value(&value).unwrap(), expected);
}

#[test]
fn to_arc() {
    let value = Arc::new(-12);
    let expected = Value::Integer(-12);

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn from_arc() {
    let value = Value::Integer(-12);
    let expected = Arc::new(-12);

    assert_eq!(<Arc<i32>>::try_from_value(&value).unwrap(), expected);
}

#[cfg(feature = "derive")]
#[test]
fn to_hashmap() {
    use crate::TryToValue;
    use std::collections::HashMap;

    #[derive(Debug, PartialEq, TryToValue)]
    struct TestMap {
        foo: i32,
        bar: i32,
    }

    let value = TestMap { foo: 1, bar: 2 };

    let expected = {
        let mut value = HashMap::new();
        value.insert(String::from("foo"), 1);
        value.insert(String::from("bar"), 2);
        value.try_to_value().unwrap()
    };

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[cfg(feature = "derive")]
#[test]
fn from_hashmap() {
    use crate::TryFromValue;
    use std::collections::HashMap;

    #[derive(Debug, PartialEq, TryFromValue)]
    struct TestMap {
        foo: i32,
        bar: i32,
    }

    let value = {
        let mut value = HashMap::new();
        value.insert(String::from("foo"), 1);
        value.insert(String::from("bar"), 2);
        value.try_to_value().unwrap()
    };

    let expected = TestMap { foo: 1, bar: 2 };

    assert_eq!(TestMap::try_from_value(&value).unwrap(), expected);
}

#[cfg(feature = "derive")]
#[test]
fn from_hashmap_fail() {
    use crate::TryFromValue;
    use std::collections::HashMap;

    #[derive(Debug, PartialEq, TryFromValue)]
    struct TestMap {
        foo: i32,
        bar: i32,
    }

    let value = {
        let mut value = HashMap::new();
        value.insert(String::from("foo"), Value::Integer(1));
        value.insert(String::from("bar"), Value::Boolean(true));
        value.try_to_value().unwrap()
    };

    assert!(TestMap::try_from_value(&value).unwrap_err().is_wrong_type());
}

#[test]
fn from_unit() {
    let value = Vec::<Value>::new().try_to_value().unwrap();
    <()>::try_from_value(&value).unwrap();
}

#[cfg(feature = "nil")]
#[test]
fn from_unit_nil() {
    let value = Value::Nil;
    <()>::try_from_value(&value).unwrap();
}

#[cfg(feature = "nil")]
#[test]
fn from_unit_nil_fail() {
    let value = Value::Integer(1);
    assert!(<()>::try_from_value(&value).unwrap_err().is_wrong_type());
}

#[test]
fn from_unit_fail_length() {
    let value = vec![1].try_to_value().unwrap();
    assert!(<()>::try_from_value(&value).unwrap_err().is_parameter_mismatch());
}

#[test]
fn to_tuple_1() {
    let value = (true,);
    let expected = vec![Value::Boolean(true)].try_to_value().unwrap();

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn from_tuple_1() {
    let value = vec![Value::Boolean(true)].try_to_value().unwrap();
    let expected = (true,);

    assert_eq!(<(bool,)>::try_from_value(&value).unwrap(), expected);
}

#[test]
fn from_tuple_1_fail() {
    let value = Value::Boolean(true);
    assert!(<(bool,)>::try_from_value(&value).unwrap_err().is_wrong_type());
}

#[test]
fn to_tuple_2() {
    let value = (true, 1);
    let expected = vec![Value::Boolean(true), Value::Integer(1)].try_to_value().unwrap();

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn from_tuple_2() {
    let value = vec![Value::Boolean(true), Value::Integer(1)].try_to_value().unwrap();
    let expected = (true, 1);

    assert_eq!(<(bool, i32)>::try_from_value(&value).unwrap(), expected);
}

#[test]
fn from_tuple_2_fail() {
    let value = Value::Boolean(true);
    assert!(<(bool, i32)>::try_from_value(&value).unwrap_err().is_wrong_type());
}

#[test]
fn to_tuple_3() {
    let value = (true, 1, 2.5);
    let expected = vec![Value::Boolean(true), Value::Integer(1), Value::Double(2.5)]
        .try_to_value()
        .unwrap();

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn from_tuple_3() {
    let value = vec![Value::Boolean(true), Value::Integer(1), Value::Double(2.5)]
        .try_to_value()
        .unwrap();
    let expected = (true, 1, 2.5);

    assert_eq!(<(bool, i32, f64)>::try_from_value(&value).unwrap(), expected);
}

#[test]
fn from_tuple_3_fail() {
    let value = Value::Boolean(true);
    assert!(<(bool, i32, f64)>::try_from_value(&value).unwrap_err().is_wrong_type());
}

#[test]
fn to_tuple_4() {
    let value = (true, 1, 2.5, String::from("HELLO"));
    let expected = vec![
        Value::Boolean(true),
        Value::Integer(1),
        Value::Double(2.5),
        Value::String(String::from("HELLO")),
    ]
    .try_to_value()
    .unwrap();

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn from_tuple_4() {
    let value = vec![
        Value::Boolean(true),
        Value::Integer(1),
        Value::Double(2.5),
        Value::String(String::from("HELLO")),
    ]
    .try_to_value()
    .unwrap();
    let expected = (true, 1, 2.5, String::from("HELLO"));

    assert_eq!(<(bool, i32, f64, String)>::try_from_value(&value).unwrap(), expected);
}

#[test]
fn from_tuple_4_fail() {
    let value = Value::Boolean(true);
    assert!(
        <(bool, i32, f64, String)>::try_from_value(&value)
            .unwrap_err()
            .is_wrong_type()
    );
}

#[test]
fn to_tuple_5() {
    let value = (true, 1, 2.5, String::from("HELLO"), -1);
    let expected = vec![
        Value::Boolean(true),
        Value::Integer(1),
        Value::Double(2.5),
        Value::String(String::from("HELLO")),
        Value::Integer(-1),
    ]
    .try_to_value()
    .unwrap();

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn from_tuple_5() {
    let value = vec![
        Value::Boolean(true),
        Value::Integer(1),
        Value::Double(2.5),
        Value::String(String::from("HELLO")),
        Value::Integer(-1),
    ]
    .try_to_value()
    .unwrap();
    let expected = (true, 1, 2.5, String::from("HELLO"), -1);

    assert_eq!(
        <(bool, i32, f64, String, i32)>::try_from_value(&value).unwrap(),
        expected
    );
}

#[test]
fn from_tuple_5_fail() {
    let value = Value::Boolean(true);
    assert!(
        <(bool, i32, f64, String, i32)>::try_from_value(&value)
            .unwrap_err()
            .is_wrong_type()
    );
}

#[test]
fn to_tuple_6() {
    let value = (true, 1, 2.5, String::from("HELLO"), -1, -1.5);
    let expected = vec![
        Value::Boolean(true),
        Value::Integer(1),
        Value::Double(2.5),
        Value::String(String::from("HELLO")),
        Value::Integer(-1),
        Value::Double(-1.5),
    ]
    .try_to_value()
    .unwrap();

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn from_tuple_6() {
    let value = vec![
        Value::Boolean(true),
        Value::Integer(1),
        Value::Double(2.5),
        Value::String(String::from("HELLO")),
        Value::Integer(-1),
        Value::Double(-1.5),
    ]
    .try_to_value()
    .unwrap();
    let expected = (true, 1, 2.5, String::from("HELLO"), -1, -1.5);

    assert_eq!(
        <(bool, i32, f64, String, i32, f64)>::try_from_value(&value).unwrap(),
        expected
    );
}

#[test]
fn from_tuple_6_fail() {
    let value = Value::Boolean(true);
    assert!(
        <(bool, i32, f64, String, i32, f64)>::try_from_value(&value)
            .unwrap_err()
            .is_wrong_type()
    );
}

#[test]
fn to_tuple_7() {
    let value = (true, 1, 2.5, String::from("HELLO"), -1, -1.5, String::from("WORLD"));
    let expected = vec![
        Value::Boolean(true),
        Value::Integer(1),
        Value::Double(2.5),
        Value::String(String::from("HELLO")),
        Value::Integer(-1),
        Value::Double(-1.5),
        Value::String(String::from("WORLD")),
    ]
    .try_to_value()
    .unwrap();

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn from_tuple_7() {
    let value = vec![
        Value::Boolean(true),
        Value::Integer(1),
        Value::Double(2.5),
        Value::String(String::from("HELLO")),
        Value::Integer(-1),
        Value::Double(-1.5),
        Value::String(String::from("WORLD")),
    ]
    .try_to_value()
    .unwrap();
    let expected = (true, 1, 2.5, String::from("HELLO"), -1, -1.5, String::from("WORLD"));

    assert_eq!(
        <(bool, i32, f64, String, i32, f64, String)>::try_from_value(&value).unwrap(),
        expected
    );
}

#[test]
fn from_tuple_7_fail() {
    let value = Value::Boolean(true);
    assert!(
        <(bool, i32, f64, String, i32, f64, String)>::try_from_value(&value)
            .unwrap_err()
            .is_wrong_type()
    );
}

#[test]
fn to_tuple_8() {
    let value = (
        true,
        1,
        2.5,
        String::from("HELLO"),
        -1,
        -1.5,
        String::from("WORLD"),
        false,
    );
    let expected = vec![
        Value::Boolean(true),
        Value::Integer(1),
        Value::Double(2.5),
        Value::String(String::from("HELLO")),
        Value::Integer(-1),
        Value::Double(-1.5),
        Value::String(String::from("WORLD")),
        Value::Boolean(false),
    ]
    .try_to_value()
    .unwrap();

    assert_eq!(value.try_to_value().unwrap(), expected);
}

#[test]
fn from_tuple_8() {
    let value = vec![
        Value::Boolean(true),
        Value::Integer(1),
        Value::Double(2.5),
        Value::String(String::from("HELLO")),
        Value::Integer(-1),
        Value::Double(-1.5),
        Value::String(String::from("WORLD")),
        Value::Boolean(false),
    ]
    .try_to_value()
    .unwrap();
    let expected = (
        true,
        1,
        2.5,
        String::from("HELLO"),
        -1,
        -1.5,
        String::from("WORLD"),
        false,
    );

    assert_eq!(
        <(bool, i32, f64, String, i32, f64, String, bool)>::try_from_value(&value).unwrap(),
        expected
    );
}

#[test]
fn from_tuple_8_fail() {
    let value = Value::Boolean(true);
    assert!(
        <(bool, i32, f64, String, i32, f64, String, bool)>::try_from_value(&value)
            .unwrap_err()
            .is_wrong_type()
    );
}
