use std::{collections::BTreeMap, sync::Arc};

use ::inventory::{AssetType, CategoryKey, InventoryOwner, InventorySnapshot, InventoryType, Item, ItemKey};

const SETTINGS_SUBTYPE_MASK: u32 = 0xff;
const MAX_ENVIRONMENT_LABEL_CHARS: usize = 128;

pub(crate) struct EnvironmentMenu {
    items: Arc<[EnvironmentMenuItem]>,
}

pub(crate) struct EnvironmentMenuItem {
    pub(crate) key: ItemKey,
    pub(crate) asset_id: utils::AssetId,
    name: Arc<str>,
    kind: EnvironmentSettingKind,
    library: bool,
    parent: CategoryKey,
    folder: Option<Arc<str>>,
    duplicate: bool,
}

impl EnvironmentMenu {
    pub(crate) fn items(&self) -> &[EnvironmentMenuItem] {
        &self.items
    }

    pub(crate) fn label(&self, key: ItemKey) -> Option<String> {
        self.items.iter().find(|item| item.key == key).map(EnvironmentMenuItem::label)
    }
}

impl EnvironmentMenuItem {
    pub(crate) fn label(&self) -> String {
        let label = environment_setting_label(&self.name, self.kind, self.library);
        if self.duplicate {
            let folder =
                self.folder.as_deref().map(|name| visible_name(name).collect::<String>()).unwrap_or_else(|| "Unknown folder".into());
            format!("{label} / {folder} [{}]", self.key.id().uuid())
        } else {
            label
        }
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
}

impl EnvironmentSettingKind {
    fn label(self) -> &'static str {
        match self {
            Self::Sky => "Sky",
            Self::Water => "Water",
            Self::DayCycle => "Day cycle",
        }
    }
}

pub(crate) fn environment_items(
    snapshot: InventorySnapshot,
    previous: Option<Arc<EnvironmentMenu>>,
    cancelled: impl Fn() -> bool,
) -> Option<Arc<EnvironmentMenu>> {
    let library_owner = snapshot.roots().library().map(CategoryKey::owner);
    if let Some(previous) = previous.as_ref()
        && previous.matches(&snapshot, library_owner, &cancelled)?
    {
        return Some(Arc::clone(previous));
    }
    let mut sorted = BTreeMap::new();
    for item in snapshot.items() {
        if cancelled() {
            return None;
        }
        let Some(kind) = environment_setting_kind(item) else { continue };
        let library = library_owner == Some(item.key.owner());
        let key = (displayed_name(&item.name), kind.label(), library, item.key);
        sorted.insert(
            key,
            EnvironmentMenuItem {
                key: item.key,
                asset_id: item.asset_id,
                name: Arc::clone(&item.name),
                kind,
                library,
                parent: item.parent,
                folder: snapshot.category(item.parent).map(|folder| Arc::clone(&folder.name)),
                duplicate: false,
            },
        );
    }
    let mut items: Vec<EnvironmentMenuItem> = Vec::with_capacity(sorted.len());
    for (_, mut row) in sorted {
        if cancelled() {
            return None;
        }
        if let Some(previous) = items.last_mut()
            && previous.kind == row.kind
            && previous.library == row.library
            && displayed_name(&previous.name) == displayed_name(&row.name)
        {
            previous.duplicate = true;
            row.duplicate = true;
        }
        items.push(row);
    }
    Some(Arc::new(EnvironmentMenu { items: Arc::from(items) }))
}

