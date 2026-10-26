use std::{
    collections::{HashMap, HashSet},
    fmt::{self, Display, Formatter},
    str::from_utf8,
};

use ::inventory::{AssetType, CategoryKey, FolderType, InventoryChild, InventoryOwner, InventorySnapshot, InventoryType, Item};
use futures::{StreamExt, TryStreamExt, stream};
use network::{
    AgentSession,
    capabilities::{CapabilityUrl, Client},
};
use serde::{Deserialize, Serialize};
use utils::{FolderId, RegionId};
use uuid::Uuid;
use world::RegionHandle;

use super::{
    favorite_store::MAX_FAVORITES_PER_AGENT,
    fetch::{fetch_single_folder, fetch_single_items},
};
use crate::session::{RegionGridCell, RegionLookup, RegionLookupError, map_request_coordinates};

const REQUESTED_CAPABILITIES: [&str; 4] = ["InventoryAPIv3", "FetchInventoryDescendents2", "FetchInventory2", "ViewerAsset"];
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
    InvalidFavoritesItemList,
    TooManyFolderItems,
    ResolveFavoriteItems(String),
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
            Self::InvalidFavoritesItemList => formatter.write_str("Second Life returned an invalid Favorites item list."),
            Self::TooManyFolderItems => {
                write!(formatter, "Second Life returned more than {MAX_FAVORITES_PER_AGENT} items in the Favorites folder.")
            }
            Self::ResolveFavoriteItems(detail) => write!(formatter, "Could not resolve favorite landmarks: {detail}"),
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
    let ais = capability_urls.get("InventoryAPIv3");
    let descendents = capability_urls.get("FetchInventoryDescendents2");
    if ais.is_none() && descendents.is_none() {
        return Err(FavoritesError::CapabilityUnavailable("InventoryAPIv3 or FetchInventoryDescendents2".into()));
    }
    let fetch_items = capability_urls.get("FetchInventory2");
    let viewer_asset = capability(&capability_urls, "ViewerAsset")?;
    let owner = InventoryOwner::new(agent_id);
    let folder = CategoryKey::new(owner, FolderId::new(favorite_folder_id));
    let folder_response = fetch_single_folder(capabilities, ais, descendents, folder).await.map_err(|error| match error {
        super::fetch::InventoryFetchError::InvalidResponse => FavoritesError::InvalidFavoritesItemList,
        error => FavoritesError::FetchFavoritesFolder(error.to_string()),
    })?;
    let items = favorite_folder_items(folder_response.children)?;
    let candidates = resolve_favorite_items(capabilities, ais, fetch_items, owner, items).await?;
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

fn favorite_folder_items(children: Vec<InventoryChild>) -> Result<Vec<Item>, FavoritesError> {
    let items = children.into_iter().filter_map(|child| match child {
        InventoryChild::Item(item) => Some(item),
        InventoryChild::Category(_) => None,
    });
    let mut output = Vec::new();
    for item in items {
        if output.len() >= MAX_FAVORITES_PER_AGENT {
            return Err(FavoritesError::TooManyFolderItems);
        }
        output.push(item);
    }
    Ok(output)
}

