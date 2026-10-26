use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
    sync::Arc,
};

use ::inventory::{
    AssetType, Category, CategoryKey, ChangeSet, FolderType, Inventory, InventoryDelta, InventoryError, InventoryOwner, InventorySnapshot,
    InventoryType, Item, ItemKey, PermissionMask, Permissions, SaleInfo, SaleType,
};
use llsd::{Map, Value};
use protocol::{
    lludp::Message,
    messages::{
        BulkUpdateInventory, InventoryFolderBlock, InventoryItemBlock, MoveInventoryItem, MoveInventoryItemBlock, RemoveInventoryFolder,
        RemoveInventoryItem, RemoveInventoryObjects, SaveAssetIntoInventory, UpdateCreateInventoryItem,
    },
};
use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
use uuid::Uuid;

const MAX_BLOCKS: usize = u8::MAX as usize;
const MAX_STRING_BYTES: usize = u8::MAX as usize - 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct InventoryIntakeOutcome {
    pub(super) changes: ChangeSet,
    pub(super) rejected_blocks: usize,
}

#[derive(Debug)]
pub(super) enum InventoryIntakeError {
    MalformedEvent,
    Model(InventoryError),
}

impl fmt::Display for InventoryIntakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedEvent => formatter.write_str("malformed EventQueue inventory message"),
            Self::Model(error) => write!(formatter, "{error}"),
        }
    }
}

impl Error for InventoryIntakeError {}

impl From<InventoryError> for InventoryIntakeError {
    fn from(error: InventoryError) -> Self {
        Self::Model(error)
    }
}

pub(super) fn apply_udp(
    inventory: &mut Inventory,
    expected_agent: Uuid,
    message: &Message,
) -> Result<Option<InventoryIntakeOutcome>, InventoryIntakeError> {
    apply_message(inventory, expected_agent, message)
}

pub(super) fn apply_event_queue(
    inventory: &mut Inventory,
    expected_agent: Uuid,
    name: &str,
    body: &Value,
) -> Result<Option<InventoryIntakeOutcome>, InventoryIntakeError> {
    let Some(message) = parse_event_message(name, body, expected_agent)? else { return Ok(None) };
    apply_message(inventory, expected_agent, &message)
}

fn apply_message(
    inventory: &mut Inventory,
    expected_agent: Uuid,
    message: &Message,
) -> Result<Option<InventoryIntakeOutcome>, InventoryIntakeError> {
    let Some((agent_id, invalid_blocks)) = message_agent(message) else { return Ok(None) };
    if agent_id != expected_agent {
        return Ok(None);
    }
    let snapshot = inventory.snapshot();
    let Some((delta, rejected_blocks)) = delta_from_message(message, InventoryOwner::new(expected_agent), &snapshot) else {
        return Ok(None);
    };
    let changes = inventory.apply_delta(delta)?;
    Ok(Some(InventoryIntakeOutcome { changes, rejected_blocks: rejected_blocks.saturating_add(invalid_blocks) }))
}

fn message_agent(message: &Message) -> Option<(Uuid, usize)> {
    match message {
        Message::UpdateCreateInventoryItem(message) => Some((message.agent_id, message.invalid_blocks)),
        Message::RemoveInventoryItem(message) => Some((message.agent_id, message.invalid_blocks)),
        Message::RemoveInventoryFolder(message) => Some((message.agent_id, message.invalid_blocks)),
        Message::RemoveInventoryObjects(message) => Some((message.agent_id, message.invalid_blocks)),
        Message::SaveAssetIntoInventory(message) => Some((message.agent_id, 0)),
        Message::BulkUpdateInventory(message) => Some((message.agent_id, message.invalid_blocks)),
        Message::MoveInventoryItem(message) => Some((message.agent_id, message.invalid_blocks)),
        _ => None,
    }
}

