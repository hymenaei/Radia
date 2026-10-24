use llsd::{Binary, Uri};

use super::*;

#[test]
fn counts_owned_buffer_capacity() {
    let mut uri_text = String::with_capacity(4096);
    uri_text.push_str("uri");
    let uri_capacity = uri_text.capacity();
    let mut uri = Value::Uri(Uri::from(uri_text));
    assert_eq!(llsd_heap_size(&mut uri), uri_capacity);
    std::assert_matches!(&uri, Value::Uri(value) if value.as_str() == "uri");

    let mut bytes = Vec::with_capacity(4096);
    bytes.extend_from_slice(b"bin");
    let binary_capacity = bytes.capacity();
    let mut binary = Value::Binary(Binary::from(bytes));
    assert_eq!(llsd_heap_size(&mut binary), binary_capacity);
    std::assert_matches!(&binary, Value::Binary(value) if value.as_slice() == b"bin");
}