impl EnvironmentMenu {
    fn matches(&self, snapshot: &InventorySnapshot, library_owner: Option<InventoryOwner>, cancelled: &impl Fn() -> bool) -> Option<bool> {
        let mut count = 0;
        for item in snapshot.items() {
            if cancelled() {
                return None;
            }
            count += usize::from(environment_setting_kind(item).is_some());
        }
        if count != self.items.len() {
            return Some(false);
        }
        for row in self.items.iter() {
            if cancelled() {
                return None;
            }
            let matches = snapshot.item(row.key).is_some_and(|item| {
                item.asset_id == row.asset_id
                    && item.name.as_ref() == row.name.as_ref()
                    && environment_setting_kind(item) == Some(row.kind)
                    && (library_owner == Some(item.key.owner())) == row.library
                    && item.parent == row.parent
                    && snapshot.category(item.parent).map(|folder| folder.name.as_ref()) == row.folder.as_deref()
            });
            if !matches {
                return Some(false);
            }
        }
        Some(true)
    }
}

fn environment_setting_kind(item: &Item) -> Option<EnvironmentSettingKind> {
    if item.asset_type != AssetType::Settings || item.inventory_type != InventoryType::Settings {
        return None;
    }
    match item.flags & SETTINGS_SUBTYPE_MASK {
        0 => Some(EnvironmentSettingKind::Sky),
        1 => Some(EnvironmentSettingKind::Water),
        2 => Some(EnvironmentSettingKind::DayCycle),
        _ => None,
    }
}

fn visible_name(name: &str) -> impl Iterator<Item = char> + '_ {
    name.chars().filter(|character| !character.is_control()).take(MAX_ENVIRONMENT_LABEL_CHARS)
}

fn displayed_name(name: &str) -> String {
    let name = visible_name(name).collect::<String>();
    if name.is_empty() { "Unnamed setting".into() } else { name }
}

