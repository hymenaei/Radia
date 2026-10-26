use std::sync::Arc;

use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InventoryOwner(Uuid);

impl InventoryOwner {
    pub const fn new(id: Uuid) -> Self {
        Self(id)
    }

    pub const fn id(self) -> Uuid {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CategoryKey {
    owner: InventoryOwner,
    id: FolderId,
}

impl CategoryKey {
    pub const fn new(owner: InventoryOwner, id: FolderId) -> Self {
        Self { owner, id }
    }

    pub const fn owner(self) -> InventoryOwner {
        self.owner
    }

    pub const fn id(self) -> FolderId {
        self.id
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ItemKey {
    owner: InventoryOwner,
    id: ItemId,
}

impl ItemKey {
    pub const fn new(owner: InventoryOwner, id: ItemId) -> Self {
        Self { owner, id }
    }

    pub const fn owner(self) -> InventoryOwner {
        self.owner
    }

    pub const fn id(self) -> ItemId {
        self.id
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum InventoryNodeId {
    Category(FolderId),
    Item(ItemId),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InventoryNodeKey {
    owner: InventoryOwner,
    id: InventoryNodeId,
}

impl InventoryNodeKey {
    pub const fn new(owner: InventoryOwner, id: InventoryNodeId) -> Self {
        Self { owner, id }
    }

    pub const fn owner(self) -> InventoryOwner {
        self.owner
    }

    pub const fn id(self) -> InventoryNodeId {
        self.id
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InventoryRevision(pub(crate) u64);

impl InventoryRevision {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServerVersion(Option<i32>);

impl ServerVersion {
    pub const UNKNOWN: Self = Self(None);

    pub const fn known(value: i32) -> Option<Self> {
        if value == -1 { None } else { Some(Self(Some(value))) }
    }

    pub const fn from_raw(value: i32) -> Self {
        if value == -1 { Self::UNKNOWN } else { Self(Some(value)) }
    }

    pub const fn raw(self) -> i32 {
        match self.0 {
            Some(value) => value,
            None => -1,
        }
    }

    pub const fn is_known(self) -> bool {
        self.0.is_some()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FolderType {
    Texture,
    Sound,
    CallingCard,
    Landmark,
    Clothing,
    Object,
    Notecard,
    RootInventory,
    LslText,
    Bodypart,
    Trash,
    Snapshot,
    LostAndFound,
    Animation,
    Gesture,
    Favorite,
    Ensemble(u8),
    CurrentOutfit,
    Outfit,
    MyOutfits,
    Mesh,
    Inbox,
    Outbox,
    BasicRoot,
    MarketplaceListings,
    MarketplaceStock,
    MarketplaceVersion,
    Settings,
    Material,
    None,
    Unknown(i32),
}

impl FolderType {
    pub const fn from_raw(value: i32) -> Self {
        match value {
            0 => Self::Texture,
            1 => Self::Sound,
            2 => Self::CallingCard,
            3 => Self::Landmark,
            5 => Self::Clothing,
            6 => Self::Object,
            7 => Self::Notecard,
            8 => Self::RootInventory,
            10 => Self::LslText,
            13 => Self::Bodypart,
            14 => Self::Trash,
            15 => Self::Snapshot,
            16 => Self::LostAndFound,
            20 => Self::Animation,
            21 => Self::Gesture,
            23 => Self::Favorite,
            26..=45 => Self::Ensemble((value - 26) as u8),
            46 => Self::CurrentOutfit,
            47 => Self::Outfit,
            48 => Self::MyOutfits,
            49 => Self::Mesh,
            50 => Self::Inbox,
            51 => Self::Outbox,
            52 => Self::BasicRoot,
            53 => Self::MarketplaceListings,
            54 => Self::MarketplaceStock,
            55 => Self::MarketplaceVersion,
            56 => Self::Settings,
            57 => Self::Material,
            -1 => Self::None,
            _ => Self::Unknown(value),
        }
    }

    pub const fn raw(self) -> i32 {
        match self {
            Self::Texture => 0,
            Self::Sound => 1,
            Self::CallingCard => 2,
            Self::Landmark => 3,
            Self::Clothing => 5,
            Self::Object => 6,
            Self::Notecard => 7,
            Self::RootInventory => 8,
            Self::LslText => 10,
            Self::Bodypart => 13,
            Self::Trash => 14,
            Self::Snapshot => 15,
            Self::LostAndFound => 16,
            Self::Animation => 20,
            Self::Gesture => 21,
            Self::Favorite => 23,
            Self::Ensemble(index) => index as i32 + 26,
            Self::CurrentOutfit => 46,
            Self::Outfit => 47,
            Self::MyOutfits => 48,
            Self::Mesh => 49,
            Self::Inbox => 50,
            Self::Outbox => 51,
            Self::BasicRoot => 52,
            Self::MarketplaceListings => 53,
            Self::MarketplaceStock => 54,
            Self::MarketplaceVersion => 55,
            Self::Settings => 56,
            Self::Material => 57,
            Self::None => -1,
            Self::Unknown(value) => value,
        }
    }

    pub const fn is_protected(self) -> bool {
        !matches!(
            self,
            Self::Ensemble(_) | Self::Outfit | Self::MarketplaceListings | Self::MarketplaceStock | Self::MarketplaceVersion | Self::None
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AssetType {
    Texture,
    Sound,
    CallingCard,
    Landmark,
    Script,
    Clothing,
    Object,
    Notecard,
    Category,
    LslText,
    LslBytecode,
    TextureTga,
    Bodypart,
    SoundWav,
    ImageTga,
    ImageJpeg,
    Animation,
    Gesture,
    Simstate,
    Link,
    LinkFolder,
    MarketplaceFolder,
    Widget,
    Person,
    Mesh,
    Reserved(u8),
    Settings,
    Material,
    Gltf,
    GltfBinary,
    None,
    Unknown(i32),
}

impl AssetType {
    pub const fn from_raw(value: i32) -> Self {
        match value {
            0 => Self::Texture,
            1 => Self::Sound,
            2 => Self::CallingCard,
            3 => Self::Landmark,
            4 => Self::Script,
            5 => Self::Clothing,
            6 => Self::Object,
            7 => Self::Notecard,
            8 => Self::Category,
            10 => Self::LslText,
            11 => Self::LslBytecode,
            12 => Self::TextureTga,
            13 => Self::Bodypart,
            17 => Self::SoundWav,
            18 => Self::ImageTga,
            19 => Self::ImageJpeg,
            20 => Self::Animation,
            21 => Self::Gesture,
            22 => Self::Simstate,
            24 => Self::Link,
            25 => Self::LinkFolder,
            26 => Self::MarketplaceFolder,
            40 => Self::Widget,
            45 => Self::Person,
            49 => Self::Mesh,
            50..=55 => Self::Reserved((value - 50) as u8),
            56 => Self::Settings,
            57 => Self::Material,
            58 => Self::Gltf,
            59 => Self::GltfBinary,
            -1 => Self::None,
            _ => Self::Unknown(value),
        }
    }

    pub const fn raw(self) -> i32 {
        match self {
            Self::Texture => 0,
            Self::Sound => 1,
            Self::CallingCard => 2,
            Self::Landmark => 3,
            Self::Script => 4,
            Self::Clothing => 5,
            Self::Object => 6,
            Self::Notecard => 7,
            Self::Category => 8,
            Self::LslText => 10,
            Self::LslBytecode => 11,
            Self::TextureTga => 12,
            Self::Bodypart => 13,
            Self::SoundWav => 17,
            Self::ImageTga => 18,
            Self::ImageJpeg => 19,
            Self::Animation => 20,
            Self::Gesture => 21,
            Self::Simstate => 22,
            Self::Link => 24,
            Self::LinkFolder => 25,
            Self::MarketplaceFolder => 26,
            Self::Widget => 40,
            Self::Person => 45,
            Self::Mesh => 49,
            Self::Reserved(index) => index as i32 + 50,
            Self::Settings => 56,
            Self::Material => 57,
            Self::Gltf => 58,
            Self::GltfBinary => 59,
            Self::None => -1,
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InventoryType {
    Texture,
    Sound,
    CallingCard,
    Landmark,
    Object,
    Notecard,
    Category,
    RootCategory,
    Lsl,
    Snapshot,
    Attachment,
    Wearable,
    Animation,
    Gesture,
    Mesh,
    Widget,
    Person,
    Settings,
    Material,
    Gltf,
    GltfBinary,
    None,
    Unknown(i32),
}

impl InventoryType {
    pub const fn from_raw(value: i32) -> Self {
        match value {
            0 => Self::Texture,
            1 => Self::Sound,
            2 => Self::CallingCard,
            3 => Self::Landmark,
            6 => Self::Object,
            7 => Self::Notecard,
            8 => Self::Category,
            9 => Self::RootCategory,
            10 => Self::Lsl,
            15 => Self::Snapshot,
            17 => Self::Attachment,
            18 => Self::Wearable,
            19 => Self::Animation,
            20 => Self::Gesture,
            22 => Self::Mesh,
            23 => Self::Widget,
            24 => Self::Person,
            25 => Self::Settings,
            26 => Self::Material,
            27 => Self::Gltf,
            28 => Self::GltfBinary,
            -1 => Self::None,
            _ => Self::Unknown(value),
        }
    }

    pub const fn raw(self) -> i32 {
        match self {
            Self::Texture => 0,
            Self::Sound => 1,
            Self::CallingCard => 2,
            Self::Landmark => 3,
            Self::Object => 6,
            Self::Notecard => 7,
            Self::Category => 8,
            Self::RootCategory => 9,
            Self::Lsl => 10,
            Self::Snapshot => 15,
            Self::Attachment => 17,
            Self::Wearable => 18,
            Self::Animation => 19,
            Self::Gesture => 20,
            Self::Mesh => 22,
            Self::Widget => 23,
            Self::Person => 24,
            Self::Settings => 25,
            Self::Material => 26,
            Self::Gltf => 27,
            Self::GltfBinary => 28,
            Self::None => -1,
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PermissionMask(u32);

impl PermissionMask {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Permissions {
    pub creator: AvatarId,
    pub owner: AvatarId,
    pub group: GroupId,
    pub last_owner: AvatarId,
    pub base: PermissionMask,
    pub owner_mask: PermissionMask,
    pub group_mask: PermissionMask,
    pub everyone: PermissionMask,
    pub next_owner: PermissionMask,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaleType {
    NotForSale,
    Original,
    Copy,
    Contents,
    Unknown(i32),
}

impl SaleType {
    pub const fn from_raw(value: i32) -> Self {
        match value {
            0 => Self::NotForSale,
            1 => Self::Original,
            2 => Self::Copy,
            3 => Self::Contents,
            _ => Self::Unknown(value),
        }
    }

    pub const fn raw(self) -> i32 {
        match self {
            Self::NotForSale => 0,
            Self::Original => 1,
            Self::Copy => 2,
            Self::Contents => 3,
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SaleInfo {
    pub sale_type: SaleType,
    pub price: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Category {
    pub key: CategoryKey,
    pub parent: Option<CategoryKey>,
    pub preferred_folder_type: FolderType,
    pub name: Arc<str>,
    pub server_version: ServerVersion,
    pub server_descendent_count: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Item {
    pub key: ItemKey,
    pub parent: CategoryKey,
    pub asset_id: AssetId,
    pub asset_type: AssetType,
    pub inventory_type: InventoryType,
    pub flags: u32,
    pub name: Arc<str>,
    pub description: Arc<str>,
    pub created_at: i64,
    pub permissions: Permissions,
    pub sale: SaleInfo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkResolution {
    NotLink,
    Resolved(InventoryNodeKey),
    NotLoaded(Uuid),
    Broken(Uuid),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InventoryChange {
    CategoryUpserted(Arc<Category>),
    ItemUpserted(Arc<Item>),
    NodeMoved { node: InventoryNodeKey, from: CategoryKey, to: CategoryKey },
    CategoryRemoved(CategoryKey),
    ItemRemoved(ItemKey),
    FolderFetchApplied { folder: CategoryKey, child_count: usize },
    VersionSet { folder: CategoryKey, version: ServerVersion },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeSet {
    pub revision: InventoryRevision,
    pub changes: Vec<InventoryChange>,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}
