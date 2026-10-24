use std::assert_matches;
use std::{collections::BTreeMap, fmt, io::Cursor};

use llsd::{Binary, Date, Error, Limits, Map, Uri, Value, binary, notation, xml};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Serialize, Serializer};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Profile {
    active: bool,
    name: String,
    score: i32,
}

#[test]
fn special_reals_preserve_values() {
    for (text, expected) in [("nan", f64::NAN), ("inf", f64::INFINITY), ("-inf", f64::NEG_INFINITY), ("+INFINITY", f64::INFINITY)] {
        for actual in [
            xml::from_str::<f64>(&format!("<llsd><real>{text}</real></llsd>")).unwrap(),
            notation::from_str::<f64>(&format!("r{text}")).unwrap(),
        ] {
            if expected.is_nan() {
                assert!(actual.is_nan(), "{text}");
            } else {
                assert_eq!(actual, expected, "{text}");
            }
        }
    }
    for (value, text) in [(f64::NAN, "nan"), (f64::INFINITY, "inf"), (f64::NEG_INFINITY, "-inf")] {
        assert_eq!(notation::to_string(&value).unwrap(), format!("<?llsd/notation?>\nr{text}"));
        assert_eq!(xml::to_string(&value).unwrap(), format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><llsd><real>{text}</real></llsd>"));
    }
    assert_matches!(xml::from_str::<f64>("<llsd><real>not-a-number</real></llsd>"), Err(Error::InvalidValue(_)));
    assert_matches!(notation::from_str::<f64>("rnot-a-number"), Err(Error::InvalidValue(_)));
}

#[derive(Debug, Deserialize)]
struct IgnoreUnknown {}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct UuidField {
    #[serde(with = "llsd::uuid")]
    id: Uuid,
}

struct OversizedSequence(usize);

impl Serialize for OversizedSequence {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_seq(Some(self.0))?.end()
    }
}

struct NestedSequence(usize);

struct UnknownLengthContainers;

impl Serialize for UnknownLengthContainers {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        struct Sequence;
        impl Serialize for Sequence {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut seq = serializer.serialize_seq(None)?;
                seq.serialize_element(&17_i32)?;
                seq.serialize_element(&"x")?;
                seq.end()
            }
        }
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("items", &Sequence)?;
        map.serialize_entry("empty", &Vec::<i32>::new())?;
        map.end()
    }
}

#[test]
fn unknown_lengths_use_emitted_counts() {
    let value = UnknownLengthContainers;
    let binary = binary::to_vec(&value).expect("encode unknown lengths");
    assert_eq!(binary, b"<?llsd/binary?>\n{\x00\x00\x00\x02k\x00\x00\x00\x05items[\x00\x00\x00\x02i\x00\x00\x00\x11s\x00\x00\x00\x01x]k\x00\x00\x00\x05empty[\x00\x00\x00\x00]}");
    let expected = Value::Map(Map::from([
        ("items".into(), Value::Array(vec![Value::Integer(17), Value::String("x".into())])),
        ("empty".into(), Value::Array(Vec::new())),
    ]));
    assert_eq!(binary::from_slice::<Value>(&binary).unwrap(), expected);
    assert_eq!(xml::from_slice::<Value>(&xml::to_vec(&value).unwrap()).unwrap(), expected);
    assert_eq!(notation::to_vec(&value).unwrap(), b"<?llsd/notation?>\n{'items':[i17,'x'],'empty':[]}");
    assert_xml_limit(&value);
}

impl Serialize for NestedSequence {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.0 == 0 {
            return serializer.serialize_seq(Some(0))?.end();
        }

        let mut sequence = serializer.serialize_seq(Some(1))?;
        sequence.serialize_element(&Self(self.0 - 1))?;
        sequence.end()
    }
}

#[derive(Serialize)]
struct BoundedTupleStruct(i32, String);

#[derive(Serialize)]
enum BoundedEnum {
    Unit,
    Newtype(Value),
    Tuple(i32, String),
    Struct { label: String, value: Value },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
enum WireEnum {
    Unit,
    Newtype(i32),
    Tuple(i32, String),
    Struct { label: String, count: i32 },
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct Counter(i32);

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct WrappedProfile(Profile);

#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct RepeatedText {
    character: char,
    count: usize,
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct FormatAwareKey;

impl fmt::Display for RepeatedText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for _ in 0..self.count {
            write!(formatter, "{}", self.character)?;
        }
        Ok(())
    }
}

impl Serialize for RepeatedText {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl Serialize for FormatAwareKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() { serializer.serialize_str("key") } else { serializer.serialize_bytes(b"key") }
    }
}

fn assert_xml_limit<T: Serialize>(value: &T) {
    let encoded = xml::to_vec(value).expect("encode XML value");
    assert_eq!(xml::to_vec_with_max_bytes(value, encoded.len()).expect("exact XML limit"), encoded);
    assert_matches!(xml::to_vec_with_max_bytes(value, encoded.len() - 1), Err(Error::LimitExceeded("encoded length")));
}

fn profile() -> Profile {
    Profile { active: true, name: "A&B <viewer>".into(), score: -17 }
}

fn assert_ignored_limit(inputs: [&[u8]; 3], limits: Limits, expected: &'static str) {
    let results = [
        ("XML", xml::from_slice_with_limits::<IgnoreUnknown>(inputs[0], limits)),
        ("notation", notation::from_slice_with_limits::<IgnoreUnknown>(inputs[1], limits)),
        ("binary", binary::from_slice_with_limits::<IgnoreUnknown>(inputs[2], limits)),
    ];

    for (format, result) in results {
        assert_matches!(result, Err(Error::LimitExceeded(actual)) if actual == expected, "{format}: expected {expected}, got {result:?}");
    }
}

fn llsd_date(value: &str) -> Date {
    Date::from_datetime(OffsetDateTime::parse(value, &Rfc3339).expect("valid LLSD date"))
}

#[test]
fn python_fixtures_decode() {
    let expected = Map::from([("foo".into(), Value::String("bar".into()))]);
    let xml = include_bytes!("fixtures/map_foo_bar.xml");
    let notation = include_bytes!("fixtures/map_foo_bar.notation");
    let binary = include_bytes!("fixtures/map_foo_bar.bin");

    assert_eq!(xml::from_slice::<Map>(xml).expect("Python XML fixture"), expected);
    assert_eq!(notation::from_slice::<Map>(notation).expect("Python notation fixture"), expected);
    assert_eq!(binary::from_slice::<Map>(binary).expect("Python binary fixture"), expected);
}

