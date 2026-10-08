use crate::fault::Fault;
use crate::serde::{XmlFaultResponse, XmlMethodResponse, XmlValue};
use crate::xml::{deserialize_xml as from_str, serialize_xml as to_string};

#[test]
fn to_method_response_success() {
    let value = XmlMethodResponse::new(XmlValue::string(Into::into("Success!")));
    let expected =
        "<methodResponse><params><param><value><string>Success!</string></value></param></params></methodResponse>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[test]
fn from_method_response_success() {
    let value =
        "<methodResponse><params><param><value><string>Success!</string></value></param></params></methodResponse>";
    let expected = XmlMethodResponse::new(XmlValue::string(Into::into("Success!")));

    assert_eq!(from_str::<XmlMethodResponse>(value).unwrap(), expected);
}

#[test]
fn to_method_response_fault() {
    let value = XmlFaultResponse::from(Fault::new(4, String::from("Too many parameters.")));
    let expected = "<methodResponse><fault><value><struct><member><name>faultCode</name><value><i4>4</i4></value></member><member><name>faultString</name><value><string>Too many parameters.</string></value></member></struct></value></fault></methodResponse>";

    assert_eq!(to_string(&value).unwrap(), expected);
}

#[test]
fn from_method_response_fault() {
    let value = "<methodResponse><fault><value><struct><member><name>faultCode</name><value><i4>4</i4></value></member><member><name>faultString</name><value><string>Too many parameters.</string></value></member></struct></value></fault></methodResponse>";
    let expected = XmlFaultResponse::from(Fault::new(4, String::from("Too many parameters.")));

    assert_eq!(from_str::<XmlFaultResponse>(value).unwrap(), expected);
}
