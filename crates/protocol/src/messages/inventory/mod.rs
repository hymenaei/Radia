mod blocks;
mod bulk_update_inventory;
mod move_inventory_folder;
mod move_inventory_item;
mod remove_inventory_folder;
mod remove_inventory_item;
mod remove_inventory_objects;
mod save_asset_into_inventory;
mod update_create_inventory_item;

pub use self::blocks::{InventoryFolderBlock, InventoryItemBlock, MoveInventoryFolderBlock, MoveInventoryItemBlock};
pub use self::bulk_update_inventory::BulkUpdateInventory;
pub use self::move_inventory_folder::MoveInventoryFolder;
pub use self::move_inventory_item::MoveInventoryItem;
pub use self::remove_inventory_folder::RemoveInventoryFolder;
pub use self::remove_inventory_item::RemoveInventoryItem;
pub use self::remove_inventory_objects::RemoveInventoryObjects;
pub use self::save_asset_into_inventory::SaveAssetIntoInventory;
pub use self::update_create_inventory_item::UpdateCreateInventoryItem;

use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::codec::{Reader, Writer},
};

pub(crate) fn validate_agent_id(agent_id: Uuid) -> Result<(), LLUdpError> {
    if agent_id.is_nil() { Err(LLUdpError::InvalidMessage) } else { Ok(()) }
}

pub(crate) fn read_bool(reader: &mut Reader<'_>) -> Result<bool, LLUdpError> {
    match reader.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(LLUdpError::InvalidMessage),
    }
}

pub(crate) fn write_bool(writer: &mut Writer<'_>, value: bool) {
    writer.u8(u8::from(value));
}

pub(crate) fn read_blocks<T>(
    reader: &mut Reader<'_>,
    decode: impl FnMut(&mut Reader<'_>) -> Result<Option<T>, LLUdpError>,
) -> Result<(Vec<T>, usize), LLUdpError> {
    let count = reader.terminal_variable_count()?;
    read_blocks_with_count(reader, count, decode)
}

pub(crate) fn read_blocks_with_count<T>(
    reader: &mut Reader<'_>,
    count: usize,
    mut decode: impl FnMut(&mut Reader<'_>) -> Result<Option<T>, LLUdpError>,
) -> Result<(Vec<T>, usize), LLUdpError> {
    let mut blocks = Vec::new();
    let mut invalid_blocks = 0usize;
    for _ in 0..count {
        match decode(reader)? {
            Some(block) => blocks.push(block),
            None => invalid_blocks += 1,
        }
    }
    Ok((blocks, invalid_blocks))
}

pub(crate) fn write_count(writer: &mut Writer<'_>, count: usize) -> Result<(), LLUdpError> {
    writer.u8(u8::try_from(count).map_err(|_| LLUdpError::VariableFieldTooLong)?);
    Ok(())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
