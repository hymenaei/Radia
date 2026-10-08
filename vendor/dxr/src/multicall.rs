//! Helper functions for dealing with "system.multicall" method calls.

use std::collections::HashMap;

use crate::error::Error;
use crate::fault::Fault;
use crate::serde::{XmlArray, XmlStruct, XmlStructMember, XmlValue};
use crate::traits::{TryFromValue, TryToParams, TryToValue};
use crate::types::Value;

/// Convenience method for constructing arguments for "system.multicall" calls.
pub fn into_multicall_params<P>(calls: Vec<(String, P)>) -> Result<Value, Error>
where
    P: TryToParams,
{
    let params: Vec<Value> = calls
        .into_iter()
        .map(|(n, p)| {
            let members = vec![
                XmlStructMember::new(Into::into("methodName"), XmlValue::string(n.into())),
                XmlStructMember::new(Into::into("params"), p.try_to_params()?.try_to_value()?.into()),
            ];
            Ok(Value::from(XmlValue::structure(XmlStruct::new(members))))
        })
        .collect::<Result<Vec<Value>, Error>>()?;

    params.try_to_value()
}

/// Convenience method for reconstructing method calls from "system.multicall" arguments.
#[allow(clippy::type_complexity)]
pub fn from_multicall_params(mut values: Vec<Value>) -> Result<Vec<Result<(String, Vec<Value>), Error>>, Error> {
    // system.multicall calls take an array of arguments as single argument
    let Some(value) = values.pop() else {
        return Err(Error::parameter_mismatch(0, 1));
    };

    // check if there are more than one arguments
    if !values.is_empty() {
        return Err(Error::parameter_mismatch(values.len() + 1, 1));
    }

    // extract vector of argument values
    let params = <Vec<Value>>::try_from_value(&value)?;

    let calls: Vec<Result<(String, Vec<Value>), Error>> = params
        .into_iter()
        .map(|v| {
            let mut members: HashMap<String, Value> = HashMap::try_from_value(&v)?;

            if members.len() != 2 {
                return Err(Error::parameter_mismatch(members.len(), 2));
            }

            let Some(name) = members.remove("methodName") else {
                return Err(Error::missing_field("system.multicall", "methodName"));
            };

            let Some(params) = members.remove("params") else {
                return Err(Error::missing_field("system.multicall", "params"));
            };

            Ok((String::try_from_value(&name)?, <Vec<Value>>::try_from_value(&params)?))
        })
        .collect();

    Ok(calls)
}

/// Convenience method for constructing return values for "system.multicall" calls.
#[must_use]
pub fn into_multicall_response(results: Vec<Result<Value, Fault>>) -> Value {
    let values: Vec<Value> = results
        .into_iter()
        .map(|r| match r {
            Ok(value) => Value::from(XmlValue::array(XmlArray::new(vec![value.into()]))),
            Err(fault) => {
                let members = vec![
                    XmlStructMember::new(Into::into("faultCode"), XmlValue::i4(fault.code())),
                    XmlStructMember::new(Into::into("faultString"), XmlValue::string(fault.string().into())),
                ];
                Value::from(XmlValue::structure(XmlStruct::new(members)))
            },
        })
        .collect();

    Value::Array(values)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    use crate::types::MethodCall;

    #[test]
    fn from_multicall() {
        let string = "\
<methodCall>
    <methodName>system.multicall</methodName>
    <params>
        <param>
            <value>
                <array>
                    <data>
                        <value>
                            <struct>
                                <member>
                                    <name>methodName</name>
                                    <value>event</value>
                                </member>
                                <member>
                                    <name>params</name>
                                    <value>
                                        <array>
                                            <data>
                                                <value>foo</value>
                                                <value>bar</value>
                                                <value>baz</value>
                                                <value><boolean>1</boolean></value>
                                            </data>
                                        </array>
                                    </value>
                                </member>
                            </struct>
                        </value>
                        <value>
                            <struct>
                                <member>
                                    <name>methodName</name>
                                    <value>event</value>
                                </member>
                                <member>
                                    <name>params</name>
                                    <value>
                                        <array>
                                            <data>
                                                <value>another</value>
                                                <value>call</value>
                                                <value>hi</value>
                                                <value><boolean>1</boolean></value>
                                            </data>
                                        </array>
                                    </value>
                                </member>
                            </struct>
                        </value>
                    </data>
                </array>
            </value>
        </param>
    </params>
</methodCall>";

        let call: MethodCall = MethodCall::from_xml(string).unwrap();

        let params = from_multicall_params(call.params).unwrap();

        let expected: Vec<Result<(String, Vec<Value>), Error>> = vec![
            Ok((
                String::from("event"),
                vec![
                    Value::String(String::from("foo")),
                    Value::String(String::from("bar")),
                    Value::String(String::from("baz")),
                    Value::Boolean(true),
                ],
            )),
            Ok((
                String::from("event"),
                vec![
                    Value::String(String::from("another")),
                    Value::String(String::from("call")),
                    Value::String(String::from("hi")),
                    Value::Boolean(true),
                ],
            )),
        ];

        assert_eq!(params, expected);
    }
}
