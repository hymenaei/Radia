use std::{
    collections::{HashMap, HashSet},
    fmt::{self, Display, Formatter},
    str::from_utf8,
};

use ::inventory::{FolderType, InventorySnapshot};
use futures::{StreamExt, TryStreamExt, stream};
use llsd::{Map, Value};
use network::{
    AgentSession,
    capabilities::{CapabilityUrl, Client},
};
use serde::{Deserialize, Serialize};
use utils::RegionId;
use uuid::Uuid;
use world::RegionHandle;

use super::favorite_store::MAX_FAVORITES_PER_AGENT;
use crate::session::{RegionGridCell, RegionLookup, RegionLookupError, map_request_coordinates};

const REQUESTED_CAPABILITIES: [&str; 3] = ["FetchInventoryDescendents2", "FetchInventory2", "ViewerAsset"];
const ASSET_TYPE_LINK: i32 = 24;
const ASSET_TYPE_LINK_FOLDER: i32 = 25;
const ASSET_TYPE_LANDMARK: i32 = 3;
const INVENTORY_TYPE_LANDMARK: i32 = 3;
const INVENTORY_TYPE_PERSON: i32 = 24;
const REGION_LOOKUP_CONCURRENCY: usize = 4;
const LANDMARK_FETCH_CONCURRENCY: usize = 3;
const MAX_LANDMARK_ASSET_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FavoriteLocation {
    pub name: String,
    pub region_name: String,
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

#[derive(Clone, Debug)]
pub struct FavoriteLandmark {
    name: String,
    destination: LandmarkDestination,
}

impl FavoriteLandmark {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn destination(&self) -> LandmarkDestination {
        self.destination
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LandmarkDestination {
    Global([f64; 3]),
    Region { id: RegionId, position: [f64; 3] },
    RegionHandle { handle: RegionHandle, position: [f64; 3] },
}

#[derive(Clone, Debug)]
pub enum FavoritesError {
    MissingFavoritesFolder,
    RequestCapabilities(String),
    CapabilityUnavailable(String),
    FetchFavoritesFolder(String),
    InvalidFolderResponse,
    FavoritesFolderNotReturned,
    InvalidFavoritesFolder,
    InvalidFavoritesItemList,
    TooManyFolderItems,
    ResolveFavoriteItems(String),
    InvalidLinkedItemsResponse,
    TooManyLinkedItems,
    FetchLandmarkAsset { name: String, detail: String },
    NoReadableFavoriteLandmarks,
    RegionLookup(RegionLookupError),
}

impl Display for FavoritesError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFavoritesFolder => formatter.write_str("The personal inventory does not include a Favorites folder."),
            Self::RequestCapabilities(detail) => write!(formatter, "Could not request inventory capabilities: {detail}"),
            Self::CapabilityUnavailable(name) => write!(formatter, "Second Life did not provide the {name} capability."),
            Self::FetchFavoritesFolder(detail) => write!(formatter, "Could not fetch the Favorites folder: {detail}"),
            Self::InvalidFolderResponse => formatter.write_str("Second Life returned an invalid Favorites folder response."),
            Self::FavoritesFolderNotReturned => formatter.write_str("Second Life did not return the Favorites folder."),
            Self::InvalidFavoritesFolder => formatter.write_str("Second Life returned an invalid Favorites folder."),
            Self::InvalidFavoritesItemList => formatter.write_str("Second Life returned an invalid Favorites item list."),
            Self::TooManyFolderItems => {
                write!(formatter, "Second Life returned more than {MAX_FAVORITES_PER_AGENT} items in the Favorites folder.")
            }
            Self::ResolveFavoriteItems(detail) => write!(formatter, "Could not resolve favorite landmarks: {detail}"),
            Self::InvalidLinkedItemsResponse => formatter.write_str("Second Life returned an invalid linked-item response."),
            Self::TooManyLinkedItems => {
                write!(formatter, "Second Life returned more than {MAX_FAVORITES_PER_AGENT} linked favorite items.")
            }
            Self::FetchLandmarkAsset { name, detail } => {
                write!(formatter, "Could not fetch favorite landmark asset {name:?}: {detail}")
            }
            Self::NoReadableFavoriteLandmarks => formatter.write_str("Second Life did not return readable data for any favorite landmark."),
            Self::RegionLookup(error) => write!(formatter, "Could not look up a favorite landmark region: {error}"),
        }
    }
}