#[test]
fn unknown_fields_are_ignored() {
    let mut fields = BTreeMap::new();
    fields.insert("active".into(), Value::Boolean(true));
    fields.insert("name".into(), Value::String("A&B <viewer>".into()));
    fields.insert("score".into(), Value::Integer(-17));
    fields.insert(
        "future".into(),
        Value::Array(vec![
            Value::Map(BTreeMap::from([("nested".into(), Value::Binary(Binary::from(vec![0, 255])))])),
            Value::String("additional data".into()),
        ]),
    );
    let payload = Value::Map(fields);
    let expected = profile();

    assert_eq!(xml::from_slice::<Profile>(&xml::to_vec(&payload).expect("encode XML")).expect("decode XML"), expected);
    assert_eq!(notation::from_slice::<Profile>(&notation::to_vec(&payload).expect("encode notation")).expect("decode notation"), expected);
    assert_eq!(binary::from_slice::<Profile>(&binary::to_vec(&payload).expect("encode binary")).expect("decode binary"), expected);
}

#[test]
fn ignored_values_keep_limits() {
    let payload = Value::Map(BTreeMap::from([(
        "x".into(),
        Value::Array(vec![Value::Map(BTreeMap::from([
            ("nested".into(), Value::String("long".into())),
            ("other".into(), Value::Binary(Binary::from(vec![0, 255]))),
        ]))]),
    )]));
    let inputs = [
        xml::to_vec(&payload).expect("encode XML"),
        notation::to_vec(&payload).expect("encode notation"),
        binary::to_vec(&payload).expect("encode binary"),
    ];
    let cases = [
        (Limits { max_depth: 1, ..Limits::default() }, "nesting depth"),
        (Limits { max_values: 3, ..Limits::default() }, "value count"),
        (Limits { max_container_items: 1, ..Limits::default() }, "container items"),
        (Limits { max_string_bytes: 3, ..Limits::default() }, "string bytes"),
        (Limits { max_map_key_bytes: 1, ..Limits::default() }, "map key bytes"),
        (Limits { max_binary_bytes: 1, ..Limits::default() }, "binary bytes"),
    ];

    for (limits, expected) in cases {
        assert_ignored_limit([&inputs[0], &inputs[1], &inputs[2]], limits, expected);
    }
}

#[test]
fn xml_accepts_doctype() {
    let input = b"<!DOCTYPE llsd [<!ELEMENT llsd (undef)><!ELEMENT undef EMPTY>]><llsd><undef/></llsd>";

    assert_eq!(xml::from_slice::<Value>(input).expect("decode LLSD with its DTD"), Value::Undefined);
    assert!(xml::value_allocation_bound(input, Limits::default()).is_ok());
    assert!(xml::from_str::<Value>("<!DOCTYPE llsd [<!ENTITY custom 'value'>]><llsd><string>&custom;</string></llsd>").is_err());
}

