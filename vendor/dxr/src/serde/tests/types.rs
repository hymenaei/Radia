#![allow(clippy::needless_pass_by_value)]

use quickcheck::TestResult;
use quickcheck_macros::quickcheck;

use crate::serde::XmlType;
use crate::xml::{deserialize_xml as from_str, serialize_xml as to_string};

#[test]
fn to_i4() {
    let value = XmlType::Integer(-12);
    let expected = "<i4>-12</i4>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[test]
fn from_i4() {
    let value = "<i4>-12</i4>";
    let expected = XmlType::Integer(-12);

    assert_eq!(from_str::<XmlType>(value).unwrap(), expected);
}

#[test]
fn from_int() {
    let value = "<int>-12</int>";
    let expected = XmlType::Integer(-12);

    assert_eq!(from_str::<XmlType>(value).unwrap(), expected);
}

#[cfg(feature = "i8")]
#[test]
fn to_i8() {
    let value = XmlType::Long(-12);
    let expected = "<i8>-12</i8>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[cfg(feature = "i8")]
#[test]
fn from_i8() {
    let value = "<i8>-12</i8>";
    let expected = XmlType::Long(-12);

    assert_eq!(from_str::<XmlType>(value).unwrap(), expected);
}

#[test]
fn to_boolean() {
    let value = XmlType::Boolean(false);
    let expected = "<boolean>0</boolean>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[test]
fn from_boolean() {
    let value = "<boolean>0</boolean>";
    let expected = XmlType::Boolean(false);

    assert_eq!(from_str::<XmlType>(value).unwrap(), expected);
}

#[test]
fn to_str() {
    let value = XmlType::String(Into::into("Hello, World!"));
    let expected = "<string>Hello, World!</string>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[test]
fn from_string() {
    let value = "<string>Hello, World!</string>";
    let expected = XmlType::String(Into::into("Hello, World!"));

    assert_eq!(from_str::<XmlType>(value).unwrap(), expected);
}

#[test]
fn to_double() {
    let value = XmlType::Double(1.5);
    let expected = "<double>1.5</double>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[test]
fn from_double() {
    let value = "<double>1.5</double>";
    let expected = XmlType::Double(1.5);

    assert_eq!(from_str::<XmlType>(value).unwrap(), expected);
}

#[cfg(feature = "chrono")]
#[test]
fn to_datetime() {
    use crate::datetime::DateTime;
    use chrono::Utc;

    let datetime = DateTime::from(Utc::now().naive_utc());
    let datetime_str = datetime.to_string();

    let value = XmlType::DateTime(datetime);
    let expected = format!("<dateTime.iso8601>{datetime_str}</dateTime.iso8601>");

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[cfg(feature = "chrono")]
#[test]
fn from_datetime() {
    use crate::datetime::DateTime;
    use chrono::{SubsecRound, Utc};

    let datetime = DateTime::from(Utc::now().round_subsecs(0).naive_utc());
    let datetime_str = datetime.to_string();

    let value = format!("<dateTime.iso8601>{datetime_str}</dateTime.iso8601>");
    let expected = XmlType::DateTime(datetime);

    assert_eq!(from_str::<XmlType>(&value).unwrap(), expected);
}

#[test]
fn to_base64() {
    let contents = b"you can't read this!";
    let encoded = crate::base64::encode(contents);

    let value = XmlType::Base64(contents.to_vec().into());
    let expected = format!("<base64>{encoded}</base64>");

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[test]
fn from_base64() {
    let contents = b"you can't read this!";
    let encoded = crate::base64::encode(contents);

    let value = format!("<base64>{encoded}</base64>");
    let expected = XmlType::Base64(contents.to_vec().into());

    assert_eq!(from_str::<XmlType>(&value).unwrap(), expected);
}

#[cfg(feature = "nil")]
#[test]
fn to_nil() {
    let value = XmlType::Nil;
    let expected = "<nil></nil>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[cfg(feature = "nil")]
#[test]
fn from_nil() {
    let value = "<nil/>";
    let expected = XmlType::Nil;

    assert_eq!(from_str::<XmlType>(value).unwrap(), expected);
}

#[quickcheck]
fn to_from_i4(int: i32) -> bool {
    let value = XmlType::Integer(int);

    value == from_str::<XmlType>(&to_string(&value).unwrap()).unwrap()
}

#[quickcheck]
fn from_to_i4(int: i32) -> bool {
    let value = format!("<i4>{int}</i4>");

    value == to_string(&from_str::<XmlType>(&value).unwrap()).unwrap()
}

#[cfg(feature = "i8")]
#[quickcheck]
fn to_from_i8(long: i64) -> bool {
    let value = XmlType::Long(long);

    value == from_str::<XmlType>(&to_string(&value).unwrap()).unwrap()
}

#[cfg(feature = "i8")]
#[quickcheck]
fn from_to_i8(long: i64) -> bool {
    let value = format!("<i8>{long}</i8>");

    value == to_string(&from_str::<XmlType>(&value).unwrap()).unwrap()
}

#[quickcheck]
fn to_from_boolean(boolean: bool) -> bool {
    let value = XmlType::Boolean(boolean);

    value == from_str::<XmlType>(&to_string(&value).unwrap()).unwrap()
}

#[quickcheck]
fn from_to_boolean(boolean: bool) -> bool {
    let value = format!("<boolean>{}</boolean>", i32::from(boolean));

    value == to_string(&from_str::<XmlType>(&value).unwrap()).unwrap()
}

#[quickcheck]
fn to_from_string(string: String) -> bool {
    let value = XmlType::String(string.trim().into());

    value == from_str::<XmlType>(&to_string(&value).unwrap()).unwrap()
}

#[quickcheck]
fn from_to_string(string: String) -> bool {
    let string = quick_xml::escape::escape(string.trim()).to_string();
    let value = format!("<string>{string}</string>");

    value == to_string(&from_str::<XmlType>(&value).unwrap()).unwrap()
}

#[quickcheck]
fn to_from_double(double: f64) -> TestResult {
    if double.is_nan() {
        return TestResult::discard();
    }

    let value = XmlType::Double(double);

    TestResult::from_bool(value == from_str::<XmlType>(&to_string(&value).unwrap()).unwrap())
}

#[quickcheck]
fn from_to_double(double: f64) -> TestResult {
    if double.is_nan() {
        return TestResult::discard();
    }

    let value = format!("<double>{double}</double>");

    TestResult::from_bool(value == to_string(&from_str::<XmlType>(&value).unwrap()).unwrap())
}

#[quickcheck]
fn to_from_base64(bytes: Vec<u8>) -> bool {
    let value = XmlType::Base64(bytes.into());

    value == from_str::<XmlType>(&to_string(&value).unwrap()).unwrap()
}

#[quickcheck]
fn from_to_base64(bytes: Vec<u8>) -> bool {
    let value = format!("<base64>{}</base64>", crate::base64::encode(bytes));

    value == to_string(&from_str::<XmlType>(&value).unwrap()).unwrap()
}
