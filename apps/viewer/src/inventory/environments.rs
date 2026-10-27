use std::{collections::HashMap, sync::Arc};

use ::inventory::{AssetType, CategoryKey, InventoryOwner, InventorySnapshot, Item, ItemKey};

const SETTINGS_SUBTYPE_MASK: u32 = 0xff;
const MAX_ENVIRONMENT_LABEL_CHARS: usize = 128;

#[derive(Clone)]
pub(crate) struct EnvironmentMenu {
    items: Arc<[EnvironmentMenuItem]>,
    indices: HashMap<ItemKey, usize>,
}

#[derive(Clone)]
pub(crate) struct EnvironmentMenuItem {
    pub(crate) key: ItemKey,
    pub(crate) asset_id: utils::AssetId,
    pub(crate) label: Arc<str>,
}

impl EnvironmentMenu {
    pub(crate) fn items(&self) -> &[EnvironmentMenuItem] {
        &self.items
    }

    pub(crate) fn label(&self, key: ItemKey) -> Option<&str> {
        self.indices.get(&key).and_then(|index| self.items.get(*index)).map(|item| &*item.label)
    }
}

#[derive(Clone)]
pub(crate) enum EnvironmentItems {
    Loading,
    Ready(Arc<EnvironmentMenu>),
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EnvironmentSettingKind {
    Sky,
    Water,
    DayCycle,
    Other,
}

impl EnvironmentSettingKind {
    fn label(self) -> &'static str {
        match self {
            Self::Sky => "Sky",
            Self::Water => "Water",
            Self::DayCycle => "Day cycle",
            Self::Other => "Settings",
        }
    }
}

pub(crate) fn environment_items(snapshot: InventorySnapshot) -> EnvironmentMenu {
    let library_owner = snapshot.roots().library().map(CategoryKey::owner);
    let mut items = snapshot
        .items_of_asset_type(AssetType::Settings)
        .map(|item| EnvironmentMenuItem {
            key: item.key,
            asset_id: item.asset_id,
            label: Arc::from(environment_setting_label(item, library_owner)),
        })
        .collect::<Vec<_>>();
    items.sort_unstable_by(|left, right| (left.label.as_ref(), left.key).cmp(&(right.label.as_ref(), right.key)));
    let indices = items.iter().enumerate().map(|(index, item)| (item.key, index)).collect();
    EnvironmentMenu { items: Arc::from(items), indices }
}

fn environment_setting_kind(flags: u32) -> EnvironmentSettingKind {
    match flags & SETTINGS_SUBTYPE_MASK {
        0 => EnvironmentSettingKind::Sky,
        1 => EnvironmentSettingKind::Water,
        2 => EnvironmentSettingKind::DayCycle,
        _ => EnvironmentSettingKind::Other,
    }
}

pub(crate) fn environment_setting_label(item: &Item, library_owner: Option<InventoryOwner>) -> String {
    let mut name = item.name.chars().filter(|character| !character.is_control()).take(MAX_ENVIRONMENT_LABEL_CHARS).collect::<String>();
    if name.is_empty() {
        name.push_str("Unnamed setting");
    }
    let kind = environment_setting_kind(item.flags).label();
    if library_owner == Some(item.key.owner()) { format!("{name} ({kind}, Library)") } else { format!("{name} ({kind})") }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ::inventory::{
        AssetType, Category, CategoryKey, FolderType, Inventory, InventoryLimits, InventoryOwner, InventoryType, Item, ItemKey,
        PermissionMask, Permissions, SaleInfo, SaleType, ServerVersion,
    };
    use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
    use uuid::Uuid;

    use super::environment_items;

    #[test]
    fn environment_items_cover_personal_and_library_settings() {
        let (mut inventory, personal) = model();
        let library = inventory.roots().library().expect("library root").owner();
        for (owner, id, name, flags) in
            [(personal, 3, "Sky\nPreset", 0), (library, 4, "Water", 0x4000_0101), (personal, 5, "Cycle", 2), (library, 6, "Other", 255)]
        {
            let mut setting = item(owner, id, if owner == personal { 1 } else { 2 }, name, AssetType::Settings, InventoryType::Settings);
            setting.flags = flags;
            inventory.upsert_item(setting).expect("settings item");
        }
        inventory.upsert_item(item(personal, 7, 1, "Not settings", AssetType::Notecard, InventoryType::Notecard)).expect("notecard");

        let menu = environment_items(inventory.snapshot());
        let items = menu.items();

        assert_eq!(items.len(), 4);
        assert_eq!(
            items.iter().map(|item| item.label.as_ref()).collect::<Vec<_>>(),
            ["Cycle (Day cycle)", "Other (Settings, Library)", "SkyPreset (Sky)", "Water (Water, Library)",]
        );
        assert_eq!(menu.label(items[2].key), Some("SkyPreset (Sky)"));
        assert!(items.iter().any(|item| item.key.owner() == library));

        inventory.upsert_item(item(personal, 8, 1, "Later", AssetType::Settings, InventoryType::Settings)).expect("later setting");
        assert_eq!(environment_items(inventory.snapshot()).items().len(), 5);
    }

    fn model() -> (Inventory, InventoryOwner) {
        let personal = InventoryOwner::new(uuid(1));
        let library = InventoryOwner::new(uuid(2));
        (
            Inventory::new(
                category(personal, 1, None, "Personal"),
                Some(category(library, 2, None, "Library")),
                InventoryLimits::default(),
            )
            .expect("roots"),
            personal,
        )
    }

    fn category(owner: InventoryOwner, id: u128, parent: Option<u128>, name: &str) -> Category {
        Category {
            key: CategoryKey::new(owner, FolderId::new(uuid(id))),
            parent: parent.map(|parent| CategoryKey::new(owner, FolderId::new(uuid(parent)))),
            preferred_folder_type: FolderType::None,
            name: Arc::from(name),
            server_version: ServerVersion::UNKNOWN,
            server_descendent_count: None,
        }
    }

    fn item(owner: InventoryOwner, id: u128, parent: u128, name: &str, asset_type: AssetType, inventory_type: InventoryType) -> Item {
        Item {
            key: ItemKey::new(owner, ItemId::new(uuid(id))),
            parent: CategoryKey::new(owner, FolderId::new(uuid(parent))),
            asset_id: AssetId::new(uuid(id + 1000)),
            asset_type,
            inventory_type,
            flags: 0,
            name: Arc::from(name),
            description: Arc::from(""),
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

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }
}