#[test]
fn notation_unicode_escapes() {
    let value = notation::from_str::<String>(r#""\u00E9\U0001F642""#).expect("decode BMP and supplementary escapes");

    assert_eq!(value, "é🙂");
    assert!(notation::from_str::<String>(r#""\U00110000""#).is_err(), "reject a codepoint above Unicode's maximum");
}

#[test]
fn value_accessors_match_variants() {
    let uuid = Uuid::parse_str("12345678-90ab-cdef-0123-456789abcdef").expect("valid UUID");
    let values = [
        Value::Boolean(true),
        Value::Integer(42),
        Value::Real(1.5),
        Value::Uuid(uuid),
        Value::String("plain text".into()),
        Value::Uri(Uri::from("https://example.invalid/resource")),
        Value::Array(vec![Value::Undefined]),
        Value::Map(BTreeMap::from([("key".into(), Value::Undefined)])),
    ];
    let binary = Binary::from(vec![0, 1, 255]);

    assert_eq!(values[0].as_bool(), Some(true));
    assert_eq!(values[1].as_integer(), Some(42));
    assert_eq!(values[2].as_real(), Some(1.5));
    assert_eq!(values[3].as_uuid(), Some(uuid));
    assert_eq!(values[4].as_str(), Some("plain text"));
    assert_eq!(values[5].as_str(), Some("https://example.invalid/resource"));
    assert_eq!(values[6].as_array(), Some(&[Value::Undefined][..]));
    assert_eq!(values[7].as_map().and_then(|map| map.get("key")), Some(&Value::Undefined));
    assert_eq!(binary.as_slice(), &[0, 1, 255]);

    let undefined = Value::Undefined;
    assert_eq!(undefined.as_bool(), None);
    assert_eq!(undefined.as_integer(), None);
    assert_eq!(undefined.as_real(), None);
    assert_eq!(undefined.as_uuid(), None);
    assert_eq!(undefined.as_str(), None);
    assert_eq!(undefined.as_array(), None);
    assert_eq!(undefined.as_map(), None);
    let text = Value::String("example".into());
    let uri = Value::Uri(Uri::from("https://example.com"));
    let date = Value::Date(Date::UNIX_EPOCH);
    let binary = Value::Binary(Binary::from(&b"abc"[..]));

    assert_eq!(text.as_str(), Some("example"));
    assert!(text.as_uri().is_none());
    assert_eq!(uri.as_str(), Some("https://example.com"));
    assert_eq!(uri.as_uri().map(Uri::as_str), Some("https://example.com"));
    assert_eq!(date.as_date(), Some(Date::UNIX_EPOCH));
    assert_eq!(binary.as_binary(), Some(&b"abc"[..]));
}

#[test]
fn streams_match_slice_codecs() {
    let value = Value::Array(vec![Value::Integer(42), Value::String("reader".into()), Value::Binary(Binary::from(vec![0, 255]))]);

    let xml_bytes = xml::to_vec(&value).expect("encode XML");
    let mut xml_writer = Vec::new();
    xml::to_writer(&mut xml_writer, &value).expect("write XML");
    assert_eq!(xml_writer, xml_bytes);
    assert_eq!(xml::from_reader::<Value, _>(Cursor::new(xml_bytes)).expect("read XML"), value);

    let notation_bytes = notation::to_vec(&value).expect("encode notation");
    let mut notation_writer = Vec::new();
    notation::to_writer(&mut notation_writer, &value).expect("write notation");
    assert_eq!(notation_writer, notation_bytes);
    assert_eq!(notation::from_reader::<Value, _>(Cursor::new(notation_bytes)).expect("read notation"), value);

    let binary_bytes = binary::to_vec(&value).expect("encode binary");
    let mut binary_writer = Vec::new();
    binary::to_writer(&mut binary_writer, &value).expect("write binary");
    assert_eq!(binary_writer, binary_bytes);
    assert_eq!(binary::from_reader::<Value, _>(Cursor::new(binary_bytes)).expect("read binary"), value);
}

#[test]
fn integer_conversions_enforce_range() {
    #[derive(Debug, Deserialize, PartialEq)]
    struct IntegerWidths {
        signed: i8,
        unsigned: u8,
    }

    let value = Value::Map(BTreeMap::from([("signed".into(), Value::Integer(-128)), ("unsigned".into(), Value::Integer(255))]));
    let expected = IntegerWidths { signed: -128, unsigned: 255 };
    let xml_bytes = xml::to_vec(&value).expect("encode in-range integer widths");
    let notation_bytes = notation::to_vec(&value).expect("encode in-range integer widths");
    let binary_bytes = binary::to_vec(&value).expect("encode in-range integer widths");
    assert_eq!(xml::from_slice::<IntegerWidths>(&xml_bytes).expect("decode XML integer widths"), expected);
    assert_eq!(notation::from_slice::<IntegerWidths>(&notation_bytes).expect("decode notation integer widths"), expected);
    assert_eq!(binary::from_slice::<IntegerWidths>(&binary_bytes).expect("decode binary integer widths"), expected);

    let out_of_range = [
        Value::Map(BTreeMap::from([("signed".into(), Value::Integer(128)), ("unsigned".into(), Value::Integer(0))])),
        Value::Map(BTreeMap::from([("signed".into(), Value::Integer(0)), ("unsigned".into(), Value::Integer(-1))])),
    ];
    for value in out_of_range {
        assert!(xml::from_slice::<IntegerWidths>(&xml::to_vec(&value).expect("encode out-of-range value")).is_err());
        assert!(notation::from_slice::<IntegerWidths>(&notation::to_vec(&value).expect("encode out-of-range value")).is_err());
        assert!(binary::from_slice::<IntegerWidths>(&binary::to_vec(&value).expect("encode out-of-range value")).is_err());
    }
}

#[test]
fn readers_stop_after_limit_probe() {
    const MAX_INPUT: usize = 4;
    let input = b"not a valid LLSD document";
    let limits = Limits { max_bytes: MAX_INPUT, ..Limits::default() };
    let expected_position = (MAX_INPUT + 1) as u64;

    let mut xml_reader = Cursor::new(input);
    assert_matches!(xml::from_reader_with_limits::<Value, _>(&mut xml_reader, limits), Err(Error::LimitExceeded("input bytes")));
    assert_eq!(xml_reader.position(), expected_position);

    let mut notation_reader = Cursor::new(input);
    assert_matches!(notation::from_reader_with_limits::<Value, _>(&mut notation_reader, limits), Err(Error::LimitExceeded("input bytes")));
    assert_eq!(notation_reader.position(), expected_position);

    let mut binary_reader = Cursor::new(input);
    assert_matches!(binary::from_reader_with_limits::<Value, _>(&mut binary_reader, limits), Err(Error::LimitExceeded("input bytes")));
    assert_eq!(binary_reader.position(), expected_position);
}

#[test]
fn binary_fixture_matches() {
    let mut map = Map::new();
    map.insert("foo".into(), Value::String("bar".into()));
    let encoded = binary::to_vec(&map).expect("encode binary fixture");
    assert_eq!(encoded, include_bytes!("fixtures/map_foo_bar.bin"));
}

#[test]
fn binary_dates_match_viewer_fixture() {
    let date_fixture = b"<?llsd/binary?>\nd\x00\x00\x00\x00\x00\x00\xf0?";
    let expected_date = Date::from_unix_seconds(1.0).expect("valid date");

    assert_eq!(binary::from_slice::<Date>(date_fixture).expect("decode typed date fixture"), expected_date);
    assert_eq!(binary::from_slice::<Value>(date_fixture).expect("decode dynamic date fixture"), Value::Date(expected_date));
    assert_eq!(binary::to_vec(&expected_date).expect("encode typed date fixture"), date_fixture);
    assert_eq!(binary::to_vec(&Value::Date(expected_date)).expect("encode dynamic date fixture"), date_fixture);

    let real_fixture = b"<?llsd/binary?>\nr\x3f\xf0\x00\x00\x00\x00\x00\x00";
    assert_eq!(binary::from_slice::<f64>(real_fixture).expect("decode network-order real"), 1.0);
    assert_eq!(binary::to_vec(&1.0_f64).expect("encode network-order real"), real_fixture);
}

fn without_final_line_ending(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\r\n").or_else(|| bytes.strip_suffix(b"\n")).unwrap_or(bytes)
}

#[test]
fn notation_fixture_matches() {
    let mut map = Map::new();
    map.insert("foo".into(), Value::String("bar".into()));
    let encoded = notation::to_vec(&map).expect("encode notation fixture");
    let value = encoded.strip_prefix(b"<?llsd/notation?>\n").expect("notation declaration");
    let fixture = include_bytes!("fixtures/map_foo_bar.notation");
    assert_eq!(value, without_final_line_ending(fixture));
}

#[test]
fn xml_fixture_matches() {
    let mut map = Map::new();
    map.insert("foo".into(), Value::String("bar".into()));
    let encoded = xml::to_vec(&map).expect("encode XML fixture");
    let fixture = include_bytes!("fixtures/map_foo_bar.xml");
    let declaration_end = fixture.windows(2).position(|part| part == b"?>").expect("fixture declaration") + 2;
    let encoded_body = encoded.windows(2).position(|part| part == b"?>").map(|index| &encoded[index + 2..]).expect("encoded declaration");
    let fixture_body = without_final_line_ending(&fixture[declaration_end..]);
    assert_eq!(encoded_body, fixture_body);
}

#[test]
fn xml_entity_references() {
    let value = xml::from_str::<Value>("<llsd><string>A&amp;&#60;&#x42;</string></llsd>").expect("decode XML entities");
    assert_eq!(value, Value::String("A&<B".into()));
    assert!(xml::from_str::<Value>("<llsd><string>&custom;</string></llsd>").is_err());
}

#[test]
fn xml_escapes_carriage_returns() {
    let value = Value::String("before\rafter".into());
    let encoded = xml::to_vec(&value).expect("encode carriage return");
    assert!(encoded.windows(b"before&#13;after".len()).any(|window| window == b"before&#13;after"));
    assert_eq!(xml::from_slice::<Value>(&encoded).expect("decode carriage return"), value);

    let map = BTreeMap::from([("key\rname".to_owned(), Value::Undefined)]);
    let encoded = xml::to_vec(&map).expect("encode carriage return in map key");
    assert!(encoded.windows(b"key&#13;name".len()).any(|window| window == b"key&#13;name"));
    assert_eq!(xml::from_slice::<Value>(&encoded).expect("decode carriage return map key"), Value::Map(map));
}

#[test]
fn xml_uris_preserve_character_references() {
    let values = [Value::Uri(Uri::new("before\r\nafter")), Value::Uri(Uri::new("literal &#13; &amp; <tag>"))];

    for value in values {
        let encoded = xml::to_vec(&value).expect("encode URI");
        let decoded = xml::from_slice::<Value>(&encoded).expect("decode URI");
        assert_eq!(decoded, value);
    }
}

#[test]
fn xml_combines_text_and_cdata() {
    let value = xml::from_str::<String>("<llsd><string>plain<!--split--><?split text?><![CDATA[<raw>&text]]>&amp;tail</string></llsd>")
        .expect("decode adjacent XML text events");

    assert_eq!(value, "plain<raw>&text&tail");
}

#[test]
fn xml_preserves_character_reference_line_endings() {
    let input = "<llsd><array><string>crlf\r\ncr\rlf</string><string>ref&#13;char</string><string><![CDATA[crlf\r\ncr\rlf]]></string></array></llsd>";
    let value = xml::from_str::<Value>(input).expect("decode XML line endings");
    assert!(xml::value_allocation_bound(input.as_bytes(), Limits::default()).is_ok());

    assert_eq!(
        value,
        Value::Array(vec![Value::String("crlf\ncr\nlf".into()), Value::String("ref\rchar".into()), Value::String("crlf\ncr\nlf".into()),])
    );
}

#[test]
fn xml_document_boundaries() {
    assert_eq!(xml::from_str::<Value>("\u{00a0}<llsd><undef/></llsd>\u{00a0}").expect("preserve whitespace tolerance"), Value::Undefined);
    for input in
        ["<llsd><undef/>", "<llsd><undef/><undef/></llsd>", "<llsd><undef/></llsd><llsd><undef/></llsd>", "<llsd><undef/></llsd>trailing"]
    {
        assert!(xml::from_str::<Value>(input).is_err(), "reject incomplete or extra data: {input}");
    }
    assert_eq!(xml::from_str::<[i32; 1]>("<llsd><array><integer>1</integer></array></llsd>").expect("read complete tuple"), [1]);
    assert!(xml::from_str::<[i32; 1]>("<llsd><array><integer>1</integer><integer>2</integer></array></llsd>").is_err());
}

#[test]
fn decode_limits_match_formats() {
    type Decode = fn(&[u8], Limits) -> llsd::Result<Value>;
    struct LimitCase {
        name: &'static str,
        inputs: [Vec<u8>; 3],
        over_limit_inputs: [Vec<u8>; 3],
        expected: Value,
        at_limit: [Limits; 3],
        over_limit: [Limits; 3],
    }

    fn limit_case(name: &'static str, inputs: [&'static [u8]; 3], expected: Value, at_limit: Limits, over_limit: Limits) -> LimitCase {
        let inputs = inputs.map(|input| input.to_vec());
        LimitCase { name, over_limit_inputs: inputs.clone(), inputs, expected, at_limit: [at_limit; 3], over_limit: [over_limit; 3] }
    }

    let decoders: [(&str, Decode); 3] =
        [("XML", xml::from_slice_with_limits), ("binary", binary::from_slice_with_limits), ("notation", notation::from_slice_with_limits)];

    let array = Value::Array(vec![Value::Undefined]);
    let array_inputs: [&[u8]; 3] = [b"<llsd><array><undef/></array></llsd>", b"[\0\0\0\x01!]", b"[!]"];
    let string = Value::String("ab".into());
    let string_inputs: [&[u8]; 3] = [b"<llsd><string>ab</string></llsd>", b"s\0\0\0\x02ab", b"'ab'"];
    let map = Value::Map(BTreeMap::from([("key".into(), Value::Undefined)]));
    let map_inputs: [&[u8]; 3] = [b"<llsd><map><key>key</key><undef/></map></llsd>", b"{\0\0\0\x01k\0\0\0\x03key!}", b"{'key':!}"];
    let binary_value = Value::Binary(Binary::from(vec![0, 255]));
    let binary_inputs: [&[u8]; 3] = [b"<llsd><binary>AP8=</binary></llsd>", b"b\0\0\0\x02\0\xff", b"b(2)\"\0\xff\""];

    let mut cases = vec![
        limit_case(
            "value count",
            array_inputs,
            array.clone(),
            Limits { max_values: 2, ..Limits::default() },
            Limits { max_values: 1, ..Limits::default() },
        ),
        limit_case(
            "container items",
            array_inputs,
            array.clone(),
            Limits { max_container_items: 1, ..Limits::default() },
            Limits { max_container_items: 0, ..Limits::default() },
        ),
        limit_case(
            "string bytes",
            string_inputs,
            string,
            Limits { max_string_bytes: 2, ..Limits::default() },
            Limits { max_string_bytes: 1, ..Limits::default() },
        ),
        limit_case(
            "map key bytes",
            map_inputs,
            map,
            Limits { max_string_bytes: 1, max_map_key_bytes: 3, ..Limits::default() },
            Limits { max_string_bytes: 1, max_map_key_bytes: 2, ..Limits::default() },
        ),
        limit_case(
            "binary bytes",
            binary_inputs,
            binary_value,
            Limits { max_binary_bytes: 2, ..Limits::default() },
            Limits { max_binary_bytes: 1, ..Limits::default() },
        ),
    ];

    let input_bytes = array_inputs.map(|input| input.to_vec());
    let input_bytes_at_limit = std::array::from_fn(|index| Limits { max_bytes: input_bytes[index].len(), ..Limits::default() });
    let input_bytes_over_limit = std::array::from_fn(|index| Limits { max_bytes: input_bytes[index].len() - 1, ..Limits::default() });
    cases.push(LimitCase {
        name: "input bytes",
        over_limit_inputs: input_bytes.clone(),
        inputs: input_bytes,
        expected: array,
        at_limit: input_bytes_at_limit,
        over_limit: input_bytes_over_limit,
    });

    fn nested_inputs(depth: usize) -> [Vec<u8>; 3] {
        let xml = format!("<llsd>{}<undef/>{}</llsd>", "<array>".repeat(depth), "</array>".repeat(depth));
        let mut binary = Vec::new();
        for _ in 0..depth {
            binary.extend_from_slice(b"[\0\0\0\x01");
        }
        binary.push(b'!');
        binary.extend(std::iter::repeat_n(b']', depth));
        let notation = format!("{}!{}", "[".repeat(depth), "]".repeat(depth));
        [xml.into_bytes(), binary, notation.into_bytes()]
    }

    for max_depth in [0, 1, Limits::default().max_depth] {
        let at_limit = Limits { max_depth, ..Limits::default() };
        let mut expected = Value::Undefined;
        for _ in 0..max_depth {
            expected = Value::Array(vec![expected]);
        }
        cases.push(LimitCase {
            name: "nesting depth",
            inputs: nested_inputs(max_depth),
            over_limit_inputs: nested_inputs(max_depth + 1),
            expected,
            at_limit: [at_limit; 3],
            over_limit: [at_limit; 3],
        });
    }

    for case in cases {
        for (index, (format, decode)) in decoders.iter().enumerate() {
            assert_matches!(
                decode(&case.inputs[index], case.at_limit[index]),
                Ok(actual) if actual == case.expected,
                "{format}: {} must accept the exact boundary",
                case.name
            );
            assert_matches!(
                decode(&case.over_limit_inputs[index], case.over_limit[index]),
                Err(Error::LimitExceeded(actual)) if actual == case.name,
                "{format}: {} must fail over its limit",
                case.name
            );
        }
    }
}

#[test]
fn uri_binary_fixture() {
    let wire = include_str!("fixtures/uri_binary.hex")
        .split_ascii_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).expect("fixture byte"))
        .collect::<Vec<_>>();
    let value = Value::Uri(Uri::from("https://example.invalid/path"));

    assert_eq!(binary::from_slice::<Value>(&wire).expect("decode spec URI"), value);
    assert_eq!(binary::to_vec(&value).expect("encode spec URI"), wire);
}