impl std::error::Error for FavoritesError {}

#[derive(Clone, Debug)]
struct InventoryItem {
    item_id: Option<Uuid>,
    asset_id: Option<Uuid>,
    inventory_type: Option<i32>,
    asset_type: Option<i32>,
    name: Option<String>,
}

impl InventoryItem {
    fn from_llsd(value: &Value) -> Option<Self> {
        let fields = value.as_map()?;
        Some(Self {
            item_id: uuid_field(fields.get("item_id")?),
            asset_id: fields.get("asset_id").and_then(uuid_field),
            inventory_type: fields.get("inv_type").and_then(inventory_type_field),
            asset_type: fields.get("type").and_then(asset_type_field),
            name: fields.get("name").and_then(string_field),
        })
    }

    fn is_link(&self) -> bool {
        self.asset_type == Some(ASSET_TYPE_LINK)
    }

    fn is_landmark(&self) -> bool {
        self.asset_type == Some(ASSET_TYPE_LANDMARK) || self.inventory_type == Some(INVENTORY_TYPE_LANDMARK)
    }
}

pub(crate) async fn load_favorites(
    capabilities: &Client,
    region_lookup: &RegionLookup,
    session: &AgentSession,
    inventory: Option<&InventorySnapshot>,
) -> Result<Vec<FavoriteLocation>, FavoritesError> {
    let folder_id = favorite_folder_id(inventory.ok_or(FavoritesError::MissingFavoritesFolder)?)?;
    let landmarks = fetch_favorite_landmarks(capabilities, session.agent_id, folder_id).await?;
    let resolved = stream::iter(landmarks.into_iter().map(|landmark| async move {
        let (region_name, position) = match landmark.destination {
            LandmarkDestination::Global(position) => {
                let Some((cell, position)) = global_position(position) else {
                    return Ok(None);
                };
                (region_lookup.lookup_region_at(cell).await, position)
            }
            LandmarkDestination::RegionHandle { handle, position } => {
                let origin = handle.origin();
                let Some(cell) = RegionGridCell::containing(origin.x, origin.y) else {
                    return Ok(None);
                };
                if map_request_coordinates(cell).is_none() {
                    return Ok(None);
                }
                (region_lookup.lookup_region_at(cell).await, position)
            }
            LandmarkDestination::Region { id, position } => (region_lookup.lookup_region_by_id(id.uuid()).await, position),
        };
        let region_name = region_name.map_err(FavoritesError::RegionLookup)?;
        Ok(favorite_location(landmark.name, region_name, position))
    }))
    .buffer_unordered(REGION_LOOKUP_CONCURRENCY)
    .collect::<Vec<Result<Option<FavoriteLocation>, FavoritesError>>>()
    .await;
    let mut locations = resolved.into_iter().collect::<Result<Vec<_>, _>>()?.into_iter().flatten().collect::<Vec<_>>();
    locations.sort_by_key(|location| location.name.to_lowercase());
    Ok(locations)
}

pub fn favorite_folder_id(inventory: &InventorySnapshot) -> Result<Uuid, FavoritesError> {
    let owner = inventory.roots().personal().owner();
    inventory
        .category_with_type(owner, FolderType::Favorite)
        .map(|category| category.key.id().uuid())
        .ok_or(FavoritesError::MissingFavoritesFolder)
}

