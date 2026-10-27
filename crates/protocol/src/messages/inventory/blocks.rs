use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::codec::{Reader, Writer},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryFolderBlock {
    pub folder_id: Uuid,
    pub parent_id: Uuid,
    pub folder_type: i8,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryItemBlock {
    pub item_id: Uuid,
    pub folder_id: Uuid,
    pub callback_id: u32,
    pub creator_id: Uuid,
    pub owner_id: Uuid,
    pub group_id: Uuid,
    pub base_mask: u32,
    pub owner_mask: u32,
    pub group_mask: u32,
    pub everyone_mask: u32,
    pub next_owner_mask: u32,
    pub group_owned: bool,
    pub asset_id: Uuid,
    pub asset_type: i8,
    pub inventory_type: i8,
    pub flags: u32,
    pub sale_type: u8,
    pub sale_price: i32,
    pub name: String,
    pub description: String,
    pub creation_date: i32,
    pub crc: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MoveInventoryItemBlock {
    pub item_id: Uuid,
    pub folder_id: Uuid,
    pub new_name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MoveInventoryFolderBlock {
    pub folder_id: Uuid,
    pub parent_id: Uuid,
}

#[derive(Clone, Copy)]
pub(crate) enum ItemBlockOrder {
    UpdateCreate,
    Bulk,
}

pub(crate) fn read_folder(reader: &mut Reader<'_>) -> Result<Option<InventoryFolderBlock>, LLUdpError> {
    let folder_id = reader.uuid()?;
    let parent_id = reader.uuid()?;
    let folder_type = reader.i8()?;
    let name = read_string(reader)?;
    Ok(match (name, !folder_id.is_nil() && !parent_id.is_nil()) {
        (Some(name), true) => Some(InventoryFolderBlock { folder_id, parent_id, folder_type, name }),
        _ => None,
    })
}

pub(crate) fn write_folder(writer: &mut Writer<'_>, block: &InventoryFolderBlock) -> Result<(), LLUdpError> {
    if block.folder_id.is_nil() || block.parent_id.is_nil() {
        return Err(LLUdpError::InvalidMessage);
    }
    writer.uuid(block.folder_id);
    writer.uuid(block.parent_id);
    writer.i8(block.folder_type);
    writer.variable1_string(&block.name)
}

pub(crate) fn read_item(reader: &mut Reader<'_>, order: ItemBlockOrder) -> Result<Option<InventoryItemBlock>, LLUdpError> {
    let item_id = reader.uuid()?;
    let callback_id;
    let folder_id;
    match order {
        ItemBlockOrder::UpdateCreate => {
            folder_id = reader.uuid()?;
            callback_id = reader.u32()?;
        }
        ItemBlockOrder::Bulk => {
            callback_id = reader.u32()?;
            folder_id = reader.uuid()?;
        }
    }
    let creator_id = reader.uuid()?;
    let owner_id = reader.uuid()?;
    let group_id = reader.uuid()?;
    let base_mask = reader.u32()?;
    let owner_mask = reader.u32()?;
    let group_mask = reader.u32()?;
    let everyone_mask = reader.u32()?;
    let next_owner_mask = reader.u32()?;
    let group_owned = reader.u8()?;
    let asset_id = reader.uuid()?;
    let asset_type = reader.i8()?;
    let inventory_type = reader.i8()?;
    let flags = reader.u32()?;
    let sale_type = reader.u8()?;
    let sale_price = reader.i32()?;
    let name = read_string(reader)?;
    let description = read_string(reader)?;
    let creation_date = reader.i32()?;
    let crc = reader.u32()?;

    Ok(match (name, description, !item_id.is_nil() && !folder_id.is_nil() && group_owned <= 1) {
        (Some(name), Some(description), true) => Some(InventoryItemBlock {
            item_id,
            folder_id,
            callback_id,
            creator_id,
            owner_id,
            group_id,
            base_mask,
            owner_mask,
            group_mask,
            everyone_mask,
            next_owner_mask,
            group_owned: group_owned == 1,
            asset_id,
            asset_type,
            inventory_type,
            flags,
            sale_type,
            sale_price,
            name,
            description,
            creation_date,
            crc,
        }),
        _ => None,
    })
}

pub(crate) fn write_item(writer: &mut Writer<'_>, block: &InventoryItemBlock, order: ItemBlockOrder) -> Result<(), LLUdpError> {
    if block.item_id.is_nil() || block.folder_id.is_nil() {
        return Err(LLUdpError::InvalidMessage);
    }
    writer.uuid(block.item_id);
    match order {
        ItemBlockOrder::UpdateCreate => {
            writer.uuid(block.folder_id);
            writer.u32(block.callback_id);
        }
        ItemBlockOrder::Bulk => {
            writer.u32(block.callback_id);
            writer.uuid(block.folder_id);
        }
    }
    writer.uuid(block.creator_id);
    writer.uuid(block.owner_id);
    writer.uuid(block.group_id);
    writer.u32(block.base_mask);
    writer.u32(block.owner_mask);
    writer.u32(block.group_mask);
    writer.u32(block.everyone_mask);
    writer.u32(block.next_owner_mask);
    writer.u8(u8::from(block.group_owned));
    writer.uuid(block.asset_id);
    writer.i8(block.asset_type);
    writer.i8(block.inventory_type);
    writer.u32(block.flags);
    writer.u8(block.sale_type);
    writer.i32(block.sale_price);
    writer.variable1_string(&block.name)?;
    writer.variable1_string(&block.description)?;
    writer.i32(block.creation_date);
    writer.u32(block.crc);
    Ok(())
}

pub(crate) fn read_move(reader: &mut Reader<'_>) -> Result<Option<MoveInventoryItemBlock>, LLUdpError> {
    let item_id = reader.uuid()?;
    let folder_id = reader.uuid()?;
    let new_name = read_string(reader)?;
    Ok(match (new_name, !item_id.is_nil() && !folder_id.is_nil()) {
        (Some(new_name), true) => Some(MoveInventoryItemBlock { item_id, folder_id, new_name }),
        _ => None,
    })
}

pub(crate) fn write_move(writer: &mut Writer<'_>, block: &MoveInventoryItemBlock) -> Result<(), LLUdpError> {
    if block.item_id.is_nil() || block.folder_id.is_nil() {
        return Err(LLUdpError::InvalidMessage);
    }
    writer.uuid(block.item_id);
    writer.uuid(block.folder_id);
    writer.variable1_string(&block.new_name)
}

pub(crate) fn read_move_folder(reader: &mut Reader<'_>) -> Result<Option<MoveInventoryFolderBlock>, LLUdpError> {
    let folder_id = reader.uuid()?;
    let parent_id = reader.uuid()?;
    Ok((!folder_id.is_nil() && !parent_id.is_nil()).then_some(MoveInventoryFolderBlock { folder_id, parent_id }))
}

pub(crate) fn write_move_folder(writer: &mut Writer<'_>, block: &MoveInventoryFolderBlock) -> Result<(), LLUdpError> {
    if block.folder_id.is_nil() || block.parent_id.is_nil() {
        return Err(LLUdpError::InvalidMessage);
    }
    writer.uuid(block.folder_id);
    writer.uuid(block.parent_id);
    Ok(())
}

pub(crate) fn read_id(reader: &mut Reader<'_>) -> Result<Option<Uuid>, LLUdpError> {
    let id = reader.uuid()?;
    Ok((!id.is_nil()).then_some(id))
}

pub(crate) fn write_id(writer: &mut Writer<'_>, id: Uuid) -> Result<(), LLUdpError> {
    if id.is_nil() {
        return Err(LLUdpError::InvalidMessage);
    }
    writer.uuid(id);
    Ok(())
}

pub(crate) fn read_string(reader: &mut Reader<'_>) -> Result<Option<String>, LLUdpError> {
    let bytes = reader.variable1()?;
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    Ok(std::str::from_utf8(&bytes[..end]).ok().map(str::to_owned))
}

pub(crate) fn add_item_capacity(total: &mut usize, block: &InventoryItemBlock) {
    crate::lludp::message::allocation::add_string_capacity(total, &block.name);
    crate::lludp::message::allocation::add_string_capacity(total, &block.description);
}