fn delta_from_message(message: &Message, owner: InventoryOwner, snapshot: &InventorySnapshot) -> Option<(InventoryDelta, usize)> {
    let mut delta = InventoryDelta::default();
    let mut rejected_blocks = 0usize;
    match message {
        Message::UpdateCreateInventoryItem(message) => {
            add_item_updates(snapshot, owner, &message.inventory_data, &mut delta, &mut rejected_blocks);
        }
        Message::RemoveInventoryItem(message) => {
            delta.removed_items.extend(message.inventory_data.iter().map(|id| ItemKey::new(owner, ItemId::new(*id))));
        }
        Message::RemoveInventoryFolder(message) => {
            add_folder_removals(snapshot, owner, &message.folder_data, &mut delta, &mut rejected_blocks);
        }
        Message::RemoveInventoryObjects(message) => {
            add_folder_removals(snapshot, owner, &message.folder_data, &mut delta, &mut rejected_blocks);
            delta.removed_items.extend(message.item_data.iter().map(|id| ItemKey::new(owner, ItemId::new(*id))));
        }
        Message::SaveAssetIntoInventory(_) => {}
        Message::BulkUpdateInventory(message) => {
            let accepted_categories = add_category_updates(snapshot, owner, &message.folder_data, &mut delta, &mut rejected_blocks);
            let available_categories = snapshot.categories().map(|category| category.key).chain(accepted_categories).collect();
            add_item_updates_for_categories(snapshot, owner, &available_categories, &message.item_data, &mut delta, &mut rejected_blocks);
        }
        Message::MoveInventoryItem(message) => {
            add_item_moves(snapshot, owner, &message.inventory_data, &mut delta, &mut rejected_blocks);
        }
        _ => return None,
    }
    Some((delta, rejected_blocks))
}

fn add_category_updates(
    snapshot: &InventorySnapshot,
    owner: InventoryOwner,
    blocks: &[InventoryFolderBlock],
    delta: &mut InventoryDelta,
    rejected_blocks: &mut usize,
) -> HashSet<CategoryKey> {
    let mut candidates = HashMap::with_capacity(blocks.len());
    let mut ordered_keys = Vec::with_capacity(blocks.len());
    for block in blocks {
        let key = CategoryKey::new(owner, FolderId::new(block.folder_id));
        let parent = CategoryKey::new(owner, FolderId::new(block.parent_id));
        let category = Category {
            key,
            parent: Some(parent),
            preferred_folder_type: FolderType::from_raw(i32::from(block.folder_type)),
            name: Arc::from(block.name.as_str()),
            server_version: ::inventory::ServerVersion::UNKNOWN,
            server_descendent_count: None,
        };
        if snapshot.roots().contains(key) || candidates.contains_key(&key) {
            *rejected_blocks = rejected_blocks.saturating_add(1);
            continue;
        }
        candidates.insert(key, category);
        ordered_keys.push(key);
    }
    let mut accepted = HashSet::with_capacity(ordered_keys.len());
    for key in ordered_keys.iter().copied() {
        if category_chain_is_valid(key, &candidates, snapshot) {
            accepted.insert(key);
        } else {
            *rejected_blocks = rejected_blocks.saturating_add(1);
        }
    }
    for key in ordered_keys {
        if !accepted.contains(&key) {
            continue;
        }
        let Some(category) = candidates.remove(&key) else { continue };
        if snapshot.category(key).is_some() {
            delta.updated_categories.push(category);
        } else {
            delta.created_categories.push(category);
        }
    }
    accepted
}

fn category_chain_is_valid(start: CategoryKey, candidates: &HashMap<CategoryKey, Category>, snapshot: &InventorySnapshot) -> bool {
    let mut visited = HashSet::new();
    let mut current = start;
    loop {
        if !visited.insert(current) {
            return false;
        }
        let parent = candidates
            .get(&current)
            .map(|category| category.parent)
            .unwrap_or_else(|| snapshot.category(current).and_then(|category| category.parent));
        let Some(parent) = parent else { return snapshot.roots().contains(current) };
        if candidates.get(&current).is_none() && snapshot.category(current).is_none() {
            return false;
        }
        if candidates.get(&parent).is_none() && snapshot.category(parent).is_none() {
            return false;
        }
        current = parent;
    }
}

fn add_item_updates(
    snapshot: &InventorySnapshot,
    owner: InventoryOwner,
    blocks: &[InventoryItemBlock],
    delta: &mut InventoryDelta,
    rejected_blocks: &mut usize,
) {
    let available_categories = snapshot.categories().map(|category| category.key).collect();
    add_item_updates_for_categories(snapshot, owner, &available_categories, blocks, delta, rejected_blocks);
}