pub async fn fetch_favorite_landmarks(
    capabilities: &Client,
    agent_id: Uuid,
    favorite_folder_id: Uuid,
) -> Result<Vec<FavoriteLandmark>, FavoritesError> {
    let capability_urls = capabilities
        .request_capabilities(&REQUESTED_CAPABILITIES)
        .await
        .map_err(|error| FavoritesError::RequestCapabilities(error.to_string()))?;
    let descendents = capability(&capability_urls, "FetchInventoryDescendents2")?;
    let fetch_items = capability(&capability_urls, "FetchInventory2")?;
    let viewer_asset = capability(&capability_urls, "ViewerAsset")?;

    let folder_response = capabilities
        .post(&descendents, &folder_request(agent_id, favorite_folder_id))
        .await
        .map_err(|error| FavoritesError::FetchFavoritesFolder(error.to_string()))?;
    let items = favorite_folder_items(&folder_response, favorite_folder_id)?;
    let candidates = resolve_favorite_items(capabilities, &fetch_items, items).await?;
    let candidate_count = candidates.len();

    let landmarks = stream::iter(candidates.into_iter().map(|(name, asset_id)| {
        let capabilities = capabilities.clone();
        let asset_url = viewer_asset.with_query_pair("landmark_id", &asset_id.to_string());
        async move { fetch_landmark(&capabilities, asset_url, name).await }
    }))
    .buffered(LANDMARK_FETCH_CONCURRENCY)
    .try_collect::<Vec<_>>()
    .await?
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if candidate_count > 0 && landmarks.is_empty() {
        return Err(FavoritesError::NoReadableFavoriteLandmarks);
    }
    Ok(landmarks)
}

fn capability(capabilities: &HashMap<String, CapabilityUrl>, name: &str) -> Result<CapabilityUrl, FavoritesError> {
    capabilities.get(name).cloned().ok_or_else(|| FavoritesError::CapabilityUnavailable(name.to_owned()))
}

fn folder_request(agent_id: Uuid, folder_id: Uuid) -> Value {
    Value::Map(Map::from([(
        "folders".into(),
        Value::Array(vec![Value::Map(Map::from([
            ("folder_id".into(), Value::Uuid(folder_id)),
            ("owner_id".into(), Value::Uuid(agent_id)),
            ("sort_order".into(), Value::Integer(0)),
            ("fetch_folders".into(), Value::Boolean(true)),
            ("fetch_items".into(), Value::Boolean(true)),
        ]))]),
    )]))
}

fn item_request(item_ids: &[Uuid]) -> Value {
    Value::Map(Map::from([(
        "items".into(),
        Value::Array(item_ids.iter().copied().map(|item_id| Value::Map(Map::from([("item_id".into(), Value::Uuid(item_id))]))).collect()),
    )]))
}

fn favorite_folder_items(response: &Value, requested_folder_id: Uuid) -> Result<Vec<InventoryItem>, FavoritesError> {
    let folders = response
        .as_map()
        .and_then(|fields| fields.get("folders"))
        .and_then(Value::as_array)
        .ok_or(FavoritesError::InvalidFolderResponse)?;
    let folder = folders
        .iter()
        .find(|folder| folder.as_map().and_then(|fields| fields.get("folder_id")).and_then(uuid_field) == Some(requested_folder_id))
        .ok_or(FavoritesError::FavoritesFolderNotReturned)?;
    let folder_fields = folder.as_map().ok_or(FavoritesError::InvalidFavoritesFolder)?;
    let entries = match folder_fields.get("items").and_then(Value::as_array) {
        Some(entries) => entries,
        None if folder_fields.get("descendents").and_then(integer_field) == Some(0)
            && matches!(
                folder_fields.get("categories"),
                Some(Value::Array(categories)) if categories.is_empty()
            ) =>
        {
            return Ok(Vec::new());
        }
        None => {
            return Err(FavoritesError::InvalidFavoritesItemList);
        }
    };
    if entries.len() > MAX_FAVORITES_PER_AGENT {
        return Err(FavoritesError::TooManyFolderItems);
    }

    Ok(entries.iter().filter_map(InventoryItem::from_llsd).collect())
}