async fn resolve_favorite_items(
    capabilities: &Client,
    ais: Option<&CapabilityUrl>,
    fetch_items: Option<&CapabilityUrl>,
    owner: InventoryOwner,
    items: Vec<Item>,
) -> Result<Vec<(String, Uuid)>, FavoritesError> {
    let items_by_id: HashMap<Uuid, Item> = items.iter().map(|item| (item.key.id().uuid(), item.clone())).collect();
    let mut links = Vec::new();
    let mut linked_item_ids = HashSet::new();
    for item in &items {
        if is_link(item) && !item.asset_id.uuid().is_nil() && !item.name.is_empty() {
            let target_id = item.asset_id.uuid();
            linked_item_ids.insert(target_id);
            links.push((item.name.to_string(), target_id));
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
        let items = fetch_single_items(capabilities, ais, fetch_items, owner, &unresolved_ids)
            .await
            .map_err(|error| FavoritesError::ResolveFavoriteItems(error.to_string()))?;
        if items.len() > MAX_FAVORITES_PER_AGENT {
            return Err(FavoritesError::TooManyLinkedItems);
        }
        items
    };
    let mut all_items = items_by_id;
    all_items.extend(fetched_items.into_iter().map(|item| (item.key.id().uuid(), item)));

    let mut candidates = Vec::new();
    for item in items {
        if is_link(&item) || linked_item_ids.contains(&item.key.id().uuid()) {
            continue;
        }
        if is_landmark(&item) && !item.asset_id.uuid().is_nil() {
            candidates.push((item.name.to_string(), item.asset_id.uuid()));
        }
    }
    for (link_name, target_id) in links {
        if let Some(target) = all_items.get(&target_id)
            && is_landmark(target)
            && !target.asset_id.uuid().is_nil()
        {
            candidates.push((link_name, target.asset_id.uuid()));
        }
    }

    let mut seen = HashSet::new();
    candidates.retain(|(name, asset_id)| seen.insert((name.clone(), *asset_id)));
    Ok(candidates)
}

fn is_link(item: &Item) -> bool {
    item.asset_type == AssetType::Link
}

fn is_landmark(item: &Item) -> bool {
    item.asset_type == AssetType::Landmark || item.inventory_type == InventoryType::Landmark
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

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use ::inventory::{
        AssetType, CategoryKey, InventoryChild, InventoryOwner, InventoryType, Item, ItemKey, PermissionMask, Permissions, SaleInfo,
        SaleType,
    };
    use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId, RegionId};
    use uuid::Uuid;
    use world::RegionHandle;

    use crate::inventory::favorite_store::MAX_FAVORITES_PER_AGENT;

    use super::{
        FavoritesError, LandmarkDestination, RegionGridCell, favorite_folder_items, global_position, is_link, parse_landmark_asset,
        parse_position,
    };

    fn inventory_item(inventory_type: InventoryType, asset_type: AssetType) -> Item {
        let owner = InventoryOwner::new(Uuid::from_u128(1));
        Item {
            key: ItemKey::new(owner, ItemId::new(Uuid::from_u128(2))),
            parent: CategoryKey::new(owner, FolderId::new(Uuid::from_u128(3))),
            asset_id: AssetId::new(Uuid::nil()),
            asset_type,
            inventory_type,
            flags: 0,
            name: "favorite".into(),
            description: "".into(),
            created_at: 0,
            permissions: Permissions {
                creator: AvatarId::new(Uuid::nil()),
                owner: AvatarId::new(Uuid::nil()),
                group: GroupId::new(Uuid::nil()),
                last_owner: AvatarId::new(Uuid::nil()),
                base: PermissionMask::new(0),
                owner_mask: PermissionMask::new(0),
                group_mask: PermissionMask::new(0),
                everyone: PermissionMask::new(0),
                next_owner: PermissionMask::new(0),
            },
            sale: SaleInfo { sale_type: SaleType::NotForSale, price: 0 },
        }
    }

    #[test]
    fn identifies_links_by_asset_type_not_inventory_type() {
        let link = inventory_item(InventoryType::Landmark, AssetType::Link);
        let person = inventory_item(InventoryType::Person, AssetType::Person);
        let folder_link = inventory_item(InventoryType::Category, AssetType::LinkFolder);
        let landmark = inventory_item(InventoryType::Landmark, AssetType::Landmark);
        let numeric_link = inventory_item(InventoryType::Landmark, AssetType::Link);

        assert!(is_link(&link));
        assert!(!is_link(&person));
        assert!(!is_link(&folder_link));
        assert!(!is_link(&landmark));
        assert!(is_link(&numeric_link));
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
        let item = inventory_item(InventoryType::Landmark, AssetType::Landmark);
        let children = vec![InventoryChild::Item(item); MAX_FAVORITES_PER_AGENT + 1];

        let error = favorite_folder_items(children).unwrap_err();

        assert_matches!(error, FavoritesError::TooManyFolderItems);
    }
}