fn add_item_updates_for_categories(
    snapshot: &InventorySnapshot,
    owner: InventoryOwner,
    available_categories: &HashSet<CategoryKey>,
    blocks: &[InventoryItemBlock],
    delta: &mut InventoryDelta,
    rejected_blocks: &mut usize,
) {
    for block in blocks {
        let parent = CategoryKey::new(owner, FolderId::new(block.folder_id));
        if !available_categories.contains(&parent) {
            *rejected_blocks = rejected_blocks.saturating_add(1);
            continue;
        }
        let mut item = item_from_block(owner, block);
        if snapshot.item(item.key).is_some() {
            if let Some(previous) = snapshot.item(item.key) {
                item.permissions.last_owner = previous.permissions.last_owner;
            }
            delta.updated_items.push(item);
        } else {
            delta.created_items.push(item);
        }
    }
}

fn item_from_block(owner: InventoryOwner, block: &InventoryItemBlock) -> Item {
    Item {
        key: ItemKey::new(owner, ItemId::new(block.item_id)),
        parent: CategoryKey::new(owner, FolderId::new(block.folder_id)),
        asset_id: AssetId::new(block.asset_id),
        asset_type: AssetType::from_raw(i32::from(block.asset_type)),
        inventory_type: InventoryType::from_raw(i32::from(block.inventory_type)),
        flags: block.flags,
        name: Arc::from(block.name.as_str()),
        description: Arc::from(block.description.as_str()),
        created_at: i64::from(block.creation_date),
        permissions: Permissions {
            creator: AvatarId::new(block.creator_id),
            owner: AvatarId::new(block.owner_id),
            group: GroupId::new(block.group_id),
            last_owner: AvatarId::new(Uuid::nil()),
            base: PermissionMask::new(block.base_mask),
            owner_mask: PermissionMask::new(block.owner_mask),
            group_mask: PermissionMask::new(block.group_mask),
            everyone: PermissionMask::new(block.everyone_mask),
            next_owner: PermissionMask::new(block.next_owner_mask),
        },
        sale: SaleInfo { sale_type: SaleType::from_raw(i32::from(block.sale_type)), price: block.sale_price },
    }
}

fn add_folder_removals(
    snapshot: &InventorySnapshot,
    owner: InventoryOwner,
    folders: &[Uuid],
    delta: &mut InventoryDelta,
    rejected_blocks: &mut usize,
) {
    for id in folders {
        let key = CategoryKey::new(owner, FolderId::new(*id));
        if snapshot.roots().contains(key) {
            *rejected_blocks = rejected_blocks.saturating_add(1);
        } else {
            delta.removed_categories.push(key);
        }
    }
}

fn add_item_moves(
    snapshot: &InventorySnapshot,
    owner: InventoryOwner,
    blocks: &[MoveInventoryItemBlock],
    delta: &mut InventoryDelta,
    rejected_blocks: &mut usize,
) {
    let mut previous_moves = HashMap::<ItemKey, Item>::new();
    for block in blocks {
        let key = ItemKey::new(owner, ItemId::new(block.item_id));
        let Some(mut item) = previous_moves.get(&key).cloned().or_else(|| snapshot.item(key).cloned()) else {
            let destination = CategoryKey::new(owner, FolderId::new(block.folder_id));
            if snapshot.category(destination).is_some() {
                delta.updated_category_versions.push((destination, ::inventory::ServerVersion::UNKNOWN));
            }
            *rejected_blocks = rejected_blocks.saturating_add(1);
            continue;
        };
        let destination = CategoryKey::new(owner, FolderId::new(block.folder_id));
        if snapshot.category(destination).is_none() {
            delta.updated_category_versions.push((item.parent, ::inventory::ServerVersion::UNKNOWN));
            *rejected_blocks = rejected_blocks.saturating_add(1);
            continue;
        }
        item.parent = destination;
        if !block.new_name.is_empty() {
            item.name = Arc::from(block.new_name.as_str());
        }
        previous_moves.insert(key, item.clone());
        delta.updated_items.push(item);
    }
}

