use std::collections::BTreeMap;

use protocol::login::{self, InventoryBootstrapError, Response, ResponseData};
use uuid::Uuid;

fn response_data(input: &str) -> ResponseData {
    let Response::Authenticated(response) = Response::from_xml(input, 64).expect("valid login fixture") else {
        panic!("authenticated response expected")
    };
    response.data
}

#[test]
fn parses_personal_and_library_skeletons() {
    let data = response_data(include_str!("fixtures/inventory_login.xml"));

    let bootstrap = data.inventory_bootstrap().expect("inventory fields parse");

    assert_eq!(bootstrap.personal_root, Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").ok());
    assert_eq!(bootstrap.library_root, Uuid::parse_str("eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee").ok());
    assert_eq!(bootstrap.library_owner, Uuid::parse_str("ffffffff-ffff-ffff-ffff-ffffffffffff").ok());
    assert_eq!(bootstrap.personal_skeleton.folders.len(), 3);
    assert_eq!(bootstrap.personal_skeleton.folders.get(1).map(|folder| folder.name.as_str()), Some("Favorites"));
    assert_eq!(bootstrap.personal_skeleton.folders.get(1).and_then(|folder| folder.type_default), Some(23));
    assert_eq!(bootstrap.personal_skeleton.folders.get(2).and_then(|folder| folder.type_default), None);
    assert_eq!(bootstrap.personal_skeleton.folders.first().map(|folder| folder.version), Some(-1));
    assert_eq!(bootstrap.library_skeleton.folders.len(), 2);
    assert_eq!(bootstrap.library_skeleton.folders.get(1).map(|folder| folder.name.as_str()), Some("Textures"));
}

#[test]
fn malformed_entries_are_counted_and_skipped() {
    let data = response_data(include_str!("fixtures/inventory_login_malformed.xml"));

    let bootstrap = data.inventory_bootstrap().expect("skeleton entries are independently rejected");

    assert_eq!(bootstrap.personal_skeleton.folders.len(), 1);
    assert_eq!(bootstrap.personal_skeleton.rejected_entries, 2);
    assert!(bootstrap.library_skeleton.folders.is_empty());
}

#[test]
fn oversized_skeleton_is_rejected() {
    let entries = vec![login::Value::String("invalid".into()); login::MAX_LOGIN_INVENTORY_FOLDERS.saturating_add(1)];
    let data = ResponseData::from_fields(BTreeMap::from([("inventory-skeleton".into(), login::Value::Array(entries))]));

    assert_eq!(
        data.inventory_bootstrap().expect_err("folder count exceeds the admission limit"),
        InventoryBootstrapError::TooManyPersonalFolders
    );
}

#[test]
fn oversized_name_entry_is_skipped() {
    let fields = BTreeMap::from([
        ("folder_id".into(), login::Value::String("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".into())),
        ("parent_id".into(), login::Value::String("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".into())),
        ("name".into(), login::Value::String("x".repeat(login::MAX_INVENTORY_FOLDER_NAME_BYTES.saturating_add(1)))),
        ("version".into(), login::Value::Integer(1)),
    ]);
    let data =
        ResponseData::from_fields(BTreeMap::from([("inventory-skeleton".into(), login::Value::Array(vec![login::Value::Struct(fields)]))]));

    let bootstrap = data.inventory_bootstrap().expect("one malformed folder is not a malformed skeleton");

    assert!(bootstrap.personal_skeleton.folders.is_empty());
    assert_eq!(bootstrap.personal_skeleton.rejected_entries, 1);
}