#[test]
fn notation_uri_preserves_control_bytes() {
    const NOTATION_DECLARATION: &[u8] = b"<?llsd/notation?>\n";
    let uri = Uri::from("path/\u{0001}segment");

    let encoded = notation::to_vec(&uri).expect("encode URI with a control byte");
    let body = encoded.strip_prefix(NOTATION_DECLARATION).expect("notation declaration");
    assert_eq!(body, b"l\"path/\\x01segment\"");
    assert_eq!(notation::from_slice::<Uri>(&encoded).expect("decode URI with a control byte"), uri);
}

#[test]
fn viewer_fixtures() {
    const XML_DECLARATION: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>";
    const NOTATION_DECLARATION: &[u8] = b"<?llsd/notation?>\n";

    let uri_xml = include_bytes!("fixtures/viewer_uri.xml");
    let uri = Value::Uri(Uri::from("https://secondlife.com/login"));
    assert_eq!(xml::from_slice::<Value>(uri_xml).expect("decode viewer URI XML"), uri);
    let encoded = xml::to_vec(&uri).expect("encode viewer URI XML");
    let uri_xml = without_final_line_ending(uri_xml);
    assert_eq!(encoded.strip_prefix(XML_DECLARATION).expect("XML declaration"), uri_xml);

    let uri_notation = include_bytes!("fixtures/viewer_uri.notation");
    let uri = Value::Uri(Uri::from("http://www.google.com"));
    let uri_notation = without_final_line_ending(uri_notation);
    assert_eq!(notation::from_slice::<Value>(uri_notation).expect("decode viewer URI notation"), uri);
    let encoded = notation::to_vec(&uri).expect("encode viewer URI notation");
    assert_eq!(encoded.strip_prefix(NOTATION_DECLARATION).expect("notation declaration"), uri_notation);

    let date_xml = include_bytes!("fixtures/viewer_date.xml");
    let date = llsd_date("2006-04-24T16:11:33Z");
    let decoded = xml::from_slice::<Value>(date_xml).expect("decode viewer date XML");
    assert_matches!(decoded, Value::Date(value) if (value.unix_seconds() - date.unix_seconds()).abs() < 0.000_001);
    let date = Value::Date(date);
    let encoded = xml::to_vec(&date).expect("encode viewer date XML");
    let date_xml = without_final_line_ending(date_xml);
    assert_eq!(encoded.strip_prefix(XML_DECLARATION).expect("XML declaration"), date_xml);

    let date_notation = include_bytes!("fixtures/viewer_date.notation");
    let date_notation = without_final_line_ending(date_notation);
    let expected = llsd_date("2007-12-28T09:22:53.10Z");
    let decoded = notation::from_slice::<Value>(date_notation).expect("decode viewer date notation");
    assert_matches!(decoded, Value::Date(value) if (value.unix_seconds() - expected.unix_seconds()).abs() < 0.000_001);
}

