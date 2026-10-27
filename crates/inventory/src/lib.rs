mod cache;
mod delta;
mod mutation;
mod snapshot;
mod state;
mod types;

use std::{error::Error, fmt};

pub use cache::{
    InventoryCache, InventoryCacheError, InventoryCacheLoad, InventoryCacheOpenStatus, InventoryCacheRestore,
    InventoryCacheRestoreProgress, InventoryCacheWrite,
};
pub use mutation::{InventoryChild, MAX_FOLDER_FETCH_CHILDREN};
pub use snapshot::InventorySnapshot;
pub use state::{Inventory, InventoryLimits, InventoryRoots};
pub use types::{
    AssetType, Category, CategoryKey, ChangeSet, FolderType, InventoryChange, InventoryDelta, InventoryNodeId, InventoryNodeKey,
    InventoryOwner, InventoryRevision, InventoryType, Item, ItemKey, LinkResolution, PermissionMask, Permissions, SaleInfo, SaleType,
    ServerVersion,
};

pub const MAX_INVENTORY_CATEGORIES: usize = 131_074;
pub const MAX_INVENTORY_ITEMS: usize = 1_000_000;
pub const MAX_INVENTORY_NAME_BYTES: usize = 1_024;
pub const MAX_INVENTORY_DESCRIPTION_BYTES: usize = 4_096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryError {
    InvalidOwner,
    InvalidCategoryId,
    InvalidItemId,
    InvalidRoot,
    DuplicateRoots,
    DuplicateCategory,
    SharedRootOwner,
    MissingCategory,
    MissingItem,
    MissingParent,
    InvalidParent,
    Cycle,
    FixedRoot,
    DuplicateFetchChild,
    TooManyCategories,
    TooManyItems,
    TooManyDeltaOperations,
    TooManyFolderChildren,
    NameTooLong,
    DescriptionTooLong,
    InconsistentIndex,
    RevisionExhausted,
}

impl fmt::Display for InventoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidOwner => "inventory owner ID is nil",
            Self::InvalidCategoryId => "inventory category ID is nil",
            Self::InvalidItemId => "inventory item ID is nil",
            Self::InvalidRoot => "inventory root has an invalid parent",
            Self::DuplicateRoots => "personal inventory and Library roots have the same key",
            Self::DuplicateCategory => "inventory bootstrap contains a duplicate category key",
            Self::SharedRootOwner => "personal inventory and Library must have different owners",
            Self::MissingCategory => "inventory category does not exist",
            Self::MissingItem => "inventory item does not exist",
            Self::MissingParent => "inventory parent category does not exist",
            Self::InvalidParent => "inventory node owner or parent is invalid",
            Self::Cycle => "inventory move would create a parent cycle",
            Self::FixedRoot => "inventory roots cannot be moved or removed",
            Self::DuplicateFetchChild => "folder fetch contains a duplicate child ID",
            Self::TooManyCategories => "inventory category limit exceeded",
            Self::TooManyItems => "inventory item limit exceeded",
            Self::TooManyDeltaOperations => "inventory delta operation limit exceeded",
            Self::TooManyFolderChildren => "folder fetch child limit exceeded",
            Self::NameTooLong => "inventory name exceeds its byte limit",
            Self::DescriptionTooLong => "inventory description exceeds its byte limit",
            Self::InconsistentIndex => "inventory parent and child indexes disagree",
            Self::RevisionExhausted => "inventory revision space exhausted",
        })
    }
}

impl Error for InventoryError {}
