#![allow(clippy::needless_pass_by_value)]

use std::borrow::Cow;

use quickcheck::TestResult;
use quickcheck_macros::quickcheck;

use crate::serde::XmlValue;
use crate::xml::{deserialize_xml as from_str, serialize_xml as to_string};
use crate::{TryFromValue, TryToValue};

#[test]
fn to_i4() {
    let value = XmlValue::i4(-12);
    let expected = "<value><i4>-12</i4></value>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[test]
fn from_i4() {
    let value = "<value><i4>-12</i4></value>";
    let expected = XmlValue::i4(-12);

    assert_eq!(from_str::<XmlValue>(value).unwrap(), expected);
}

#[test]
fn from_int() {
    let value = "<value><int>-12</int></value>";
    let expected = XmlValue::i4(-12);

    assert_eq!(from_str::<XmlValue>(value).unwrap(), expected);
}

#[cfg(feature = "i8")]
#[test]
fn to_i8() {
    let value = XmlValue::i8(-12);
    let expected = "<value><i8>-12</i8></value>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[cfg(feature = "i8")]
#[test]
fn from_i8() {
    let value = "<value><i8>-12</i8></value>";
    let expected = XmlValue::i8(-12);

    assert_eq!(from_str::<XmlValue>(value).unwrap(), expected);
}

#[test]
fn to_boolean() {
    let value = XmlValue::boolean(true);
    let expected = "<value><boolean>1</boolean></value>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[test]
fn from_boolean() {
    let value = "<value><boolean>1</boolean></value>";
    let expected = XmlValue::boolean(true);

    assert_eq!(from_str::<XmlValue>(value).unwrap(), expected);
}

#[test]
fn from_boolean_fail() {
    let value = "<value><boolean>hello</boolean></value>";

    assert!(
        from_str::<XmlValue>(value)
            .unwrap_err()
            .to_string()
            .contains("Unsupported boolean value")
    );
}

#[test]
fn to_str() {
    let value = XmlValue::string(Into::into("Hello, World!"));
    let expected = "<value><string>Hello, World!</string></value>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[test]
fn from_string() {
    let value = "<value><string>Hello, World!</string></value>";
    let expected = XmlValue::string(Into::into("Hello, World!"));

    assert_eq!(from_str::<XmlValue>(value).unwrap(), expected);
}

#[test]
fn to_str_with_escape() {
    let value = XmlValue::string(Into::into("a&b"));
    let expected = "<value><string>a&amp;b</string></value>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[test]
fn from_string_with_escape() {
    let value = "<value><string>a&amp;b</string></value>";
    let expected = XmlValue::string(Into::into("a&b"));

    assert_eq!(from_str::<XmlValue>(value).unwrap(), expected);
}

#[test]
fn from_untyped_string() {
    let value = "<value>Hello, World!</value>";
    let expected = XmlValue::string(Into::into("Hello, World!"));

    assert_eq!(from_str::<XmlValue>(value).unwrap(), expected);
}

#[test]
fn from_untyped_empty_string() {
    let value = "<value></value>";
    let expected = XmlValue::string(Into::into(""));

    assert_eq!(from_str::<XmlValue>(value).unwrap(), expected);
}

#[test]
fn from_untyped_empty_string_self_closing() {
    let value = "<value />";
    let expected = XmlValue::string(Into::into(""));

    assert_eq!(from_str::<XmlValue>(value).unwrap(), expected);
}

#[test]
fn from_untyped_string_with_escape() {
    let value = "<value>a&amp;b</value>";
    let expected = XmlValue::string(Into::into("a&b"));

    assert_eq!(from_str::<XmlValue>(value).unwrap(), expected);
}

#[test]
fn to_double() {
    let value = XmlValue::double(1.5);
    let expected = "<value><double>1.5</double></value>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[test]
fn from_double() {
    let value = "<value><double>1.5</double></value>";
    let expected = XmlValue::double(1.5);

    assert_eq!(from_str::<XmlValue>(value).unwrap(), expected);
}

#[cfg(feature = "chrono")]
#[test]
fn to_datetime() {
    use crate::datetime::DateTime;
    use chrono::Utc;

    let datetime = DateTime::from(Utc::now().naive_utc());
    let datetime_str = datetime.to_string();

    let value = XmlValue::datetime(datetime);
    let expected = format!("<value><dateTime.iso8601>{datetime_str}</dateTime.iso8601></value>");

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[cfg(feature = "chrono")]
#[test]
fn from_datetime() {
    use crate::datetime::DateTime;
    use chrono::{SubsecRound, Utc};

    let datetime = DateTime::from(Utc::now().round_subsecs(0).naive_utc());
    let datetime_str = datetime.to_string();

    let value = format!("<value><dateTime.iso8601>{datetime_str}</dateTime.iso8601></value>");
    let expected = XmlValue::datetime(datetime);

    assert_eq!(from_str::<XmlValue>(&value).unwrap(), expected);
}

#[test]
fn from_datetime_fail() {
    let value = "<value><dateTime.iso8601>202520252025</dateTime.iso8601></value>";
    assert!(
        from_str::<XmlValue>(value)
            .unwrap_err()
            .to_string()
            .contains("Invalid format for dateTime.iso8601 value")
    );
}

#[test]
fn to_base64() {
    let contents = b"you can't read this!";
    let encoded = crate::base64::encode(contents);

    let value = XmlValue::base64(contents.to_vec().into());
    let expected = format!("<value><base64>{encoded}</base64></value>");

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[test]
fn from_base64() {
    let contents = b"you can't read this!";
    let encoded = crate::base64::encode(contents);

    let value = format!("<value><base64>{encoded}</base64></value>");
    let expected = XmlValue::base64(contents.to_vec().into());

    assert_eq!(from_str::<XmlValue>(&value).unwrap(), expected);
}

#[cfg(feature = "nil")]
#[test]
fn to_nil() {
    let value = XmlValue::nil();
    let expected = "<value><nil></nil></value>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[cfg(feature = "nil")]
#[test]
fn from_nil() {
    let value = "<value><nil/></value>";
    let expected = XmlValue::nil();

    assert_eq!(from_str::<XmlValue>(value).unwrap(), expected);
}

#[quickcheck]
fn to_from_i4(int: i32) -> bool {
    let value = XmlValue::i4(int);

    value == from_str::<XmlValue>(&to_string(&value).unwrap()).unwrap()
}

#[quickcheck]
fn from_to_i4(int: i32) -> bool {
    let value = format!("<value><i4>{int}</i4></value>");

    value == to_string(&from_str::<XmlValue>(&value).unwrap()).unwrap()
}

#[cfg(feature = "i8")]
#[quickcheck]
fn to_from_i8(long: i64) -> bool {
    let value = XmlValue::i8(long);

    value == from_str::<XmlValue>(&to_string(&value).unwrap()).unwrap()
}

#[cfg(feature = "i8")]
#[quickcheck]
fn from_to_i8(long: i64) -> bool {
    let value = format!("<value><i8>{long}</i8></value>");

    value == to_string(&from_str::<XmlValue>(&value).unwrap()).unwrap()
}

#[quickcheck]
fn to_from_boolean(boolean: bool) -> bool {
    let value = XmlValue::boolean(boolean);

    value == from_str::<XmlValue>(&to_string(&value).unwrap()).unwrap()
}

#[quickcheck]
fn from_to_boolean(boolean: bool) -> bool {
    let value = format!("<value><boolean>{}</boolean></value>", i32::from(boolean));

    value == to_string(&from_str::<XmlValue>(&value).unwrap()).unwrap()
}

#[quickcheck]
fn to_from_string(string: String) -> bool {
    let value = XmlValue::string(string.trim().into());

    value == from_str::<XmlValue>(&to_string(&value).unwrap()).unwrap()
}

#[quickcheck]
fn from_to_string(string: String) -> bool {
    let string = quick_xml::escape::escape(string.trim()).to_string();
    let value = format!("<value><string>{string}</string></value>");

    value == to_string(&from_str::<XmlValue>(&value).unwrap()).unwrap()
}

#[quickcheck]
fn to_from_double(double: f64) -> TestResult {
    if double.is_nan() {
        return TestResult::discard();
    }

    let value = XmlValue::double(double);

    TestResult::from_bool(value == from_str::<XmlValue>(&to_string(&value).unwrap()).unwrap())
}

#[quickcheck]
fn from_to_double(double: f64) -> TestResult {
    if double.is_nan() {
        return TestResult::discard();
    }

    let value = format!("<value><double>{double}</double></value>");

    TestResult::from_bool(value == to_string(&from_str::<XmlValue>(&value).unwrap()).unwrap())
}

#[quickcheck]
fn to_from_base64(bytes: Vec<u8>) -> bool {
    let value = XmlValue::base64(bytes.into());

    value == from_str::<XmlValue>(&to_string(&value).unwrap()).unwrap()
}

#[quickcheck]
fn from_to_base64(bytes: Vec<u8>) -> bool {
    let value = format!("<value><base64>{}</base64></value>", crate::base64::encode(bytes));

    value == to_string(&from_str::<XmlValue>(&value).unwrap()).unwrap()
}

#[cfg(feature = "nil")]
#[quickcheck]
fn roundtrip_option_some(a: i32) -> bool {
    let value = Some(a);
    <Option<i32>>::try_from_value(&value.try_to_value().unwrap()).unwrap() == value
}

#[quickcheck]
fn roundtrip_cow_string(string: String) -> bool {
    let expected: Cow<str> = Cow::Owned(string.trim().to_owned());
    let value = <Cow<str>>::try_from_value(&TryToValue::try_to_value(&expected).unwrap()).unwrap();

    expected == value
}

#[quickcheck]
fn roundtrip_array(a: i32, b: i32) -> bool {
    let value = vec![a, b];
    <Vec<i32>>::try_from_value(&value.try_to_value().unwrap()).unwrap() == value
}

#[cfg(feature = "derive")]
#[test]
fn roundtrip_struct_empty() {
    use crate::{TryFromValue, TryToValue};

    #[derive(Debug, Eq, PartialEq, TryFromValue, TryToValue)]
    struct Test {}

    let value = Test {};
    assert_eq!(Test::try_from_value(&value.try_to_value().unwrap()).unwrap(), value);
}

#[cfg(all(feature = "derive", feature = "nil"))]
#[quickcheck]
fn roundtrip_struct(int: i32, string: String, boolean: bool, optional: Option<f64>) -> TestResult {
    if matches!(optional, Some(f) if f.is_nan()) {
        return TestResult::discard();
    }

    #[allow(clippy::items_after_statements)]
    #[derive(Debug, PartialEq, TryFromValue, TryToValue)]
    struct Test {
        int: i32,
        string: String,
        boolean: bool,
        optional: Option<f64>,
    }

    let value = Test {
        int,
        string: string.trim().to_string(),
        boolean,
        optional,
    };
    TestResult::from_bool(Test::try_from_value(&value.try_to_value().unwrap()).unwrap() == value)
}

#[cfg(feature = "derive")]
#[quickcheck]
fn roundtrip_struct_cow_str(string: String) -> bool {
    #[derive(Debug, Eq, PartialEq, TryFromValue, TryToValue)]
    struct TestCow<'a> {
        string: Cow<'a, str>,
    }

    let expected = TestCow {
        string: Cow::Owned(string.trim().to_owned()),
    };
    let value = TestCow::try_from_value(&TryToValue::try_to_value(&expected).unwrap()).unwrap();

    expected == value
}

#[cfg(feature = "derive")]
#[quickcheck]
fn roundtrip_struct_cow_bytes(bytes: Vec<u8>) -> bool {
    #[allow(clippy::owned_cow)]
    #[derive(Debug, Eq, PartialEq, TryFromValue, TryToValue)]
    struct TestCow<'a> {
        bytes: Cow<'a, Vec<u8>>,
    }

    let expected = TestCow {
        bytes: Cow::Owned(bytes),
    };
    let value = TestCow::try_from_value(&TryToValue::try_to_value(&expected).unwrap()).unwrap();

    expected == value
}

#[cfg(feature = "derive")]
#[quickcheck]
fn roundtrip_struct_cow_static_str(string: String) -> bool {
    #[derive(Debug, Eq, PartialEq, TryFromValue, TryToValue)]
    struct TestCow {
        string: Cow<'static, str>,
    }

    let expected = TestCow {
        string: Cow::Owned(string.trim().to_owned()),
    };
    let value = TestCow::try_from_value(&TryToValue::try_to_value(&expected).unwrap()).unwrap();

    expected == value
}