async fn resolve_favorite_items(
    capabilities: &Client,
    fetch_items: &CapabilityUrl,
    items: Vec<InventoryItem>,
) -> Result<Vec<(String, Uuid)>, FavoritesError> {
    let items_by_id: HashMap<Uuid, InventoryItem> = items.iter().filter_map(|item| item.item_id.map(|id| (id, item.clone()))).collect();
    let mut links = Vec::new();
    let mut linked_item_ids = HashSet::new();
    for item in &items {
        if item.is_link()
            && let (Some(target_id), Some(name)) = (item.asset_id, item.name.as_ref())
        {
            linked_item_ids.insert(target_id);
            links.push((name.clone(), target_id));
        }
    }

    let unresolved_ids: Vec<Uuid> = links
        .iter()
        .map(|(_, item_id)| *item_id)
        .filter(|item_id| !items_by_id.contains_key(item_id))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let fetched_items = if unresolved_ids.is_empty() {
        Vec::new()
    } else {
        let response = capabilities
            .post(fetch_items, &item_request(&unresolved_ids))
            .await
            .map_err(|error| FavoritesError::ResolveFavoriteItems(error.to_string()))?;
        let entries = response
            .as_map()
            .and_then(|fields| fields.get("items"))
            .and_then(Value::as_array)
            .ok_or(FavoritesError::InvalidLinkedItemsResponse)?;
        if entries.len() > MAX_FAVORITES_PER_AGENT {
            return Err(FavoritesError::TooManyLinkedItems);
        }
        entries.iter().filter_map(InventoryItem::from_llsd).collect()
    };
    let mut all_items = items_by_id;
    all_items.extend(fetched_items.into_iter().filter_map(|item| item.item_id.map(|id| (id, item))));

    let mut candidates = Vec::new();
    for item in items {
        if item.is_link() || item.item_id.is_some_and(|id| linked_item_ids.contains(&id)) {
            continue;
        }
        if item.is_landmark()
            && let (Some(name), Some(asset_id)) = (item.name, item.asset_id)
        {
            candidates.push((name, asset_id));
        }
    }
    for (link_name, target_id) in links {
        if let Some(target) = all_items.get(&target_id)
            && target.is_landmark()
            && let Some(asset_id) = target.asset_id
        {
            candidates.push((link_name, asset_id));
        }
    }

    let mut seen = HashSet::new();
    candidates.retain(|(name, asset_id)| seen.insert((name.clone(), *asset_id)));
    Ok(candidates)
}

async fn fetch_landmark(capabilities: &Client, asset_url: CapabilityUrl, name: String) -> Result<Option<FavoriteLandmark>, FavoritesError> {
    let bytes = capabilities
        .get_bytes_with_response_limit(&asset_url, MAX_LANDMARK_ASSET_BYTES)
        .await
        .map_err(|error| FavoritesError::FetchLandmarkAsset { name: name.clone(), detail: error.to_string() })?;
    Ok(parse_landmark_asset(&name, &bytes))
}

fn global_position([global_x, global_y, z]: [f64; 3]) -> Option<(RegionGridCell, [f64; 3])> {
    if ![global_x, global_y, z].iter().all(|value| value.is_finite()) {
        return None;
    }
    let cell = RegionGridCell::containing(global_x, global_y)?;
    map_request_coordinates(cell)?;
    let local_x = global_x - f64::from(cell.x()) * f64::from(RegionGridCell::METERS_PER_CELL);
    let local_y = global_y - f64::from(cell.y()) * f64::from(RegionGridCell::METERS_PER_CELL);
    Some((cell, [local_x, local_y, z]))
}

fn favorite_location(name: String, region_name: String, [x, y, z]: [f64; 3]) -> Option<FavoriteLocation> {
    if ![x, y, z].iter().all(|value| value.is_finite())
        || !(0.0..256.0).contains(&x)
        || !(0.0..256.0).contains(&y)
        || !(0.0..=4096.0).contains(&z)
    {
        return None;
    }
    Some(FavoriteLocation { name, region_name, x: x.floor() as u32, y: y.floor() as u32, z: z.floor() as u32 })
}

