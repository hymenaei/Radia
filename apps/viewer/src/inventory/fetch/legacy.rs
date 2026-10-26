use ::inventory::{CategoryKey, InventoryChild, InventoryOwner, Item};
use llsd::{Map, Value};
use network::capabilities::{CapabilityUrl, Client};
use protocol::inventory::{self, DecodeError, RequestedFolder};
use uuid::Uuid;

use super::{
    FolderFetch, InventoryFetchError, MAX_FALLBACK_FOLDERS, MAX_FETCH_RESPONSE_BYTES, RequestFailure, classify_request_error,
    decode::{to_category, to_item},
};

pub(super) async fn fetch_batch(
    client: &Client,
    capability: &CapabilityUrl,
    folders: &[CategoryKey],
) -> Result<Vec<FolderFetch>, RequestFailure> {
    if folders.is_empty() || folders.len() > MAX_FALLBACK_FOLDERS {
        return Err(RequestFailure::Failed);
    }
    let requested =
        folders.iter().map(|folder| RequestedFolder { id: folder.id().uuid(), owner_id: folder.owner().id() }).collect::<Vec<_>>();
    let response = client
        .post_bytes_with_response_limit(capability, &folder_request(folders), MAX_FETCH_RESPONSE_BYTES)
        .await
        .map_err(classify_request_error)?;
    inventory::decode_fetch_inventory_descendents2(&response, &requested)
        .map_err(classify_decode_error)?
        .into_iter()
        .zip(folders.iter().copied())
        .map(|(folder, key)| to_folder_fetch(folder, key).map_err(|_| RequestFailure::InvalidResponse))
        .collect()
}

pub(crate) async fn fetch_items(
    client: &Client,
    capability: &CapabilityUrl,
    owner: InventoryOwner,
    item_ids: &[Uuid],
) -> Result<Vec<Item>, InventoryFetchError> {
    let mut items = Vec::with_capacity(item_ids.len());
    for batch in item_ids.chunks(MAX_FALLBACK_FOLDERS) {
        let response = client
            .post_bytes_with_response_limit(capability, &item_request(batch), MAX_FETCH_RESPONSE_BYTES)
            .await
            .map_err(map_request_error)?;
        let decoded = inventory::decode_fetch_inventory2(&response, owner.id(), batch).map_err(map_decode_error)?;
        for item in decoded {
            let parent = item.parent_id.ok_or(InventoryFetchError::InvalidResponse)?;
            items.push(to_item(&item, owner, CategoryKey::new(owner, utils::FolderId::new(parent)))?);
        }
    }
    Ok(items)
}

fn to_folder_fetch(folder: protocol::inventory::FolderDescendents, key: CategoryKey) -> Result<FolderFetch, InventoryFetchError> {
    let mut children = Vec::with_capacity(folder.categories.len().saturating_add(folder.items.len()));
    for category in &folder.categories {
        children.push(InventoryChild::Category(to_category(category, key.owner(), Some(key))?));
    }
    for item in &folder.items {
        children.push(InventoryChild::Item(to_item(item, key.owner(), key)?));
    }
    Ok(FolderFetch {
        folder: key,
        version: ::inventory::ServerVersion::from_raw(folder.version),
        descendent_count: Some(folder.descendent_count),
        children,
    })
}

fn folder_request(folders: &[CategoryKey]) -> Value {
    Value::Map(Map::from([("folders".into(), Value::Array(folders.iter().map(|folder| folder_request_entry(*folder)).collect()))]))
}

fn folder_request_entry(folder: CategoryKey) -> Value {
    Value::Map(Map::from([
        ("folder_id".into(), Value::Uuid(folder.id().uuid())),
        ("owner_id".into(), Value::Uuid(folder.owner().id())),
        ("sort_order".into(), Value::Integer(0)),
        ("fetch_folders".into(), Value::Boolean(true)),
        ("fetch_items".into(), Value::Boolean(true)),
    ]))
}

fn item_request(item_ids: &[Uuid]) -> Value {
    Value::Map(Map::from([(
        "items".into(),
        Value::Array(item_ids.iter().copied().map(|item_id| Value::Map(Map::from([("item_id".into(), Value::Uuid(item_id))]))).collect()),
    )]))
}

fn classify_decode_error(error: DecodeError) -> RequestFailure {
    match error {
        DecodeError::TooLarge => RequestFailure::FolderTooLarge,
        DecodeError::InvalidResponse => RequestFailure::InvalidResponse,
    }
}

fn map_decode_error(error: DecodeError) -> InventoryFetchError {
    match error {
        DecodeError::TooLarge => InventoryFetchError::FolderTooLarge,
        DecodeError::InvalidResponse => InventoryFetchError::InvalidResponse,
    }
}

fn map_request_error(error: network::Error) -> InventoryFetchError {
    match classify_request_error(error) {
        RequestFailure::FolderTooLarge => InventoryFetchError::FolderTooLarge,
        RequestFailure::InvalidResponse => InventoryFetchError::InvalidResponse,
        RequestFailure::Transient(_) => InventoryFetchError::RetryExhausted,
        RequestFailure::Forbidden | RequestFailure::Failed => InventoryFetchError::RequestFailed,
    }
}
