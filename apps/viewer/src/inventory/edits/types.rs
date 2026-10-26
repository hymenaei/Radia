use ::inventory::{CategoryKey, FolderType, InventoryNodeKey, ItemKey};
use tokio::sync::oneshot;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryEditOutcome {
    Applied,
    Created(CategoryKey),
    Rejected(InventoryEditRejection),
    ServerError,
    Gone,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryEditRejection {
    QueueFull,
    SessionEnded,
    MissingCategory,
    MissingItem,
    MissingTrash,
    DifferentOwner,
    RootFolder,
    ProtectedFolder,
    InvalidName,
    FolderCycle,
}

pub(super) enum EditOperation {
    CreateFolder { parent: CategoryKey, name: String, preferred_type: FolderType },
    RenameCategory { category: CategoryKey, name: String },
    RenameItem { item: ItemKey, name: String },
    MoveItem { item: ItemKey, destination: CategoryKey },
    MoveFolder { folder: CategoryKey, destination: CategoryKey },
    DeleteItem(ItemKey),
    DeleteFolder(CategoryKey),
    EmptyTrash,
}

pub(super) struct EditRequest {
    pub(super) operation: EditOperation,
    pub(super) response: oneshot::Sender<InventoryEditOutcome>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MoveExpectation {
    pub(crate) node: InventoryNodeKey,
    pub(crate) from: CategoryKey,
    pub(crate) to: CategoryKey,
}