#[test]
fn typed_struct_roundtrips() {
    let expected = profile();
    let xml = xml::to_vec(&expected).expect("encode XML");
    let notation = notation::to_vec(&expected).expect("encode notation");
    let binary = binary::to_vec(&expected).expect("encode binary");

    assert_eq!(xml::from_slice::<Profile>(&xml).expect("decode XML"), expected);
    assert_eq!(notation::from_slice::<Profile>(&notation).expect("decode notation"), expected);
    assert_eq!(binary::from_slice::<Profile>(&binary).expect("decode binary"), expected);
}

#[test]
fn ordinary_newtypes_roundtrip_across_formats() {
    let counter = Counter(41);
    let wrapped_profile = WrappedProfile(profile());

    let xml_counter = xml::to_vec(&counter).expect("encode XML counter");
    let xml_profile = xml::to_vec(&wrapped_profile).expect("encode XML wrapped profile");
    assert_eq!(xml_counter, xml::to_vec(&counter.0).expect("encode inner XML counter"));
    assert_eq!(xml_profile, xml::to_vec(&wrapped_profile.0).expect("encode inner XML profile"));
    assert_eq!(xml::from_slice::<Counter>(&xml_counter).expect("decode XML counter"), counter);
    assert_eq!(xml::from_slice::<WrappedProfile>(&xml_profile).expect("decode XML wrapped profile"), wrapped_profile);

    let notation_counter = notation::to_vec(&counter).expect("encode notation counter");
    let notation_profile = notation::to_vec(&wrapped_profile).expect("encode notation wrapped profile");
    assert_eq!(notation_counter, notation::to_vec(&counter.0).expect("encode inner notation counter"));
    assert_eq!(notation_profile, notation::to_vec(&wrapped_profile.0).expect("encode inner notation profile"));
    assert_eq!(notation::from_slice::<Counter>(&notation_counter).expect("decode notation counter"), counter);
    assert_eq!(notation::from_slice::<WrappedProfile>(&notation_profile).expect("decode notation wrapped profile"), wrapped_profile);

    let binary_counter = binary::to_vec(&counter).expect("encode binary counter");
    let binary_profile = binary::to_vec(&wrapped_profile).expect("encode binary wrapped profile");
    assert_eq!(binary_counter, binary::to_vec(&counter.0).expect("encode inner binary counter"));
    assert_eq!(binary_profile, binary::to_vec(&wrapped_profile.0).expect("encode inner binary profile"));
    assert_eq!(binary::from_slice::<Counter>(&binary_counter).expect("decode binary counter"), counter);
    assert_eq!(binary::from_slice::<WrappedProfile>(&binary_profile).expect("decode binary wrapped profile"), wrapped_profile);
}