fn parse_event_message(name: &str, body: &Value, expected_agent: Uuid) -> Result<Option<Message>, InventoryIntakeError> {
    if !matches!(
        name,
        "UpdateCreateInventoryItem"
            | "RemoveInventoryItem"
            | "RemoveInventoryFolder"
            | "RemoveInventoryObjects"
            | "SaveAssetIntoInventory"
            | "BulkUpdateInventory"
            | "MoveInventoryItem"
    ) {
        return Ok(None);
    }
    let body = body.as_map().ok_or(InventoryIntakeError::MalformedEvent)?;
    let agent = single_block(body, "AgentData")?;
    let agent_id = non_nil_uuid(agent, "AgentID").ok_or(InventoryIntakeError::MalformedEvent)?;
    if agent_id != expected_agent {
        return Ok(None);
    }

    let message = match name {
        "UpdateCreateInventoryItem" => {
            let (inventory_data, invalid_blocks) = event_blocks(body, "InventoryData", parse_item_block)?;
            Message::UpdateCreateInventoryItem(UpdateCreateInventoryItem {
                agent_id,
                sim_approved: bool_field(agent, "SimApproved").ok_or(InventoryIntakeError::MalformedEvent)?,
                transaction_id: uuid_field(agent, "TransactionID").ok_or(InventoryIntakeError::MalformedEvent)?,
                inventory_data,
                invalid_blocks,
            })
        }
        "RemoveInventoryItem" => {
            let (inventory_data, invalid_blocks) = event_ids(body, "InventoryData", "ItemID")?;
            Message::RemoveInventoryItem(RemoveInventoryItem {
                agent_id,
                session_id: uuid_field(agent, "SessionID").ok_or(InventoryIntakeError::MalformedEvent)?,
                inventory_data,
                invalid_blocks,
            })
        }
        "RemoveInventoryFolder" => {
            let (folder_data, invalid_blocks) = event_ids(body, "FolderData", "FolderID")?;
            Message::RemoveInventoryFolder(RemoveInventoryFolder {
                agent_id,
                session_id: uuid_field(agent, "SessionID").ok_or(InventoryIntakeError::MalformedEvent)?,
                folder_data,
                invalid_blocks,
            })
        }
        "RemoveInventoryObjects" => {
            let (folder_data, invalid_folders) = event_ids(body, "FolderData", "FolderID")?;
            let (item_data, invalid_items) = event_ids(body, "ItemData", "ItemID")?;
            Message::RemoveInventoryObjects(RemoveInventoryObjects {
                agent_id,
                session_id: uuid_field(agent, "SessionID").ok_or(InventoryIntakeError::MalformedEvent)?,
                folder_data,
                item_data,
                invalid_blocks: invalid_folders.saturating_add(invalid_items),
            })
        }
        "SaveAssetIntoInventory" => {
            let item = single_block(body, "InventoryData")?;
            Message::SaveAssetIntoInventory(SaveAssetIntoInventory {
                agent_id,
                item_id: non_nil_uuid(item, "ItemID").ok_or(InventoryIntakeError::MalformedEvent)?,
                new_asset_id: non_nil_uuid(item, "NewAssetID").ok_or(InventoryIntakeError::MalformedEvent)?,
            })
        }
        "BulkUpdateInventory" => {
            let (folder_data, invalid_folders) = event_blocks(body, "FolderData", parse_folder_block)?;
            let (item_data, invalid_items) = event_blocks(body, "ItemData", parse_bulk_item_block)?;
            Message::BulkUpdateInventory(BulkUpdateInventory {
                agent_id,
                transaction_id: uuid_field(agent, "TransactionID").ok_or(InventoryIntakeError::MalformedEvent)?,
                folder_data,
                item_data,
                invalid_blocks: invalid_folders.saturating_add(invalid_items),
            })
        }
        "MoveInventoryItem" => {
            let (inventory_data, invalid_blocks) = event_blocks(body, "InventoryData", parse_move_block)?;
            Message::MoveInventoryItem(MoveInventoryItem {
                agent_id,
                session_id: uuid_field(agent, "SessionID").ok_or(InventoryIntakeError::MalformedEvent)?,
                stamp: bool_field(agent, "Stamp").ok_or(InventoryIntakeError::MalformedEvent)?,
                inventory_data,
                invalid_blocks,
            })
        }
        _ => return Ok(None),
    };
    Ok(Some(message))
}

fn single_block<'a>(body: &'a Map, name: &str) -> Result<&'a Map, InventoryIntakeError> {
    let values = body.get(name).and_then(Value::as_array).ok_or(InventoryIntakeError::MalformedEvent)?;
    if values.len() != 1 {
        return Err(InventoryIntakeError::MalformedEvent);
    }
    values.first().and_then(Value::as_map).ok_or(InventoryIntakeError::MalformedEvent)
}

