use std::{collections::BTreeMap, error, fmt};

use llsd::{Limits, Value, xml};
use uuid::Uuid;

const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_RESPONSE_DEPTH: usize = 192;
const MAX_RESPONSE_VALUES: usize = 4_000_000;
const MAX_CONTAINER_ITEMS: usize = 1_000_000;
const MAX_STRING_BYTES: usize = 4_096;
const MAX_MAP_KEY_BYTES: usize = 128;
const MAX_NAME_BYTES: usize = 1_024;
const MAX_DESCRIPTION_BYTES: usize = 4_096;
const MAX_HYPERMEDIA_URI_BYTES: usize = 2_048;
const MAX_AIS_DEPTH: usize = 50;
const MAX_CATEGORIES: usize = 131_074;
const MAX_ITEMS: usize = 1_000_000;
const MAX_FOLDER_CHILDREN: usize = 100_000;
const MAX_FOLDERS_PER_BATCH: usize = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    InvalidResponse,
    TooLarge,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidResponse => "inventory response is invalid",
            Self::TooLarge => "inventory response exceeds a decoding limit",
        })
    }
}

impl error::Error for DecodeError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Category {
    pub id: Uuid,
    pub owner_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub name: String,
    pub version: Option<i32>,
    pub descendent_count: Option<u32>,
    pub preferred_folder_type: Option<i32>,
    pub children: Option<Vec<Child>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Child {
    Category(Category),
    Item(Item),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Item {
    pub id: Uuid,
    pub owner_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub asset_id: Option<Uuid>,
    pub asset_type: Option<i32>,
    pub inventory_type: Option<i32>,
    pub name: String,
    pub description: String,
    pub flags: u32,
    pub created_at: i64,
    pub permissions: Permissions,
    pub sale_info: SaleInfo,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Permissions {
    pub creator_id: Option<Uuid>,
    pub owner_id: Option<Uuid>,
    pub group_id: Option<Uuid>,
    pub last_owner_id: Option<Uuid>,
    pub base_mask: Option<u32>,
    pub owner_mask: Option<u32>,
    pub group_mask: Option<u32>,
    pub everyone_mask: Option<u32>,
    pub next_owner_mask: Option<u32>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SaleInfo {
    pub sale_type: Option<i32>,
    pub sale_price: Option<i32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FolderDescendents {
    pub id: Uuid,
    pub owner_id: Option<Uuid>,
    pub version: i32,
    pub descendent_count: u32,
    pub categories: Vec<Category>,
    pub items: Vec<Item>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestedFolder {
    pub id: Uuid,
    pub owner_id: Uuid,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AisEditResponse {
    pub category: Option<Category>,
    pub item: Option<Item>,
    pub created_categories: Vec<Uuid>,
    pub created_items: Vec<Uuid>,
    pub updated_categories: Vec<Uuid>,
    pub updated_items: Vec<Uuid>,
    pub removed_items: Vec<Uuid>,
    pub categories_removed: Vec<Uuid>,
    pub category_items_removed: Vec<Uuid>,
    pub broken_links_removed: Vec<Uuid>,
    pub updated_category_versions: BTreeMap<Uuid, i32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatedCategoryResponse {
    pub id: Uuid,
    pub parent_id: Uuid,
    pub preferred_folder_type: i32,
    pub name: String,
}

#[derive(Default)]
struct ParseCounts {
    categories: usize,
    items: usize,
}

#[derive(Clone, Copy)]
enum InventoryWireFormat {
    Ais,
    Legacy,
}

pub fn decode_ais_children(bytes: &[u8], expected_folder: Uuid, expected_owner: Uuid) -> Result<Category, DecodeError> {
    let response = decode_llsd(bytes)?;
    let mut counts = ParseCounts::default();
    let category = parse_category(&response, expected_owner, None, 0, &mut counts, InventoryWireFormat::Ais)?;
    if category.id != expected_folder {
        return Err(DecodeError::InvalidResponse);
    }
    Ok(category)
}

pub fn decode_ais_item(bytes: &[u8], expected_item: Uuid, expected_owner: Uuid) -> Result<Item, DecodeError> {
    let response = decode_llsd(bytes)?;
    validate_hypermedia(map(&response)?)?;
    let item = parse_item(&response, expected_owner, None, false, InventoryWireFormat::Ais)?;
    if item.id != expected_item || item.parent_id.is_none_or(|parent| parent.is_nil()) {
        return Err(DecodeError::InvalidResponse);
    }
    Ok(item)
}

pub fn decode_ais_edit_response(bytes: &[u8], expected_owner: Uuid) -> Result<AisEditResponse, DecodeError> {
    let response = decode_llsd(bytes)?;
    let fields = map(&response)?;
    let has_category = fields.contains_key("category_id");
    let has_item = fields.contains_key("item_id");
    if has_category && has_item {
        return Err(DecodeError::InvalidResponse);
    }
    let category = has_category
        .then(|| parse_category(&response, expected_owner, None, 0, &mut ParseCounts::default(), InventoryWireFormat::Ais))
        .transpose()?;
    let item = has_item.then(|| parse_item(&response, expected_owner, None, false, InventoryWireFormat::Ais)).transpose()?;
    let removed_items = metadata_ids(fields, "_removed_items", MAX_ITEMS)?;
    validate_metadata_ids(fields, "_active_gestures_removed", MAX_ITEMS)?;
    metadata_integer_uuid_map(fields, "_attachments_removed", MAX_ITEMS)?;
    metadata_integer_uuid_map(fields, "_wearables_removed", MAX_ITEMS)?;
    validate_metadata_ids(fields, "_moved_items", MAX_ITEMS)?;
    metadata_uuid_uuid_map(fields, "_linked_ids", MAX_ITEMS)?;
    validate_update_result(fields)?;
    if uuid_field(fields, &["_top_category_id"])?.is_some_and(|id| id.is_nil()) {
        return Err(DecodeError::InvalidResponse);
    }
    if let Some(total) = integer(fields, &["_total_items_removed"])?
        && total < 0
    {
        return Err(DecodeError::InvalidResponse);
    }
    Ok(AisEditResponse {
        category,
        item,
        created_categories: metadata_ids(fields, "_created_categories", MAX_CATEGORIES)?,
        created_items: metadata_ids(fields, "_created_items", MAX_ITEMS)?,
        updated_categories: metadata_ids(fields, "_updated_categories", MAX_CATEGORIES)?,
        updated_items: metadata_ids(fields, "_updated_items", MAX_ITEMS)?,
        removed_items,
        categories_removed: metadata_ids(fields, "_categories_removed", MAX_CATEGORIES)?,
        category_items_removed: metadata_ids(fields, "_category_items_removed", MAX_ITEMS)?,
        broken_links_removed: metadata_ids(fields, "_broken_links_removed", MAX_ITEMS)?,
        updated_category_versions: metadata_versions(fields, "_updated_category_versions")?,
    })
}

pub fn decode_created_category_response(bytes: &[u8]) -> Result<CreatedCategoryResponse, DecodeError> {
    let response = decode_llsd(bytes)?;
    let fields = map(&response)?;
    let id = uuid_field(fields, &["folder_id"])?.filter(|id| !id.is_nil()).ok_or(DecodeError::InvalidResponse)?;
    let parent_id = uuid_field(fields, &["parent_id"])?.filter(|id| !id.is_nil()).ok_or(DecodeError::InvalidResponse)?;
    let preferred_folder_type = integer(fields, &["type"])?.ok_or(DecodeError::InvalidResponse)?;
    let name = string(fields, &["name"], "")?;
    if name.len() > MAX_NAME_BYTES {
        return Err(DecodeError::InvalidResponse);
    }
    Ok(CreatedCategoryResponse { id, parent_id, preferred_folder_type, name })
}

pub fn decode_fetch_inventory_descendents2(bytes: &[u8], requested: &[RequestedFolder]) -> Result<Vec<FolderDescendents>, DecodeError> {
    if requested.is_empty() || requested.len() > MAX_FOLDERS_PER_BATCH {
        return Err(DecodeError::InvalidResponse);
    }
    let response = decode_llsd(bytes)?;
    let fields = map(&response)?;
    let folders = array(fields.get("folders"))?;
    if folders.len() != requested.len() || folders.len() > MAX_FOLDERS_PER_BATCH {
        return Err(DecodeError::InvalidResponse);
    }
    let mut by_id = std::collections::HashMap::with_capacity(folders.len());
    for folder in folders {
        let fields = map(folder)?;
        let id = uuid_field(fields, &["folder_id"])?.ok_or(DecodeError::InvalidResponse)?;
        if by_id.insert(id, folder).is_some() {
            return Err(DecodeError::InvalidResponse);
        }
    }
    requested
        .iter()
        .map(|requested| {
            let response = by_id.get(&requested.id).ok_or(DecodeError::InvalidResponse)?;
            let parsed = parse_folder_descendents(response, *requested)?;
            Ok(parsed)
        })
        .collect()
}

pub fn decode_fetch_inventory2(bytes: &[u8], expected_owner: Uuid, requested_items: &[Uuid]) -> Result<Vec<Item>, DecodeError> {
    if requested_items.len() > MAX_FOLDERS_PER_BATCH {
        return Err(DecodeError::InvalidResponse);
    }
    let response = decode_llsd(bytes)?;
    let fields = map(&response)?;
    let items = array(fields.get("items"))?;
    if items.len() > requested_items.len() {
        return Err(DecodeError::InvalidResponse);
    }
    let requested = requested_items.iter().copied().collect::<std::collections::HashSet<_>>();
    let mut returned = std::collections::HashSet::with_capacity(items.len());
    items
        .iter()
        .map(|value| {
            let item = parse_item(value, expected_owner, None, false, InventoryWireFormat::Legacy)?;
            if item.parent_id.is_none_or(|parent| parent.is_nil()) || !requested.contains(&item.id) || !returned.insert(item.id) {
                return Err(DecodeError::InvalidResponse);
            }
            Ok(item)
        })
        .collect()
}

fn decode_llsd(bytes: &[u8]) -> Result<Value, DecodeError> {
    xml::from_slice_with_limits(
        bytes,
        Limits {
            max_bytes: MAX_RESPONSE_BYTES,
            max_depth: MAX_RESPONSE_DEPTH,
            max_values: MAX_RESPONSE_VALUES,
            max_container_items: MAX_CONTAINER_ITEMS,
            max_string_bytes: MAX_STRING_BYTES,
            max_map_key_bytes: MAX_MAP_KEY_BYTES,
            max_binary_bytes: 0,
        },
    )
    .map_err(|error| match error {
        llsd::Error::LimitExceeded(_) => DecodeError::TooLarge,
        _ => DecodeError::InvalidResponse,
    })
}

fn parse_folder_descendents(value: &Value, requested: RequestedFolder) -> Result<FolderDescendents, DecodeError> {
    let fields = map(value)?;
    let id = uuid_field(fields, &["folder_id"])?.ok_or(DecodeError::InvalidResponse)?;
    let owner_id = uuid_field(fields, &["owner_id"])?;
    if id != requested.id || owner_id.is_some_and(|owner| owner != requested.owner_id) {
        return Err(DecodeError::InvalidResponse);
    }
    let version = integer(fields, &["version"])?.unwrap_or(-1);
    let descendent_count = count(fields, &["descendents"])?.ok_or(DecodeError::InvalidResponse)?;
    if descendent_count as usize > MAX_FOLDER_CHILDREN {
        return Err(DecodeError::TooLarge);
    }
    let raw_categories = optional_array(fields, "categories", descendent_count)?;
    let raw_items = optional_array(fields, "items", descendent_count)?;
    let direct_count = raw_categories.len().checked_add(raw_items.len()).ok_or(DecodeError::TooLarge)?;
    if direct_count > MAX_FOLDER_CHILDREN {
        return Err(DecodeError::TooLarge);
    }
    let mut categories = Vec::with_capacity(raw_categories.len());
    for category in raw_categories {
        categories.push(parse_category(
            category,
            requested.owner_id,
            Some(id),
            0,
            &mut ParseCounts::default(),
            InventoryWireFormat::Legacy,
        )?);
    }
    let mut items = Vec::with_capacity(raw_items.len());
    for item in raw_items {
        items.push(parse_item(item, requested.owner_id, Some(id), false, InventoryWireFormat::Legacy)?);
    }
    Ok(FolderDescendents { id, owner_id, version, descendent_count, categories, items })
}

fn parse_category(
    value: &Value,
    expected_owner: Uuid,
    expected_parent: Option<Uuid>,
    depth: usize,
    counts: &mut ParseCounts,
    format: InventoryWireFormat,
) -> Result<Category, DecodeError> {
    if depth > MAX_AIS_DEPTH {
        return Err(DecodeError::TooLarge);
    }
    counts.categories = counts.categories.checked_add(1).ok_or(DecodeError::TooLarge)?;
    if counts.categories > MAX_CATEGORIES {
        return Err(DecodeError::TooLarge);
    }
    let fields = map(value)?;
    validate_hypermedia(fields)?;
    let id_names = match format {
        InventoryWireFormat::Ais => &["category_id"][..],
        InventoryWireFormat::Legacy => &["folder_id", "category_id"][..],
    };
    let id = uuid_field(fields, id_names)?.filter(|id| !id.is_nil()).ok_or(DecodeError::InvalidResponse)?;
    let owner_id = owner_field(fields, expected_owner, format)?;
    let parent_id = parent_field(fields, expected_parent)?;
    let name = string(fields, &["name"], "")?;
    if name.len() > MAX_NAME_BYTES {
        return Err(DecodeError::InvalidResponse);
    }
    let version = integer(fields, &["version"])?;
    let descendent_count = match format {
        InventoryWireFormat::Ais => None,
        InventoryWireFormat::Legacy => count(fields, &["descendents", "descendent_count"])?,
    };
    if descendent_count.is_some_and(|count| count as usize > MAX_FOLDER_CHILDREN) {
        return Err(DecodeError::TooLarge);
    }
    let preferred_folder_type = match format {
        InventoryWireFormat::Ais => integer(fields, &["type_default"]),
        InventoryWireFormat::Legacy => integer(fields, &["type_default", "type"]),
    }?;
    let mut category = Category { id, owner_id, parent_id, name, version, descendent_count, preferred_folder_type, children: None };
    let Some(embedded) = fields.get("_embedded") else { return Ok(category) };
    if matches!(format, InventoryWireFormat::Legacy) {
        return Err(DecodeError::InvalidResponse);
    }
    let embedded = map(embedded)?;
    let raw_categories = map_field(embedded, "categories")?;
    let raw_items = map_field(embedded, "items")?;
    let raw_links = map_field(embedded, "links")?;
    let direct_count = raw_categories
        .map_or(0, |values| values.len())
        .checked_add(raw_items.map_or(0, |values| values.len()))
        .and_then(|count| count.checked_add(raw_links.map_or(0, |values| values.len())))
        .ok_or(DecodeError::TooLarge)?;
    if direct_count > MAX_FOLDER_CHILDREN {
        return Err(DecodeError::TooLarge);
    }
    let mut children = Vec::with_capacity(direct_count);
    for (key, item) in raw_items.into_iter().flat_map(|items| items.iter()) {
        counts.items = counts.items.checked_add(1).ok_or(DecodeError::TooLarge)?;
        if counts.items > MAX_ITEMS {
            return Err(DecodeError::TooLarge);
        }
        let expected_id = Uuid::parse_str(key).map_err(|_| DecodeError::InvalidResponse)?;
        let item = parse_item(item, expected_owner, Some(id), false, format)?;
        if item.id != expected_id {
            return Err(DecodeError::InvalidResponse);
        }
        children.push(Child::Item(item));
    }
    for (key, link) in raw_links.into_iter().flat_map(|links| links.iter()) {
        counts.items = counts.items.checked_add(1).ok_or(DecodeError::TooLarge)?;
        if counts.items > MAX_ITEMS {
            return Err(DecodeError::TooLarge);
        }
        let expected_id = Uuid::parse_str(key).map_err(|_| DecodeError::InvalidResponse)?;
        let link = parse_item(link, expected_owner, Some(id), true, format)?;
        if link.id != expected_id {
            return Err(DecodeError::InvalidResponse);
        }
        children.push(Child::Item(link));
    }
    for (key, child) in raw_categories.into_iter().flat_map(|categories| categories.iter()) {
        let expected_id = Uuid::parse_str(key).map_err(|_| DecodeError::InvalidResponse)?;
        let child = parse_category(child, expected_owner, Some(id), depth.saturating_add(1), counts, format)?;
        if child.id != expected_id {
            return Err(DecodeError::InvalidResponse);
        }
        children.push(Child::Category(child));
    }
    if raw_categories.is_some() && raw_items.is_some() && raw_links.is_some() {
        category.descendent_count = Some(direct_count as u32);
    }
    category.children = Some(children);
    Ok(category)
}

fn parse_item(
    value: &Value,
    expected_owner: Uuid,
    expected_parent: Option<Uuid>,
    link: bool,
    format: InventoryWireFormat,
) -> Result<Item, DecodeError> {
    let fields = map(value)?;
    validate_hypermedia(fields)?;
    let id = uuid_field(fields, &["item_id"])?.filter(|id| !id.is_nil()).ok_or(DecodeError::InvalidResponse)?;
    let owner_id = owner_field(fields, expected_owner, format)?;
    let parent_id = parent_field(fields, expected_parent)?;
    let asset_id_names = match (format, link) {
        (InventoryWireFormat::Ais, true) => &["linked_id"][..],
        (InventoryWireFormat::Ais, false) => &["asset_id"][..],
        (InventoryWireFormat::Legacy, _) => &["asset_id", "linked_id"][..],
    };
    let asset_id = uuid_field(fields, asset_id_names)?;
    if matches!((format, link), (InventoryWireFormat::Ais, true)) && asset_id.is_none() {
        return Err(DecodeError::InvalidResponse);
    }
    let (asset_type_names, inventory_type_names, description_names) = match format {
        InventoryWireFormat::Ais => (&["type"][..], &["inv_type"][..], &["desc"][..]),
        InventoryWireFormat::Legacy => (&["type", "asset_type"][..], &["inv_type", "inventory_type"][..], &["desc", "description"][..]),
    };
    let asset_type = inventory_type(fields, asset_type_names, named_asset_type)?;
    let inventory_type = inventory_type(fields, inventory_type_names, named_inventory_type)?;
    let name = string(fields, &["name"], "")?;
    let description = string(fields, description_names, "")?;
    if name.len() > MAX_NAME_BYTES || description.len() > MAX_DESCRIPTION_BYTES {
        return Err(DecodeError::InvalidResponse);
    }
    let flags = unsigned(fields, &["flags"])?.unwrap_or_default();
    let created_at = match format {
        InventoryWireFormat::Ais => integer(fields, &["created_at"])?.map(i64::from).unwrap_or_default(),
        InventoryWireFormat::Legacy => timestamp(fields, &["created_at", "creation_date"])?.unwrap_or_default(),
    };
    let permissions = map_field(fields, "permissions")?;
    let sale_info = map_field(fields, "sale_info")?;
    Ok(Item {
        id,
        owner_id,
        parent_id,
        asset_id,
        asset_type,
        inventory_type,
        name,
        description,
        flags,
        created_at,
        permissions: Permissions {
            creator_id: nested_uuid(permissions, fields, &["creator_id"])?,
            owner_id: nested_uuid(permissions, fields, &["owner_id"])?,
            group_id: nested_uuid(permissions, fields, &["group_id"])?,
            last_owner_id: nested_uuid(permissions, fields, &["last_owner_id"])?,
            base_mask: nested_unsigned(permissions, fields, &["base_mask"])?,
            owner_mask: nested_unsigned(permissions, fields, &["owner_mask"])?,
            group_mask: nested_unsigned(permissions, fields, &["group_mask"])?,
            everyone_mask: nested_unsigned(permissions, fields, &["everyone_mask"])?,
            next_owner_mask: nested_unsigned(permissions, fields, &["next_owner_mask"])?,
        },
        sale_info: SaleInfo {
            sale_type: nested_integer(sale_info, fields, &["sale_type"])?,
            sale_price: nested_integer(sale_info, fields, &["sale_price"])?,
        },
    })
}

fn map(value: &Value) -> Result<&llsd::Map, DecodeError> {
    value.as_map().ok_or(DecodeError::InvalidResponse)
}

fn array(value: Option<&Value>) -> Result<&[Value], DecodeError> {
    match value {
        Some(Value::Array(values)) => Ok(values),
        _ => Err(DecodeError::InvalidResponse),
    }
}

fn metadata_ids(fields: &llsd::Map, name: &str, limit: usize) -> Result<Vec<Uuid>, DecodeError> {
    let Some(value) = fields.get(name) else { return Ok(Vec::new()) };
    let values = array(Some(value))?;
    if values.len() > limit {
        return Err(DecodeError::TooLarge);
    }
    let mut ids = Vec::with_capacity(values.len());
    let mut unique = std::collections::HashSet::with_capacity(values.len());
    for value in values {
        let id = match value {
            Value::Uuid(id) => *id,
            Value::String(value) => Uuid::parse_str(value).map_err(|_| DecodeError::InvalidResponse)?,
            _ => return Err(DecodeError::InvalidResponse),
        };
        if id.is_nil() || !unique.insert(id) {
            return Err(DecodeError::InvalidResponse);
        }
        ids.push(id);
    }
    Ok(ids)
}

fn validate_metadata_ids(fields: &llsd::Map, name: &str, limit: usize) -> Result<(), DecodeError> {
    let Some(value) = fields.get(name) else { return Ok(()) };
    let values = array(Some(value))?;
    if values.len() > limit {
        return Err(DecodeError::TooLarge);
    }
    let mut unique = std::collections::HashSet::with_capacity(values.len());
    for value in values {
        let id = match value {
            Value::Uuid(id) => *id,
            Value::String(value) => Uuid::parse_str(value).map_err(|_| DecodeError::InvalidResponse)?,
            _ => return Err(DecodeError::InvalidResponse),
        };
        if id.is_nil() || !unique.insert(id) {
            return Err(DecodeError::InvalidResponse);
        }
    }
    Ok(())
}

fn metadata_versions(fields: &llsd::Map, name: &str) -> Result<BTreeMap<Uuid, i32>, DecodeError> {
    let Some(value) = fields.get(name) else { return Ok(BTreeMap::new()) };
    let values = map(value)?;
    if values.len() > MAX_CATEGORIES {
        return Err(DecodeError::TooLarge);
    }
    values
        .iter()
        .map(|(id, version)| {
            let id = Uuid::parse_str(id).map_err(|_| DecodeError::InvalidResponse)?;
            if id.is_nil() {
                return Err(DecodeError::InvalidResponse);
            }
            let version = version.as_integer().ok_or(DecodeError::InvalidResponse)?;
            Ok((id, version))
        })
        .collect()
}

fn metadata_integer_uuid_map(fields: &llsd::Map, name: &str, limit: usize) -> Result<(), DecodeError> {
    let Some(values) = map_field(fields, name)? else { return Ok(()) };
    if values.len() > limit {
        return Err(DecodeError::TooLarge);
    }
    let mut indices = std::collections::HashSet::with_capacity(values.len());
    for (index, value) in values {
        let index = index.parse::<u32>().map_err(|_| DecodeError::InvalidResponse)?;
        match value {
            Value::Uuid(_) => {}
            Value::String(value) => {
                Uuid::parse_str(value).map_err(|_| DecodeError::InvalidResponse)?;
            }
            _ => return Err(DecodeError::InvalidResponse),
        }
        if !indices.insert(index) {
            return Err(DecodeError::InvalidResponse);
        }
    }
    Ok(())
}

fn metadata_uuid_uuid_map(fields: &llsd::Map, name: &str, limit: usize) -> Result<(), DecodeError> {
    let Some(values) = map_field(fields, name)? else { return Ok(()) };
    if values.len() > limit {
        return Err(DecodeError::TooLarge);
    }
    for (key, value) in values {
        Uuid::parse_str(key).map_err(|_| DecodeError::InvalidResponse)?;
        match value {
            Value::Uuid(_) => {}
            Value::String(value) => {
                Uuid::parse_str(value).map_err(|_| DecodeError::InvalidResponse)?;
            }
            _ => return Err(DecodeError::InvalidResponse),
        }
    }
    Ok(())
}

fn validate_update_result(fields: &llsd::Map) -> Result<(), DecodeError> {
    let Some(value) = fields.get("_update_result") else { return Ok(()) };
    let result = map(value)?;
    let Some(code) = integer(result, &["code"])? else { return Err(DecodeError::InvalidResponse) };
    if !matches!(result.get("success"), Some(Value::Boolean(_)))
        || !(0..=8).contains(&code)
        || !matches!(result.get("message"), Some(Value::String(_)))
        || (result.contains_key("fault_code") && integer(result, &["fault_code"])?.is_none())
        || (result.contains_key("server_response") && !matches!(result.get("server_response"), Some(Value::String(_))))
    {
        return Err(DecodeError::InvalidResponse);
    }
    Ok(())
}

fn optional_array<'a>(fields: &'a llsd::Map, name: &str, descendent_count: u32) -> Result<&'a [Value], DecodeError> {
    match fields.get(name) {
        Some(Value::Array(values)) => Ok(values),
        None if descendent_count == 0 => Ok(&[]),
        None => Err(DecodeError::InvalidResponse),
        Some(_) => Err(DecodeError::InvalidResponse),
    }
}

fn uuid_field(fields: &llsd::Map, names: &[&str]) -> Result<Option<Uuid>, DecodeError> {
    let mut found = None;
    for name in names {
        if let Some(value) = fields.get(*name) {
            let id = match value {
                Value::Uuid(id) => *id,
                Value::String(value) => Uuid::parse_str(value).map_err(|_| DecodeError::InvalidResponse)?,
                _ => return Err(DecodeError::InvalidResponse),
            };
            if found.is_some_and(|previous| previous != id) {
                return Err(DecodeError::InvalidResponse);
            }
            found = Some(id);
        }
    }
    Ok(found)
}

fn owner_field(fields: &llsd::Map, expected: Uuid, format: InventoryWireFormat) -> Result<Option<Uuid>, DecodeError> {
    let names = match format {
        InventoryWireFormat::Ais => &["agent_id"][..],
        InventoryWireFormat::Legacy => &["owner_id", "agent_id"][..],
    };
    let owner = uuid_field(fields, names)?;
    if owner.is_some_and(|owner| owner != expected) {
        return Err(DecodeError::InvalidResponse);
    }
    Ok(owner)
}

fn parent_field(fields: &llsd::Map, expected: Option<Uuid>) -> Result<Option<Uuid>, DecodeError> {
    let parent = uuid_field(fields, &["parent_id"])?;
    if expected.is_some_and(|expected| parent.is_some_and(|parent| parent != expected)) {
        return Err(DecodeError::InvalidResponse);
    }
    Ok(parent)
}

fn integer(fields: &llsd::Map, names: &[&str]) -> Result<Option<i32>, DecodeError> {
    let Some(value) = get(fields, names) else { return Ok(None) };
    value.as_integer().map(Some).ok_or(DecodeError::InvalidResponse)
}

fn count(fields: &llsd::Map, names: &[&str]) -> Result<Option<u32>, DecodeError> {
    let Some(value) = unsigned(fields, names)? else { return Ok(None) };
    if value as usize > MAX_FOLDER_CHILDREN {
        return Err(DecodeError::TooLarge);
    }
    Ok(Some(value))
}

fn unsigned(fields: &llsd::Map, names: &[&str]) -> Result<Option<u32>, DecodeError> {
    let Some(value) = get(fields, names) else { return Ok(None) };
    let value = value.as_integer().filter(|value| *value >= 0).ok_or(DecodeError::InvalidResponse)?;
    Ok(Some(u32::try_from(value).map_err(|_| DecodeError::InvalidResponse)?))
}

fn timestamp(fields: &llsd::Map, names: &[&str]) -> Result<Option<i64>, DecodeError> {
    let Some(value) = get(fields, names) else { return Ok(None) };
    match value {
        Value::Integer(value) => Ok(Some(i64::from(*value))),
        Value::Date(value) if value.unix_seconds().is_finite() => {
            let seconds = value.unix_seconds();
            (seconds >= i64::MIN as f64 && seconds <= i64::MAX as f64)
                .then_some(seconds.trunc() as i64)
                .map(Some)
                .ok_or(DecodeError::InvalidResponse)
        }
        _ => Err(DecodeError::InvalidResponse),
    }
}

fn string(fields: &llsd::Map, names: &[&str], default: &str) -> Result<String, DecodeError> {
    match get(fields, names) {
        Some(Value::String(value)) => Ok(value.clone()),
        None => Ok(default.to_owned()),
        _ => Err(DecodeError::InvalidResponse),
    }
}

fn inventory_type(fields: &llsd::Map, names: &[&str], named: fn(&str) -> i32) -> Result<Option<i32>, DecodeError> {
    match get(fields, names) {
        None => Ok(None),
        Some(Value::Integer(value)) => Ok(Some(*value)),
        Some(Value::String(value)) => Ok(Some(named(value))),
        Some(_) => Ok(Some(255)),
    }
}

fn map_field<'a>(fields: &'a llsd::Map, name: &str) -> Result<Option<&'a llsd::Map>, DecodeError> {
    match fields.get(name) {
        None => Ok(None),
        Some(Value::Map(fields)) => Ok(Some(fields)),
        Some(_) => Err(DecodeError::InvalidResponse),
    }
}

fn nested_uuid(nested: Option<&llsd::Map>, fields: &llsd::Map, names: &[&str]) -> Result<Option<Uuid>, DecodeError> {
    uuid_field(preferred_fields(nested, fields, names), names)
}

fn nested_unsigned(nested: Option<&llsd::Map>, fields: &llsd::Map, names: &[&str]) -> Result<Option<u32>, DecodeError> {
    unsigned(preferred_fields(nested, fields, names), names)
}

fn nested_integer(nested: Option<&llsd::Map>, fields: &llsd::Map, names: &[&str]) -> Result<Option<i32>, DecodeError> {
    integer(preferred_fields(nested, fields, names), names)
}

fn preferred_fields<'a>(nested: Option<&'a llsd::Map>, fields: &'a llsd::Map, names: &[&str]) -> &'a llsd::Map {
    if let Some(nested) = nested
        && get(nested, names).is_some()
    {
        nested
    } else {
        fields
    }
}

fn get<'a>(fields: &'a llsd::Map, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| fields.get(*name))
}

fn validate_hypermedia(fields: &llsd::Map) -> Result<(), DecodeError> {
    if let Some(base_uri) = fields.get("_base_uri")
        && string_or_uri(base_uri).is_none_or(|uri| uri.len() > MAX_HYPERMEDIA_URI_BYTES)
    {
        return Err(DecodeError::InvalidResponse);
    }
    if let Some(links) = fields.get("_links") {
        let links = map(links)?;
        for relation in links.values() {
            let relation = map(relation)?;
            let href = relation.get("href").and_then(string_or_uri).ok_or(DecodeError::InvalidResponse)?;
            if href.len() > MAX_HYPERMEDIA_URI_BYTES {
                return Err(DecodeError::InvalidResponse);
            }
        }
    }
    Ok(())
}

fn string_or_uri(value: &Value) -> Option<&str> {
    match value {
        Value::String(value) => Some(value),
        Value::Uri(value) => Some(value.as_str()),
        _ => None,
    }
}

fn named_asset_type(value: &str) -> i32 {
    match value {
        "texture" => 0,
        "sound" => 1,
        "callcard" | "callingcard" => 2,
        "landmark" => 3,
        "script" => 4,
        "clothing" => 5,
        "object" => 6,
        "notecard" => 7,
        "category" => 8,
        "lsltext" => 10,
        "lslbyte" => 11,
        "txtr_tga" => 12,
        "bodypart" => 13,
        "snd_wav" => 17,
        "img_tga" => 18,
        "jpeg" => 19,
        "animatn" => 20,
        "gesture" => 21,
        "simstate" => 22,
        "link" => 24,
        "link_f" => 25,
        "widget" => 40,
        "person" => 45,
        "mesh" => 49,
        "settings" => 56,
        "material" => 57,
        "gltf" => 58,
        "glbin" | "gltf_binary" => 59,
        "invalid" => 255,
        "-1" => -1,
        _ => 255,
    }
}

fn named_inventory_type(value: &str) -> i32 {
    match value {
        "texture" => 0,
        "sound" => 1,
        "callcard" | "callingcard" => 2,
        "landmark" => 3,
        "object" => 6,
        "notecard" => 7,
        "category" => 8,
        "root" | "root_category" => 9,
        "script" | "lsl" => 10,
        "snapshot" => 15,
        "attach" | "attachment" => 17,
        "wearable" => 18,
        "animation" => 19,
        "gesture" => 20,
        "mesh" => 22,
        "widget" => 23,
        "person" => 24,
        "settings" => 25,
        "material" => 26,
        "gltf" => 27,
        "glbin" | "gltf_binary" => 28,
        "-1" => -1,
        _ => 255,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use llsd::{Map, Value, xml};

    use super::*;

    fn map_value(fields: impl IntoIterator<Item = (String, Value)>) -> Value {
        Value::Map(fields.into_iter().collect::<BTreeMap<_, _>>())
    }

    fn category(id: Uuid, parent: Option<Uuid>, descendent_count: usize) -> Value {
        let mut fields = Map::from([
            ("category_id".into(), Value::Uuid(id)),
            ("name".into(), Value::String("folder".into())),
            ("version".into(), Value::Integer(4)),
            ("descendents".into(), Value::Integer(descendent_count as i32)),
        ]);
        if let Some(parent) = parent {
            fields.insert("parent_id".into(), Value::Uuid(parent));
        }
        Value::Map(fields)
    }

    fn item(id: Uuid, parent: Uuid, owner: Uuid) -> Value {
        Value::Map(Map::from([
            ("item_id".into(), Value::Uuid(id)),
            ("agent_id".into(), Value::Uuid(owner)),
            ("created_at".into(), Value::Integer(1_700_000_000)),
            ("parent_id".into(), Value::Uuid(parent)),
            ("name".into(), Value::String("note".into())),
            ("desc".into(), Value::String("text".into())),
            ("type".into(), Value::Integer(7)),
            ("inv_type".into(), Value::Integer(7)),
        ]))
    }

    fn legacy_item(id: Uuid, parent: Uuid, owner: Uuid) -> Value {
        let Value::Map(mut fields) = item(id, parent, owner) else { unreachable!() };
        fields.remove("agent_id");
        fields.insert("owner_id".into(), Value::Uuid(owner));
        Value::Map(fields)
    }

    fn item_with_types(id: Uuid, parent: Uuid, owner: Uuid, asset_type: Value, inventory_type: Value) -> Value {
        Value::Map(Map::from([
            ("item_id".into(), Value::Uuid(id)),
            ("agent_id".into(), Value::Uuid(owner)),
            ("created_at".into(), Value::Integer(1_700_000_000)),
            ("parent_id".into(), Value::Uuid(parent)),
            ("name".into(), Value::String("item".into())),
            ("type".into(), asset_type),
            ("inv_type".into(), inventory_type),
        ]))
    }

    fn encode(value: &Value) -> Vec<u8> {
        xml::to_vec(value).expect("LLSD fixture encodes")
    }

    fn keyed(values: Vec<Value>, id_field: &str) -> Value {
        Value::Map(
            values
                .into_iter()
                .map(|value| {
                    let fields = value.as_map().expect("keyed child is a map");
                    let id = uuid_field(fields, &[id_field]).expect("valid child ID field").expect("child ID is present");
                    (id.to_string(), value)
                })
                .collect(),
        )
    }

    #[test]
    fn ais_edit_response_decodes_all_change_metadata() {
        let ids = (1..=16).map(Uuid::from_u128).collect::<Vec<_>>();
        let metadata = Value::Map(Map::from([
            ("_created_categories".into(), Value::Array(vec![Value::Uuid(ids[0])])),
            ("_created_items".into(), Value::Array(vec![Value::Uuid(ids[1])])),
            ("_updated_items".into(), Value::Array(vec![Value::Uuid(ids[2])])),
            ("_updated_categories".into(), Value::Array(vec![Value::Uuid(ids[3])])),
            ("_removed_items".into(), Value::Array(vec![Value::Uuid(ids[4])])),
            ("_categories_removed".into(), Value::Array(vec![Value::Uuid(ids[5])])),
            ("_category_items_removed".into(), Value::Array(vec![Value::Uuid(ids[6])])),
            ("_broken_links_removed".into(), Value::Array(vec![Value::Uuid(ids[7])])),
            ("_updated_category_versions".into(), Value::Map(Map::from([(ids[8].to_string(), Value::Integer(12))]))),
            ("_moved_items".into(), Value::Array(vec![Value::Uuid(ids[9])])),
            ("_active_gestures_removed".into(), Value::Array(vec![Value::Uuid(ids[10])])),
            ("_attachments_removed".into(), Value::Map(Map::from([("0".into(), Value::Uuid(ids[11]))]))),
            ("_wearables_removed".into(), Value::Map(Map::from([("8".into(), Value::Uuid(ids[12]))]))),
            ("_total_items_removed".into(), Value::Integer(4)),
            ("_top_category_id".into(), Value::Uuid(ids[13])),
            ("_linked_ids".into(), Value::Map(Map::from([(ids[14].to_string(), Value::Uuid(ids[15]))]))),
            (
                "_update_result".into(),
                Value::Map(Map::from([
                    ("success".into(), Value::Boolean(true)),
                    ("code".into(), Value::Integer(0)),
                    ("message".into(), Value::String("ok".into())),
                ])),
            ),
        ]));

        let response = decode_ais_edit_response(&encode(&metadata), Uuid::from_u128(99)).expect("valid AIS metadata");

        assert_eq!(response.created_categories, vec![ids[0]]);
        assert_eq!(response.created_items, vec![ids[1]]);
        assert_eq!(response.updated_items, vec![ids[2]]);
        assert_eq!(response.updated_categories, vec![ids[3]]);
        assert_eq!(response.removed_items, vec![ids[4]]);
        assert_eq!(response.categories_removed, vec![ids[5]]);
        assert_eq!(response.category_items_removed, vec![ids[6]]);
        assert_eq!(response.broken_links_removed, vec![ids[7]]);
        assert_eq!(response.updated_category_versions, BTreeMap::from([(ids[8], 12)]));
    }

    #[test]
    fn ais_edit_response_rejects_mistyped_metadata() {
        let cases = [
            ("_moved_items", Value::Map(Map::new())),
            ("_updated_category_versions", Value::Array(Vec::new())),
            ("_attachments_removed", Value::Array(Vec::new())),
            ("_wearables_removed", Value::Array(Vec::new())),
            ("_linked_ids", Value::Array(Vec::new())),
            ("_total_items_removed", Value::Array(Vec::new())),
            ("_update_result", Value::Array(Vec::new())),
        ];
        for (name, value) in cases {
            let response = Value::Map(Map::from([(name.into(), value)]));
            assert_eq!(
                decode_ais_edit_response(&encode(&response), Uuid::from_u128(99)),
                Err(DecodeError::InvalidResponse),
                "{name} must use its AIS schema shape"
            );
        }
    }

    #[test]
    fn ais_recursive_maps_decode_real_response() {
        let owner = Uuid::from_u128(0x99);
        let root = Uuid::from_u128(1);
        let decoded = decode_ais_children(include_bytes!("../tests/fixtures/ais_recursive_children.llsd.xml"), root, owner)
            .expect("official AIS response shape decodes");

        assert_eq!(decoded.owner_id, Some(owner));
        assert_eq!(decoded.descendent_count, Some(1));
        let root_children = decoded.children.expect("root children are present");
        assert_eq!(root_children.len(), 1);
        let Child::Category(folder) = &root_children[0] else { panic!("expected folder") };
        assert_eq!(folder.id, Uuid::from_u128(2));
        assert_eq!(folder.descendent_count, Some(3));
        let folder_children = folder.children.as_ref().expect("folder children are present");
        assert_eq!(folder_children.len(), 3);

        let Child::Item(note) = &folder_children[0] else { panic!("expected note item") };
        assert_eq!(note.id, Uuid::from_u128(5));
        assert_eq!(note.owner_id, Some(owner));
        assert_eq!(note.created_at, 1_700_000_000);
        assert_eq!(note.permissions.owner_id, Some(Uuid::from_u128(19)));
        assert_eq!(note.permissions.creator_id, Some(Uuid::from_u128(17)));
        assert_eq!(note.sale_info.sale_type, Some(0));
        assert_eq!(note.sale_info.sale_price, Some(0));

        let Child::Item(link) = &folder_children[1] else { panic!("expected link") };
        assert_eq!(link.id, Uuid::from_u128(6));
        assert_eq!(link.asset_id, Some(Uuid::from_u128(5)));
        assert_eq!(link.asset_type, Some(24));

        let Child::Category(nested) = &folder_children[2] else { panic!("expected nested folder") };
        assert_eq!(nested.id, Uuid::from_u128(3));
        assert_eq!(nested.descendent_count, Some(1));
        let nested_children = nested.children.as_ref().expect("nested folder children are present");
        assert_eq!(nested_children.len(), 1);
        let Child::Item(nested_item) = &nested_children[0] else { panic!("expected nested item") };
        assert_eq!(nested_item.id, Uuid::from_u128(7));
    }

    #[test]
    fn embedded_map_keys_match_child_ids() {
        let owner = Uuid::from_u128(1);
        let root = Uuid::from_u128(2);
        let child_id = Uuid::from_u128(3);
        let wrong_key = Uuid::from_u128(4);
        let cases = [
            (
                "categories",
                Value::Map(Map::from([
                    ("agent_id".into(), Value::Uuid(owner)),
                    ("category_id".into(), Value::Uuid(child_id)),
                    ("name".into(), Value::String("folder".into())),
                    ("parent_id".into(), Value::Uuid(root)),
                    ("type_default".into(), Value::Integer(-1)),
                    ("version".into(), Value::Integer(1)),
                ])),
            ),
            ("items", item(child_id, root, owner)),
            (
                "links",
                Value::Map(Map::from([
                    ("agent_id".into(), Value::Uuid(owner)),
                    ("item_id".into(), Value::Uuid(child_id)),
                    ("linked_id".into(), Value::Uuid(Uuid::from_u128(5))),
                    ("name".into(), Value::String("link".into())),
                    ("parent_id".into(), Value::Uuid(root)),
                    ("type".into(), Value::Integer(24)),
                ])),
            ),
        ];

        for (name, child) in cases {
            let mut root_fields = Map::from([
                ("agent_id".into(), Value::Uuid(owner)),
                ("category_id".into(), Value::Uuid(root)),
                ("name".into(), Value::String("root".into())),
                ("parent_id".into(), Value::Uuid(Uuid::from_u128(6))),
                ("type_default".into(), Value::Integer(-1)),
                ("version".into(), Value::Integer(1)),
            ]);
            root_fields.insert("_embedded".into(), map_value([(name.into(), Value::Map(Map::from([(wrong_key.to_string(), child)])))]));
            assert_eq!(
                decode_ais_children(&encode(&Value::Map(root_fields)), root, owner),
                Err(DecodeError::InvalidResponse),
                "{name} map key must match the child's ID"
            );
        }
    }

    #[test]
    fn embedded_arrays_are_rejected() {
        let owner = Uuid::from_u128(1);
        let root = Uuid::from_u128(2);
        let mut root_fields = match category(root, None, 0) {
            Value::Map(fields) => fields,
            _ => unreachable!(),
        };
        root_fields.insert("_embedded".into(), map_value([("items".into(), Value::Array(Vec::new()))]));
        assert_eq!(decode_ais_children(&encode(&Value::Map(root_fields)), root, owner), Err(DecodeError::InvalidResponse));
    }

    #[test]
    fn partial_embedded_children_do_not_set_count() {
        let owner = Uuid::from_u128(1);
        let root = Uuid::from_u128(2);
        let mut root_fields = match category(root, None, 0) {
            Value::Map(fields) => fields,
            _ => unreachable!(),
        };
        root_fields.insert("agent_id".into(), Value::Uuid(owner));
        root_fields
            .insert("_embedded".into(), map_value([("items".into(), keyed(vec![item(Uuid::from_u128(3), root, owner)], "item_id"))]));

        let decoded =
            decode_ais_children(&encode(&Value::Map(root_fields)), root, owner).expect("partial children are valid but incomplete");

        assert_eq!(decoded.descendent_count, None);
    }

    #[test]
    fn ais_created_at_requires_unix_integer_shape() {
        let owner = Uuid::from_u128(1);
        let root = Uuid::from_u128(2);
        let mut root_fields = match category(root, None, 1) {
            Value::Map(fields) => fields,
            _ => unreachable!(),
        };
        root_fields.insert("agent_id".into(), Value::Uuid(owner));
        let Value::Map(mut item_fields) = item(Uuid::from_u128(3), root, owner) else { unreachable!() };
        item_fields.insert("created_at".into(), Value::Date(llsd::Date::UNIX_EPOCH));
        root_fields.insert(
            "_embedded".into(),
            map_value([
                ("categories".into(), Value::Map(Map::new())),
                ("items".into(), keyed(vec![Value::Map(item_fields)], "item_id")),
                ("links".into(), Value::Map(Map::new())),
            ]),
        );
        assert_eq!(decode_ais_children(&encode(&Value::Map(root_fields)), root, owner), Err(DecodeError::InvalidResponse));
    }

    #[test]
    fn official_type_names_decode_without_rejecting_folder() {
        let owner = Uuid::from_u128(1);
        let folder = Uuid::from_u128(2);
        let asset_types = [
            ("texture", 0),
            ("sound", 1),
            ("callcard", 2),
            ("landmark", 3),
            ("script", 4),
            ("clothing", 5),
            ("object", 6),
            ("notecard", 7),
            ("category", 8),
            ("lsltext", 10),
            ("lslbyte", 11),
            ("txtr_tga", 12),
            ("bodypart", 13),
            ("snd_wav", 17),
            ("img_tga", 18),
            ("jpeg", 19),
            ("animatn", 20),
            ("gesture", 21),
            ("simstate", 22),
            ("link", 24),
            ("link_f", 25),
            ("widget", 40),
            ("person", 45),
            ("mesh", 49),
            ("settings", 56),
            ("material", 57),
            ("gltf", 58),
            ("glbin", 59),
            ("invalid", 255),
            ("llassettype_bad_lookup", 255),
            ("-1", -1),
        ];
        let inventory_types = [
            ("texture", 0),
            ("sound", 1),
            ("callcard", 2),
            ("landmark", 3),
            ("object", 6),
            ("notecard", 7),
            ("category", 8),
            ("root", 9),
            ("script", 10),
            ("snapshot", 15),
            ("attach", 17),
            ("wearable", 18),
            ("animation", 19),
            ("gesture", 20),
            ("mesh", 22),
            ("widget", 23),
            ("person", 24),
            ("settings", 25),
            ("material", 26),
            ("gltf", 27),
            ("glbin", 28),
            ("", 255),
        ];
        let mut expected = Vec::new();
        let mut items = Vec::new();
        for (index, (name, raw)) in asset_types.into_iter().enumerate() {
            expected.push((raw, 10));
            items.push(item_with_types(
                Uuid::from_u128(100 + index as u128),
                folder,
                owner,
                Value::String(name.into()),
                Value::String("script".into()),
            ));
        }
        for (index, (name, raw)) in inventory_types.into_iter().enumerate() {
            expected.push((0, raw));
            items.push(item_with_types(
                Uuid::from_u128(200 + index as u128),
                folder,
                owner,
                Value::String("texture".into()),
                Value::String(name.into()),
            ));
        }
        items.push(item_with_types(
            Uuid::from_u128(300),
            folder,
            owner,
            Value::String("future_asset".into()),
            Value::String("future_inventory_type".into()),
        ));
        expected.push((255, 255));
        items.push(item_with_types(Uuid::from_u128(301), folder, owner, Value::Integer(7), Value::Boolean(true)));
        expected.push((7, 255));

        let mut fields = match category(folder, None, 0) {
            Value::Map(fields) => fields,
            _ => unreachable!(),
        };
        fields.insert("descendents".into(), Value::Integer(items.len() as i32));
        fields.insert(
            "_embedded".into(),
            map_value([
                ("categories".into(), Value::Map(Map::new())),
                ("items".into(), keyed(items, "item_id")),
                ("links".into(), Value::Map(Map::new())),
            ]),
        );
        let decoded = decode_ais_children(&encode(&Value::Map(fields)), folder, owner).expect("one unknown field does not reject a folder");
        let children = decoded.children.expect("folder items are present");
        assert_eq!(children.len(), expected.len());
        for (child, (asset_type, inventory_type)) in children.iter().zip(expected) {
            let Child::Item(item) = child else { panic!("expected item") };
            assert_eq!(item.asset_type, Some(asset_type));
            assert_eq!(item.inventory_type, Some(inventory_type));
        }
    }

    #[test]
    fn legacy_folder_batch_is_typed() {
        let owner = Uuid::from_u128(1);
        let folder = Uuid::from_u128(2);
        let child = Uuid::from_u128(3);
        let response = Value::Map(Map::from([(
            "folders".into(),
            Value::Array(vec![Value::Map(Map::from([
                ("folder_id".into(), Value::Uuid(folder)),
                ("owner_id".into(), Value::Uuid(owner)),
                ("version".into(), Value::Integer(8)),
                ("descendents".into(), Value::Integer(1)),
                ("categories".into(), Value::Array(vec![category(child, Some(folder), 0)])),
                ("items".into(), Value::Array(Vec::new())),
            ]))]),
        )]));
        let decoded = decode_fetch_inventory_descendents2(&encode(&response), &[RequestedFolder { id: folder, owner_id: owner }])
            .expect("valid legacy response");
        assert_eq!(decoded[0].version, 8);
        assert_eq!(decoded[0].categories[0].id, child);
    }

    #[test]
    fn folder_limit_is_typed() {
        let owner = Uuid::from_u128(1);
        let folder = Uuid::from_u128(2);
        let mut value = match category(folder, None, 0) {
            Value::Map(fields) => fields,
            _ => unreachable!(),
        };
        value.insert("agent_id".into(), Value::Uuid(owner));
        let items = (0..=MAX_FOLDER_CHILDREN)
            .map(|index| (Uuid::from_u128(index as u128 + 100).to_string(), Value::Map(Map::new())))
            .collect::<Map>();
        value.insert(
            "_embedded".into(),
            map_value([
                ("categories".into(), Value::Map(Map::new())),
                ("items".into(), Value::Map(items)),
                ("links".into(), Value::Map(Map::new())),
            ]),
        );
        assert_eq!(decode_ais_children(&encode(&Value::Map(value)), folder, owner), Err(DecodeError::TooLarge));
    }

    #[test]
    fn wrong_owner_is_rejected() {
        let owner = Uuid::from_u128(1);
        let root = Uuid::from_u128(2);
        let mut value = match category(root, None, 0) {
            Value::Map(fields) => fields,
            _ => unreachable!(),
        };
        value.insert("agent_id".into(), Value::Uuid(Uuid::from_u128(3)));
        assert_eq!(decode_ais_children(&encode(&Value::Map(value)), root, owner), Err(DecodeError::InvalidResponse));
    }

    #[test]
    fn ais_item_endpoint_decodes_parent_and_identity() {
        let owner = Uuid::from_u128(1);
        let parent = Uuid::from_u128(2);
        let item_id = Uuid::from_u128(3);
        let decoded = decode_ais_item(&encode(&item(item_id, parent, owner)), item_id, owner).expect("valid AIS item");
        assert_eq!(decoded.id, item_id);
        assert_eq!(decoded.parent_id, Some(parent));
        assert_eq!(decoded.owner_id, Some(owner));
    }

    #[test]
    fn legacy_items_response_rejects_unrequested_id() {
        let owner = Uuid::from_u128(1);
        let parent = Uuid::from_u128(2);
        let item_id = Uuid::from_u128(3);
        let response = Value::Map(Map::from([("items".into(), Value::Array(vec![legacy_item(item_id, parent, owner)]))]));
        assert_eq!(decode_fetch_inventory2(&encode(&response), owner, &[Uuid::from_u128(4)]), Err(DecodeError::InvalidResponse));
    }
}