#[test]
fn enum_variants_roundtrip_across_formats() {
    let expected =
        [WireEnum::Unit, WireEnum::Newtype(41), WireEnum::Tuple(7, "tuple".into()), WireEnum::Struct { label: "record".into(), count: 3 }];
    for value in expected {
        let xml = xml::to_vec(&value).expect("encode XML enum");
        let notation = notation::to_vec(&value).expect("encode notation enum");
        let binary = binary::to_vec(&value).expect("encode binary enum");
        assert_eq!(xml::from_slice::<WireEnum>(&xml).expect("decode XML enum"), value);
        assert_eq!(notation::from_slice::<WireEnum>(&notation).expect("decode notation enum"), value);
        assert_eq!(binary::from_slice::<WireEnum>(&binary).expect("decode binary enum"), value);
    }

    let malformed = Value::Map(BTreeMap::from([("Newtype".into(), Value::Integer(41)), ("Other".into(), Value::Integer(42))]));
    let xml = xml::to_vec(&malformed).expect("encode malformed XML enum map");
    let notation = notation::to_vec(&malformed).expect("encode malformed notation enum map");
    let binary = binary::to_vec(&malformed).expect("encode malformed binary enum map");
    assert!(xml::from_slice::<WireEnum>(&xml).is_err());
    assert!(notation::from_slice::<WireEnum>(&notation).is_err());
    assert!(binary::from_slice::<WireEnum>(&binary).is_err());

    assert_matches!(
        notation::from_slice_with_limits::<WireEnum>(br#"{'Newtype':i41}"#, Limits { max_container_items: 0, ..Limits::default() }),
        Err(Error::LimitExceeded("container items"))
    );
}

#[test]
fn map_keys_follow_format_readability() {
    let value = BTreeMap::from([(FormatAwareKey, 1)]);

    let xml = xml::to_vec(&value).expect("encode readable XML key");
    assert_eq!(xml::from_slice::<Map>(&xml).expect("decode readable XML key").get("key"), Some(&Value::Integer(1)));

    let notation = notation::to_vec(&value).expect("encode readable notation key");
    assert_eq!(notation::from_slice::<Map>(&notation).expect("decode readable notation key").get("key"), Some(&Value::Integer(1)));

    assert_matches!(binary::to_vec(&value), Err(Error::InvalidValue(message)) if message == "map keys must be strings");
}

#[test]
fn date_forms() {
    let expected = Value::Date(Date::from_unix_seconds(1_138_752_000.0).expect("date-only midnight UTC"));
    assert_eq!(xml::from_str::<Value>("<llsd><date>2006-02-01</date></llsd>").expect("decode XML date"), expected);
    assert_eq!(notation::from_str::<Value>("d\"2006-02-01\"").expect("decode notation date"), expected);
    assert_eq!(notation::from_str::<f64>("d\"1970-01-01T00:00:01Z\"").expect("decode notation date as timestamp"), 1.0);

    let offset = Date::from_datetime(OffsetDateTime::parse("2025-02-03T05:05:06+01:00", &Rfc3339).expect("valid offset date"));
    let value = Value::Date(offset);
    assert!(xml::to_string(&value).expect("encode XML date").contains("2025-02-03T04:05:06Z"));
    assert!(notation::to_string(&value).expect("encode notation date").contains("2025-02-03T04:05:06Z"));
}

#[test]
fn invalid_timestamps_fail() {
    for seconds in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, f64::MIN] {
        assert_eq!(Date::from_unix_seconds(seconds), None, "accepted timestamp {seconds}");
    }
}

#[test]
fn text_date_codecs_preserve_nanoseconds() {
    let expected = llsd_date("2025-02-03T04:05:06.123456789Z");
    let xml = xml::to_vec(&expected).expect("encode exact XML date");
    let notation = notation::to_vec(&expected).expect("encode exact notation date");

    assert_eq!(xml::from_slice::<Date>(&xml).expect("decode exact XML date"), expected);
    assert_eq!(notation::from_slice::<Date>(&notation).expect("decode exact notation date"), expected);

    let offset = b"<llsd><date>2025-02-03T05:05:06.123456789+01:00</date></llsd>";
    assert_eq!(xml::from_slice::<Date>(offset).expect("decode offset XML date"), expected);
}

#[test]
fn typed_uuid_roundtrips() {
    let expected = UuidField { id: Uuid::parse_str("12345678-90ab-cdef-0123-456789abcdef").expect("valid UUID") };
    let xml = xml::to_vec(&expected).expect("encode XML");
    let notation = notation::to_vec(&expected).expect("encode notation");
    let binary = binary::to_vec(&expected).expect("encode binary");

    assert!(xml.windows(b"<uuid>".len()).any(|part| part == b"<uuid>"));
    assert!(notation.windows(b"u12345678-90ab-cdef-0123-456789abcdef".len()).any(|part| part == b"u12345678-90ab-cdef-0123-456789abcdef"));
    let mut binary_uuid = vec![b'u'];
    binary_uuid.extend_from_slice(expected.id.as_bytes());
    assert!(binary.windows(binary_uuid.len()).any(|part| part == binary_uuid.as_slice()));
    assert_eq!(xml::from_slice::<UuidField>(&xml).expect("decode XML"), expected);
    assert_eq!(notation::from_slice::<UuidField>(&notation).expect("decode notation"), expected);
    assert_eq!(binary::from_slice::<UuidField>(&binary).expect("decode binary"), expected);
}

#[test]
fn invalid_typed_scalars_are_rejected() {
    let invalid_uuid_xml = b"<llsd><map><key>id</key><uuid>not-a-uuid</uuid></map></llsd>";
    let invalid_uuid_notation = br#"{'id':unot-a-uuid}"#;
    assert_matches!(xml::from_slice::<UuidField>(invalid_uuid_xml), Err(Error::InvalidValue(_)));
    assert_matches!(notation::from_slice::<UuidField>(invalid_uuid_notation), Err(Error::InvalidValue(_)));

    assert_matches!(xml::from_str::<Date>("<llsd><date>2025-02-30</date></llsd>"), Err(Error::InvalidValue(_)));
    assert_matches!(notation::from_str::<Date>("d\"2025-02-30\""), Err(Error::InvalidValue(_)));
}

#[test]
fn malformed_inputs_fail() {
    let binary = include_bytes!("fixtures/map_foo_bar.bin");
    assert!(binary::from_slice::<Value>(&binary[..binary.len() - 1]).is_err());
    assert!(notation::from_str::<Value>("i2147483648").is_err());
    assert!(notation::from_str::<Value>("['unfinished'").is_err());
    assert!(xml::from_str::<Value>("<llsd><array><integer>1</llsd>").is_err());
}