fn environment_setting_label(name: &str, kind: EnvironmentSettingKind, library: bool) -> String {
    let name = displayed_name(name);
    if library { format!("{name} ({}, Library)", kind.label()) } else { format!("{name} ({})", kind.label()) }
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

    fn environment_items(
        snapshot: ::inventory::InventorySnapshot,
        previous: Option<Arc<super::EnvironmentMenu>>,
    ) -> Arc<super::EnvironmentMenu> {
        super::environment_items(snapshot, previous, || false).unwrap()
    }

    #[test]
    fn menu_filters_supported_settings() {
        let (mut inventory, personal) = model();
        let library = inventory.roots().library().expect("library root").owner();
        for (owner, id, name, flags) in [(personal, 3, "Sky\nPreset", 0), (library, 4, "Water", 0x4000_0101), (personal, 5, "Cycle", 2)] {
            let mut setting = item(owner, id, if owner == personal { 1 } else { 2 }, name, AssetType::Settings, InventoryType::Settings);
            setting.flags = flags;
            inventory.upsert_item(setting).expect("settings item");
        }
        let mut unsupported = item(personal, 6, 1, "Other", AssetType::Settings, InventoryType::Settings);
        unsupported.flags = 255;
        inventory.upsert_item(unsupported).expect("unsupported subtype");
        inventory
            .upsert_item(item(personal, 7, 1, "Wrong inventory type", AssetType::Settings, InventoryType::Notecard))
            .expect("mismatched inventory type");
        inventory.upsert_item(item(personal, 8, 1, "Not settings", AssetType::Notecard, InventoryType::Notecard)).expect("notecard");

        let menu = environment_items(inventory.snapshot(), None);
        let items = menu.items();

        assert_eq!(items.len(), 3);
        assert_eq!(
            items.iter().map(|item| item.label()).collect::<Vec<_>>(),
            ["Cycle (Day cycle)", "SkyPreset (Sky)", "Water (Water, Library)",]
        );
        assert_eq!(menu.label(items[1].key).as_deref(), Some("SkyPreset (Sky)"));
        assert!(items.iter().any(|item| item.key.owner() == library));
    }

    #[test]
    fn menu_orders_displayed_names() {
        let (mut inventory, personal) = model();
        for (id, name) in [(3, "a\u{1}z"), (4, "a0")] {
            inventory.upsert_item(item(personal, id, 1, name, AssetType::Settings, InventoryType::Settings)).unwrap();
        }
        let menu = environment_items(inventory.snapshot(), None);
        assert_eq!(menu.items().iter().map(|row| row.label()).collect::<Vec<_>>(), ["a0 (Sky)", "az (Sky)"]);
    }

    #[test]
    fn duplicate_names_keep_distinct_choices() {
        let (mut inventory, personal) = model();
        inventory.upsert_category(category(personal, 9, Some(1), "Other folder")).unwrap();
        for (id, parent) in [(3, 1), (4, 9), (5, 9)] {
            inventory.upsert_item(item(personal, id, parent, "Sky", AssetType::Settings, InventoryType::Settings)).unwrap();
        }
        let menu = environment_items(inventory.snapshot(), None);
        let labels = menu.items().iter().map(|row| row.label()).collect::<std::collections::HashSet<_>>();
        assert_eq!(labels.len(), 3);
        for row in menu.items() {
            assert_eq!(menu.label(row.key), Some(row.label()));
            assert!(row.label().contains(&row.key.id().uuid().to_string()));
        }
        inventory.upsert_category(category(personal, 9, Some(1), "Renamed folder")).unwrap();
        let updated = environment_items(inventory.snapshot(), Some(Arc::clone(&menu)));
        assert!(!Arc::ptr_eq(&updated, &menu));
        assert!(updated.items().iter().any(|row| row.label().contains("Renamed folder")));
    }

    #[test]
    fn cancelled_rebuild_stops_scanning() {
        let (mut inventory, personal) = model();
        for id in 10..110 {
            inventory.upsert_item(item(personal, id, 1, "Sky", AssetType::Settings, InventoryType::Settings)).unwrap();
        }
        for stop_at in [10, 150] {
            let (sender, receiver) = tokio::sync::watch::channel(());
            let receiver = std::cell::RefCell::new(Some(receiver));
            let calls = std::cell::Cell::new(0);
            let cancelled = super::environment_items(inventory.snapshot(), None, || {
                calls.set(calls.get() + 1);
                if calls.get() == stop_at {
                    receiver.borrow_mut().take();
                }
                sender.is_closed()
            });
            assert!(cancelled.is_none());
            assert_eq!(calls.get(), stop_at);
        }
        let next = environment_items(inventory.snapshot(), None);
        assert_eq!(next.items().len(), 100);
    }

    #[test]
    fn unchanged_menu_reuses_rows() {
        let (mut inventory, personal) = model();
        inventory.upsert_item(item(personal, 3, 1, "Sky", AssetType::Settings, InventoryType::Settings)).expect("settings item");
        let first = environment_items(inventory.snapshot(), None);
        inventory.upsert_item(item(personal, 4, 1, "Notecard", AssetType::Notecard, InventoryType::Notecard)).expect("notecard");
        let second = environment_items(inventory.snapshot(), Some(Arc::clone(&first)));
        assert!(Arc::ptr_eq(&first, &second));
        inventory.upsert_item(item(personal, 5, 1, "Cycle", AssetType::Settings, InventoryType::Settings)).expect("new setting");
        let third = environment_items(inventory.snapshot(), Some(second));
        assert_eq!(third.items().len(), 2);
        assert!(!Arc::ptr_eq(&first, &third));
    }

    #[test]
    fn large_menu_reuses_unchanged_rows() {
        let (mut inventory, personal) = model();
        for id in 10_u128..10_010 {
            inventory.upsert_item(item(personal, id, 1, "Setting", AssetType::Settings, InventoryType::Settings)).expect("settings item");
        }
        let first = environment_items(inventory.snapshot(), None);
        inventory.upsert_item(item(personal, 20_000, 1, "Notecard", AssetType::Notecard, InventoryType::Notecard)).expect("notecard");
        let second = environment_items(inventory.snapshot(), Some(Arc::clone(&first)));
        assert_eq!(first.items().len(), 10_000);
        assert!(Arc::ptr_eq(&first, &second));
        assert!(Arc::ptr_eq(&first.items()[0].name, &inventory.snapshot().item(first.items()[0].key).unwrap().name));
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
