use ::inventory::{CategoryKey, InventoryOwner};
use network::capabilities::{CapabilityUrl, Client};
use protocol::inventory::{self, DecodeError};
use utils::FolderId;

use super::{
    FolderFetch, InventoryFetchError, MAX_AIS_DEPTH, MAX_FETCH_RESPONSE_BYTES, RequestFailure, classify_request_error,
    decode::{ais_folders, to_item},
};

pub(super) async fn fetch_once(
    client: &Client,
    capability: &CapabilityUrl,
    folder: CategoryKey,
    depth: u8,
) -> Result<Vec<FolderFetch>, RequestFailure> {
    let folder_id = folder.id().uuid().to_string();
    let url = capability
        .with_path_segments(&["category", &folder_id, "children"])
        .map_err(|_| RequestFailure::Failed)?
        .with_query_pair("depth", &depth.min(MAX_AIS_DEPTH).to_string());
    let body =
        client.get_bytes_with_accept(&url, "application/llsd+xml", MAX_FETCH_RESPONSE_BYTES).await.map_err(classify_request_error)?;
    let category = inventory::decode_ais_children(&body, folder.id().uuid(), folder.owner().id()).map_err(classify_decode_error)?;
    ais_folders(&category, folder).map_err(|error| match error {
        InventoryFetchError::FolderTooLarge => RequestFailure::FolderTooLarge,
        _ => RequestFailure::InvalidResponse,
    })
}

pub(super) async fn fetch_item_once(
    client: &Client,
    capability: &CapabilityUrl,
    owner: InventoryOwner,
    item_id: uuid::Uuid,
) -> Result<::inventory::Item, RequestFailure> {
    let item_id_text = item_id.to_string();
    let url = capability.with_path_segments(&["item", &item_id_text]).map_err(|_| RequestFailure::Failed)?;
    let body =
        client.get_bytes_with_accept(&url, "application/llsd+xml", MAX_FETCH_RESPONSE_BYTES).await.map_err(classify_request_error)?;
    let item = inventory::decode_ais_item(&body, item_id, owner.id()).map_err(classify_decode_error)?;
    let parent = item.parent_id.ok_or(RequestFailure::InvalidResponse)?;
    to_item(&item, owner, CategoryKey::new(owner, FolderId::new(parent))).map_err(|_| RequestFailure::InvalidResponse)
}

fn classify_decode_error(error: DecodeError) -> RequestFailure {
    match error {
        DecodeError::TooLarge => RequestFailure::FolderTooLarge,
        DecodeError::InvalidResponse => RequestFailure::InvalidResponse,
    }
}