fn event_blocks<T>(body: &Map, name: &str, mut decode: impl FnMut(&Map) -> Option<T>) -> Result<(Vec<T>, usize), InventoryIntakeError> {
    let Some(value) = body.get(name) else { return Ok((Vec::new(), 0)) };
    let values = value.as_array().ok_or(InventoryIntakeError::MalformedEvent)?;
    let mut output = Vec::with_capacity(values.len().min(MAX_BLOCKS));
    let mut invalid = values.len().saturating_sub(MAX_BLOCKS);
    for value in values.iter().take(MAX_BLOCKS) {
        match value.as_map().and_then(&mut decode) {
            Some(block) => output.push(block),
            None => invalid = invalid.saturating_add(1),
        }
    }
    Ok((output, invalid))
}

fn event_ids(body: &Map, block_name: &str, field_name: &str) -> Result<(Vec<Uuid>, usize), InventoryIntakeError> {
    event_blocks(body, block_name, |block| non_nil_uuid(block, field_name))
}

fn parse_folder_block(block: &Map) -> Option<InventoryFolderBlock> {
    let folder_id = non_nil_uuid(block, "FolderID")?;
    let parent_id = non_nil_uuid(block, "ParentID")?;
    let folder_type = i8::try_from(integer(block, "Type")?).ok()?;
    Some(InventoryFolderBlock { folder_id, parent_id, folder_type, name: bounded_string(block, "Name")? })
}

fn parse_item_block(block: &Map) -> Option<InventoryItemBlock> {
    let item_id = non_nil_uuid(block, "ItemID")?;
    let folder_id = non_nil_uuid(block, "FolderID")?;
    Some(InventoryItemBlock {
        item_id,
        folder_id,
        callback_id: unsigned_bits(block, "CallbackID")?,
        creator_id: uuid_field(block, "CreatorID")?,
        owner_id: uuid_field(block, "OwnerID")?,
        group_id: uuid_field(block, "GroupID")?,
        base_mask: unsigned_bits(block, "BaseMask")?,
        owner_mask: unsigned_bits(block, "OwnerMask")?,
        group_mask: unsigned_bits(block, "GroupMask")?,
        everyone_mask: unsigned_bits(block, "EveryoneMask")?,
        next_owner_mask: unsigned_bits(block, "NextOwnerMask")?,
        group_owned: bool_field(block, "GroupOwned")?,
        asset_id: uuid_field(block, "AssetID")?,
        asset_type: i8::try_from(integer(block, "Type")?).ok()?,
        inventory_type: i8::try_from(integer(block, "InvType")?).ok()?,
        flags: unsigned_bits(block, "Flags")?,
        sale_type: u8::try_from(integer(block, "SaleType")?).ok()?,
        sale_price: integer(block, "SalePrice")?,
        name: bounded_string(block, "Name")?,
        description: bounded_string(block, "Description")?,
        creation_date: integer(block, "CreationDate")?,
        crc: unsigned_bits(block, "CRC")?,
    })
}

fn parse_bulk_item_block(block: &Map) -> Option<InventoryItemBlock> {
    parse_item_block(block)
}

fn parse_move_block(block: &Map) -> Option<MoveInventoryItemBlock> {
    Some(MoveInventoryItemBlock {
        item_id: non_nil_uuid(block, "ItemID")?,
        folder_id: non_nil_uuid(block, "FolderID")?,
        new_name: bounded_string(block, "NewName")?,
    })
}

fn uuid_field(block: &Map, name: &str) -> Option<Uuid> {
    block.get(name)?.as_uuid()
}

fn non_nil_uuid(block: &Map, name: &str) -> Option<Uuid> {
    uuid_field(block, name).filter(|value| !value.is_nil())
}

fn integer(block: &Map, name: &str) -> Option<i32> {
    block.get(name)?.as_integer()
}

fn unsigned_bits(block: &Map, name: &str) -> Option<u32> {
    integer(block, name).map(|value| value as u32)
}

fn bool_field(block: &Map, name: &str) -> Option<bool> {
    block.get(name)?.as_bool()
}

fn bounded_string(block: &Map, name: &str) -> Option<String> {
    let value = block.get(name)?.as_str()?;
    (value.len() <= MAX_STRING_BYTES && !value.contains('\0')).then(|| value.to_owned())
}

#[cfg(test)]
#[path = "inventory_intake_tests.rs"]
mod tests;