#[test]
fn preambles_accept_optional_line_endings() {
    let expected = Value::Undefined;
    let binary_headers =
        [&b"<?llsd/binary?>!"[..], &b"<?llsd/binary?>\r\n!"[..], &b"<? LLSD/Binary ?>\n!"[..], &b"<?\tLlSd/BiNaRy \t?>\r\n!"[..]];
    let notation_headers = [
        &b"<?llsd/notation?>!"[..],
        &b"<?llsd/notation?> \t\r\n!"[..],
        &b"<? llsd/notation ?>\n!"[..],
        &b"<?\tLLSD/NoTaTiOn \t?>\r\n!"[..],
    ];

    assert_eq!(binary::from_slice::<Value>(b"!").expect("headerless binary"), expected);
    for input in binary_headers {
        assert_eq!(binary::from_slice::<Value>(input).expect("binary preamble"), expected);
    }
    assert_eq!(notation::from_slice::<Value>(b"!").expect("headerless notation"), expected);
    for input in notation_headers {
        assert_eq!(notation::from_slice::<Value>(input).expect("notation preamble"), expected);
    }

    assert!(binary::from_slice::<Value>(b" !").is_err(), "binary leading whitespace stays invalid without a preamble");
    assert_eq!(notation::from_slice::<Value>(b" \t\r\n! ").expect("notation surrounding whitespace"), expected);

    for input in [&b"<? LLSD/Array ?>\n!"[..], &b"<? LLSD/Binary ? !"[..], &b"<? LLSD/Binary !"[..], &b"<? LLSD/Notation ?>\n!"[..]] {
        assert!(binary::from_slice::<Value>(input).is_err(), "malformed or wrong-format binary header must fail");
    }
    for input in [&b"<? LLSD/Array ?>\n!"[..], &b"<? llsd/notation ? !"[..], &b"<? llsd/notation !"[..], &b"<? LLSD/Binary ?>\n!"[..]] {
        assert!(notation::from_slice::<Value>(input).is_err(), "malformed or wrong-format notation header must fail");
    }

    assert_matches!(binary::from_slice::<Value>(b"<? LLSD/Binary ?>\n!!"), Err(Error::TrailingData));
    assert_matches!(notation::from_slice::<Value>(b"<? llsd/notation ?>\r\n!!"), Err(Error::TrailingData));

    let binary_document = b"<? LLSD/Binary ?>\n!";
    assert_matches!(
        binary::from_slice_with_limits::<Value>(binary_document, Limits { max_bytes: binary_document.len() - 1, ..Limits::default() }),
        Err(Error::LimitExceeded("input bytes"))
    );
    let notation_document = b"<? llsd/notation ?>\r\n!";
    assert_matches!(
        notation::from_slice_with_limits::<Value>(
            notation_document,
            Limits { max_bytes: notation_document.len() - 1, ..Limits::default() }
        ),
        Err(Error::LimitExceeded("input bytes"))
    );
}

#[test]
fn xml_tokens_count_coalesced_text() {
    let map = (0..24).map(|index| (format!("key_{index:02}"), Value::String(format!("value_{index:02}")))).collect::<Map>();
    let expected = Value::Map(map);
    let xml = xml::to_vec(&expected).expect("encode map-heavy XML");
    let limits = Limits { max_values: 25, max_container_items: 24, ..Limits::default() };
    assert_eq!(xml::from_slice_with_limits::<Value>(&xml, limits).expect("decode within value and item limits"), expected);

    let entities = "&amp;".repeat(64);
    let xml = format!("<llsd><string>{entities}</string></llsd>");
    let limits = Limits { max_values: 2, max_string_bytes: 64, ..Limits::default() };
    assert_eq!(xml::from_slice_with_limits::<String>(xml.as_bytes(), limits).expect("decode entity-heavy text"), "&".repeat(64));
    assert_matches!(
        xml::from_slice_with_limits::<String>(xml.as_bytes(), Limits { max_string_bytes: 63, ..limits }),
        Err(Error::LimitExceeded("string bytes"))
    );

    let over_limit = b"<llsd><array><undef/><undef/></array></llsd>";
    assert_matches!(
        xml::from_slice_with_limits::<Value>(over_limit, Limits { max_values: 2, ..limits }),
        Err(Error::LimitExceeded("value count"))
    );
}

#[test]
fn base64_binary_limit() {
    let limits = Limits { max_string_bytes: 1, max_binary_bytes: 1, ..Limits::default() };
    let expected = Value::Binary(Binary::from(vec![0]));

    assert_eq!(
        xml::from_slice_with_limits::<Value>(b"<llsd><binary encoding=\"base64\">AA==</binary></llsd>", limits)
            .expect("one-byte XML binary fits"),
        expected
    );
    assert_eq!(notation::from_slice_with_limits::<Value>(b"b64\"AA==\"", limits).expect("one-byte notation binary fits"), expected);
    assert_matches!(
        xml::from_slice_with_limits::<Value>(b"<llsd><binary>AAA=</binary></llsd>", limits),
        Err(Error::LimitExceeded("binary bytes"))
    );
    assert_matches!(notation::from_slice_with_limits::<Value>(b"b64\"AAA=\"", limits), Err(Error::LimitExceeded("binary bytes")));
    assert_matches!(
        xml::from_slice_with_limits::<Value>(b"<llsd><string>ab</string></llsd>", limits),
        Err(Error::LimitExceeded("string bytes"))
    );
    assert_matches!(notation::from_slice_with_limits::<Value>(b"'ab'", limits), Err(Error::LimitExceeded("string bytes")));

    let two_byte_limits = Limits { max_string_bytes: 1, max_binary_bytes: 2, ..Limits::default() };
    let expected = Value::Binary(Binary::from(vec![0, 255]));
    assert_eq!(
        xml::from_slice_with_limits::<Value>(b"<llsd><binary> A P 8= </binary></llsd>", two_byte_limits)
            .expect("XML binary whitespace does not count as string bytes"),
        expected
    );
    assert_matches!(
        xml::from_slice_with_limits::<Value>(b"<llsd><string>ab</string></llsd>", two_byte_limits),
        Err(Error::LimitExceeded("string bytes"))
    );
}

#[test]
fn binary_length_limit() {
    let Ok(length) = usize::try_from(u64::from(u32::MAX) + 1) else {
        return;
    };
    assert_matches!(binary::to_vec(&OversizedSequence(length)), Err(Error::LimitExceeded("encoded length")));
}