fn parse_landmark_asset(name: &str, bytes: &[u8]) -> Option<FavoriteLandmark> {
    let text = from_utf8(bytes).ok()?.trim_start_matches('\u{feff}').trim_matches('\0');
    let mut lines = text.lines();
    let version = lines.next()?.trim();
    let mut fields = HashMap::new();
    for line in lines {
        let mut words = line.split_whitespace();
        if let Some(key) = words.next() {
            fields.insert(key, words.collect::<Vec<_>>());
        }
    }

    let destination = match version {
        "Landmark version 1" => {
            let position = fields.get("global_pos").or_else(|| fields.get("global_position")).and_then(|parts| parse_position(parts))?;
            LandmarkDestination::Global(position)
        }
        "Landmark version 2" => {
            let position = fields.get("local_pos").and_then(|parts| parse_position(parts))?;
            if let Some(handle) = fields.get("region_handle").and_then(|parts| parts.first()).and_then(|value| parse_region_handle(value)) {
                LandmarkDestination::RegionHandle { handle: RegionHandle::new(handle), position }
            } else {
                let id = fields.get("region_id").and_then(|parts| parts.first()).and_then(|value| Uuid::parse_str(value).ok())?;
                LandmarkDestination::Region { id: RegionId::new(id), position }
            }
        }
        _ => return None,
    };
    Some(FavoriteLandmark { name: name.to_owned(), destination })
}

fn parse_position(parts: &[&str]) -> Option<[f64; 3]> {
    let mut parts =
        parts.iter().flat_map(|part| part.split([',', '<', '>', '(', ')'])).filter(|part| !part.is_empty()).map(str::parse::<f64>);
    let x = parts.next()?.ok()?;
    let y = parts.next()?.ok()?;
    let z = parts.next()?.ok()?;
    if parts.next().is_some() {
        return None;
    }

    let position = [x, y, z];
    position.iter().all(|value| value.is_finite()).then_some(position)
}

fn parse_region_handle(value: &str) -> Option<u64> {
    value.parse().ok().or_else(|| value.strip_prefix("0x").and_then(|hex| u64::from_str_radix(hex, 16).ok()))
}

fn uuid_field(value: &Value) -> Option<Uuid> {
    match value {
        Value::Uuid(uuid) => Some(*uuid),
        Value::String(value) => Uuid::parse_str(value).ok(),
        _ => None,
    }
}

fn integer_field(value: &Value) -> Option<i32> {
    match value {
        Value::Integer(value) => Some(*value),
        _ => None,
    }
}

fn inventory_type_field(value: &Value) -> Option<i32> {
    match value {
        Value::String(value) => match value.as_str() {
            "landmark" => Some(INVENTORY_TYPE_LANDMARK),
            "person" => Some(INVENTORY_TYPE_PERSON),
            _ => None,
        },
        _ => integer_field(value),
    }
}

fn asset_type_field(value: &Value) -> Option<i32> {
    match value {
        Value::String(value) => match value.as_str() {
            "landmark" => Some(ASSET_TYPE_LANDMARK),
            "link" => Some(ASSET_TYPE_LINK),
            "link_f" => Some(ASSET_TYPE_LINK_FOLDER),
            _ => None,
        },
        _ => integer_field(value),
    }
}

