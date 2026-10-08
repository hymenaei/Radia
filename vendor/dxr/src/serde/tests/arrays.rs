use crate::serde::{XmlArray, XmlValue};
use crate::xml::{deserialize_xml as from_str, serialize_xml as to_string};

#[test]
fn to_array_empty() {
    let value = XmlArray::new(vec![]);
    let expected = "<array><data></data></array>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[test]
fn from_array_empty() {
    let value = "<array><data/></array>";
    let expected = XmlArray::new(vec![]);

    assert_eq!(from_str::<XmlArray>(value).unwrap(), expected);

    let value = "<array><data></data></array>";
    let expected = XmlArray::new(vec![]);

    assert_eq!(from_str::<XmlArray>(value).unwrap(), expected);
}

#[test]
fn to_array_one() {
    let value = XmlArray::new(vec![XmlValue::i4(-12)]);
    let expected = "<array><data><value><i4>-12</i4></value></data></array>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[test]
fn from_array_one() {
    let value = "<array><data><value><i4>-12</i4></value></data></array>";
    let expected = XmlArray::new(vec![XmlValue::i4(-12)]);

    assert_eq!(from_str::<XmlArray>(value).unwrap(), expected);
}

#[test]
fn to_array_two() {
    let value = XmlArray::new(vec![XmlValue::i4(-12), XmlValue::string(Into::into("minus twelve"))]);
    let expected =
        "<array><data><value><i4>-12</i4></value><value><string>minus twelve</string></value></data></array>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[test]
fn from_array_two() {
    let value = "<array><data><value><i4>-12</i4></value><value><string>minus twelve</string></value></data></array>";
    let expected = XmlArray::new(vec![XmlValue::i4(-12), XmlValue::string(Into::into("minus twelve"))]);

    assert_eq!(from_str::<XmlArray>(value).unwrap(), expected);
}

#[test]
fn to_value_array() {
    let value = XmlValue::array(XmlArray::new(vec![XmlValue::i4(-12)]));
    let expected = "<value><array><data><value><i4>-12</i4></value></data></array></value>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[test]
fn from_value_array() {
    let value = "<value><array><data><value><i4>-12</i4></value></data></array></value>";
    let expected = XmlValue::array(XmlArray::new(vec![XmlValue::i4(-12)]));

    assert_eq!(from_str::<XmlValue>(value).unwrap(), expected);
}