#[test]
fn xml_binary_encoding() {
    let expected = Value::Binary(Binary::from(vec![0, 1, 0xff]));

    for input in [&b"<llsd><binary>AAH/</binary></llsd>"[..], &b"<llsd><binary encoding=\"base64\">AAH/</binary></llsd>"[..]] {
        assert_eq!(xml::from_slice::<Value>(input).expect("decode base64 binary"), expected);
    }

    for input in [&b"<llsd><binary encoding=\"base16\">AAH/</binary></llsd>"[..], &b"<llsd><binary encoding=\"base16\"/></llsd>"[..]] {
        assert_matches!(xml::from_slice::<Value>(input), Err(Error::InvalidValue(_)));
    }
}

#[test]
fn xml_bounded_sizes() {
    let values = [
        Value::String("a string that exceeds a small output budget".into()),
        Value::Binary(Binary::from(vec![0; 64])),
        Value::Array(vec![Value::String("first".into()), Value::String("second".into())]),
        Value::Uri(Uri::from("https://example.invalid/path")),
        Value::Uuid(Uuid::parse_str("12345678-90ab-cdef-0123-456789abcdef").expect("valid UUID")),
        Value::Date(llsd_date("2025-02-03T04:05:06.125Z")),
    ];

    for value in values {
        let encoded = xml::to_vec(&value).expect("encode XML");
        assert_eq!(xml::to_vec_with_max_bytes(&value, encoded.len()).expect("exact XML limit"), encoded);
        assert_matches!(xml::to_vec_with_max_bytes(&value, encoded.len() - 1), Err(Error::LimitExceeded("encoded length")));
    }

    assert_matches!(xml::to_vec_with_max_bytes(&Value::String("x".repeat(1024)), 128), Err(Error::LimitExceeded("encoded length")));
    assert_matches!(
        xml::to_vec_with_max_bytes(&Value::Binary(Binary::from(vec![0; 1024])), 128),
        Err(Error::LimitExceeded("encoded length"))
    );
}

#[test]
fn xml_bounded_escape() {
    let mut map = BTreeMap::new();
    map.insert("<&>\"'".into(), Value::String("<&>\"'".into()));
    let value = Value::Map(map);
    let encoded = xml::to_vec(&value).expect("encode escaped XML");

    assert_eq!(xml::to_vec_with_max_bytes(&value, encoded.len()).expect("exact escaped limit"), encoded);
    assert_matches!(xml::to_vec_with_max_bytes(&value, encoded.len() - 1), Err(Error::LimitExceeded("encoded length")));
}

#[test]
fn xml_rejects_invalid_characters() {
    let value = Value::String("invalid\u{1}xml".into());
    assert_matches!(xml::to_vec(&value), Err(Error::InvalidValue(_)));
    assert_matches!(xml::to_vec_with_max_bytes(&value, 128), Err(Error::InvalidValue(_)));
    assert_matches!(xml::to_vec_with_max_bytes(&RepeatedText { character: '\u{1}', count: 1 }, 128), Err(Error::InvalidValue(_)));

    let mut map = Map::new();
    map.insert("invalid\u{1}key".into(), Value::Undefined);
    assert_matches!(xml::to_vec(&map), Err(Error::InvalidValue(_)));
    assert_matches!(xml::to_vec_with_max_bytes(&map, 128), Err(Error::InvalidValue(_)));
}

#[test]
fn xml_bounded_serde_shapes() {
    let mut map = BTreeMap::new();
    map.insert("<&key>".to_owned(), vec![1i32, 2, 3]);
    let display_key_map = BTreeMap::from([(RepeatedText { character: '&', count: 8 }, 5i32)]);

    assert_xml_limit(&profile());
    assert_xml_limit(&map);
    assert_xml_limit(&display_key_map);
    assert_xml_limit(&(17i32, "tuple"));
    assert_xml_limit(&BoundedTupleStruct(17, "tuple struct".into()));
    assert_xml_limit(&Some(19i32));
    assert_xml_limit(&Option::<i32>::None);
    assert_xml_limit(&BoundedEnum::Unit);
    assert_xml_limit(&BoundedEnum::Newtype(Value::String("newtype".into())));
    assert_xml_limit(&BoundedEnum::Tuple(17, "tuple & text".into()));
    assert_xml_limit(&BoundedEnum::Struct {
        label: "record".into(),
        value: Value::Array(vec![Value::Integer(9), Value::String("<&>".into())]),
    });
}

#[test]
fn xml_bounded_collect_str() {
    assert_xml_limit(&RepeatedText { character: '&', count: 8 });
    assert_matches!(
        xml::to_vec_with_max_bytes(&RepeatedText { character: '&', count: 1024 }, 256),
        Err(Error::LimitExceeded("encoded length"))
    );

    let oversized_key_map = BTreeMap::from([(RepeatedText { character: '&', count: 1024 }, 5i32)]);
    assert_matches!(xml::to_vec_with_max_bytes(&oversized_key_map, 256), Err(Error::LimitExceeded("encoded length")));
}

#[test]
fn xml_hint_ignored() {
    let encoded = xml::to_vec_with_max_bytes(&OversizedSequence(usize::MAX), 128).expect("bounded empty sequence");
    assert!(encoded.windows(b"<array></array>".len()).any(|part| part == b"<array></array>"));
}

#[test]
fn xml_bounded_nesting() {
    let max_depth = Limits::default().max_depth;
    assert!(xml::to_vec_with_max_bytes(&NestedSequence(max_depth - 1), 4096).is_ok());
    assert_matches!(xml::to_vec_with_max_bytes(&NestedSequence(max_depth), 4096), Err(Error::LimitExceeded("nesting depth")));
}

#[test]
fn xml_limits_count_normalized_text() {
    for (input, expected, bytes, limit_name) in [
        (b"<llsd><string>a\r\nb</string></llsd>".as_slice(), Value::String("a\nb".into()), 3, "string bytes"),
        (
            b"<llsd><map><key>a\r\nb</key><undef/></map></llsd>".as_slice(),
            Value::Map(BTreeMap::from([("a\nb".into(), Value::Undefined)])),
            3,
            "map key bytes",
        ),
        (b"<llsd><string>&#13;&#10;</string></llsd>".as_slice(), Value::String("\r\n".into()), 2, "string bytes"),
        (b"<llsd><string>a&#13;\nb</string></llsd>".as_slice(), Value::String("a\r\nb".into()), 4, "string bytes"),
    ] {
        let limits = Limits { max_string_bytes: bytes, max_map_key_bytes: bytes, ..Limits::default() };
        assert_eq!(xml::from_slice_with_limits::<Value>(input, limits).expect("normalized text fits"), expected);
        assert!(xml::value_allocation_bound(input, limits).is_ok(), "scanner rejected accepted normalized text");
        let limits = Limits { max_string_bytes: bytes - 1, max_map_key_bytes: bytes - 1, ..limits };
        assert_matches!(xml::from_slice_with_limits::<Value>(input, limits), Err(Error::LimitExceeded(name)) if name == limit_name);
    }
}