fn string_field(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Uri(value) => Some(value.as_str().to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use llsd::{Map, Value};
    use utils::RegionId;
    use uuid::Uuid;
    use world::RegionHandle;

    use crate::inventory::favorite_store::MAX_FAVORITES_PER_AGENT;

    use super::{
        FavoritesError, InventoryItem, LandmarkDestination, RegionGridCell, favorite_folder_items, global_position, parse_landmark_asset,
        parse_position,
    };

    fn inventory_item(inventory_type: Value, asset_type: Value) -> InventoryItem {
        let value = Value::Map(Map::from([
            ("item_id".into(), Value::Uuid(Uuid::nil())),
            ("inv_type".into(), inventory_type),
            ("type".into(), asset_type),
        ]));
        InventoryItem::from_llsd(&value).expect("inventory item fields")
    }

    #[test]
    fn identifies_links_by_asset_type_not_inventory_type() {
        let link = inventory_item(Value::String("landmark".into()), Value::String("link".into()));
        let person = inventory_item(Value::String("person".into()), Value::String("person".into()));
        let folder_link = inventory_item(Value::String("category".into()), Value::String("link_f".into()));
        let landmark = inventory_item(Value::Integer(3), Value::Integer(3));
        let numeric_link = inventory_item(Value::Integer(3), Value::Integer(24));

        assert!(link.is_link());
        assert!(!person.is_link());
        assert!(!folder_link.is_link());
        assert!(!landmark.is_link());
        assert!(numeric_link.is_link());
    }

    #[test]
    fn parses_handle_only_landmark() {
        let handle = ((42_u64 * 256) << 32) | (73 * 256);
        let asset = format!("Landmark version 2\nregion_handle {handle}\nlocal_pos <128, 64, 30>\n");

        let landmark = parse_landmark_asset("Home", asset.as_bytes()).expect("region handle identifies the region");

        assert_eq!(
            landmark.destination,
            LandmarkDestination::RegionHandle { handle: RegionHandle::new(handle), position: [128.0, 64.0, 30.0] }
        );
        assert!(parse_landmark_asset("Unlocated", b"Landmark version 2\nlocal_pos <128, 64, 30>\n").is_none());
        assert!(parse_landmark_asset("Unpositioned", b"Landmark version 2\nregion_handle 0\n").is_none());
    }

    #[test]
    fn region_reference_precedence() {
        let id = Uuid::from_u128(42);
        let position = [128.0, 64.0, 30.0];
        for (handle, expected) in [
            ("region_handle 512", LandmarkDestination::RegionHandle { handle: RegionHandle::new(512), position }),
            ("region_handle invalid", LandmarkDestination::Region { id: RegionId::new(id), position }),
            ("", LandmarkDestination::Region { id: RegionId::new(id), position }),
        ] {
            let asset = format!("Landmark version 2\nregion_id {id}\n{handle}\nlocal_pos <128, 64, 30>\n");
            let landmark = parse_landmark_asset("Home", asset.as_bytes()).expect("at least one valid region reference");
            assert_eq!(landmark.destination, expected, "{handle:?}");
        }
    }

    #[test]
    fn position_requires_three_finite_values() {
        assert_eq!(parse_position(&["<128,", "128,", "30>"]), Some([128.0, 128.0, 30.0]));
        assert_eq!(parse_position(&["1,2,3,4"]), None);
        assert_eq!(parse_position(&["1,NaN,3"]), None);
    }

    #[test]
    fn global_landmark_position_uses_world_grid_cells() {
        let (cell, position) = global_position([512.5, 769.0, 30.0]).expect("finite positive location");
        assert_eq!(cell, RegionGridCell::new(2, 3));
        assert_eq!(position, [0.5, 1.0, 30.0]);
        assert!(global_position([-1.0, 0.0, 0.0]).is_none());
        assert!(global_position([f64::INFINITY, 0.0, 0.0]).is_none());
        assert!(global_position([f64::from(u32::from(u16::MAX) + 1) * f64::from(RegionGridCell::METERS_PER_CELL), 0.0, 0.0]).is_none());
    }

    #[test]
    fn oversized_favorites_folder_is_rejected_before_item_parsing() {
        let folder_id = Uuid::from_u128(42);
        let entries = vec![Value::Integer(0); MAX_FAVORITES_PER_AGENT + 1];
        let response = Value::Map(Map::from([(
            "folders".into(),
            Value::Array(vec![Value::Map(Map::from([
                ("folder_id".into(), Value::Uuid(folder_id)),
                ("items".into(), Value::Array(entries)),
            ]))]),
        )]));

        let error = favorite_folder_items(&response, folder_id).unwrap_err();

        assert_matches!(error, FavoritesError::TooManyFolderItems);
    }
}
