use std::collections::{BTreeMap, HashSet};

use ::inventory::{AssetType, CategoryKey, InventoryNodeId, InventoryNodeKey, InventorySnapshot, InventoryType, Item, ItemKey};

pub const MAX_SEARCH_RESULTS: usize = 2_000;
pub const MAX_TREE_ROWS: usize = 250_000;

const SEARCH_CANCEL_INTERVAL: usize = 256;
const MAX_SEARCH_PATH_BYTES: usize = 4_096;
const MAX_SEARCH_PATH_DEPTH: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchTypeFilter {
    All,
    Asset(AssetType),
    Inventory(InventoryType),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchResult {
    pub key: ItemKey,
    pub name: std::sync::Arc<str>,
    pub asset_type: AssetType,
    pub inventory_type: InventoryType,
    pub folder_path: String,
    pub library: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchResults {
    pub items: Vec<SearchResult>,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TreeRow {
    pub node: InventoryNodeKey,
    pub depth: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TreeRows {
    pub rows: Vec<TreeRow>,
    pub truncated: bool,
}

pub fn build_visible_rows(snapshot: &InventorySnapshot, root: CategoryKey, expanded: &HashSet<CategoryKey>) -> TreeRows {
    build_visible_rows_cancellable(snapshot, root, expanded, || false).unwrap_or_default()
}

pub fn build_visible_rows_cancellable(
    snapshot: &InventorySnapshot,
    root: CategoryKey,
    expanded: &HashSet<CategoryKey>,
    cancelled: impl FnMut() -> bool,
) -> Option<TreeRows> {
    build_visible_rows_with_limit(snapshot, root, expanded, MAX_TREE_ROWS, cancelled)
}

fn build_visible_rows_with_limit(
    snapshot: &InventorySnapshot,
    root: CategoryKey,
    expanded: &HashSet<CategoryKey>,
    row_limit: usize,
    mut cancelled: impl FnMut() -> bool,
) -> Option<TreeRows> {
    let mut pending = ordered_children(snapshot, root).into_iter().rev().map(|node| (node, 0)).collect::<Vec<_>>();
    let mut rows = Vec::new();
    let mut truncated = false;

    while let Some((node, depth)) = pending.pop() {
        if rows.len().is_multiple_of(SEARCH_CANCEL_INTERVAL) && cancelled() {
            return None;
        }
        if rows.len() == row_limit {
            truncated = true;
            break;
        }
        rows.push(TreeRow { node, depth });
        let InventoryNodeId::Category(id) = node.id() else { continue };
        let key = CategoryKey::new(node.owner(), id);
        if !expanded.contains(&key) {
            continue;
        }
        pending.extend(ordered_children(snapshot, key).into_iter().rev().map(|node| (node, depth + 1)));
    }

    (!cancelled()).then_some(TreeRows { rows, truncated })
}

pub fn search(
    snapshot: &InventorySnapshot,
    query: &str,
    type_filter: SearchTypeFilter,
    cancelled: impl FnMut() -> bool,
) -> Option<SearchResults> {
    search_with_limit(snapshot, query, type_filter, MAX_SEARCH_RESULTS, cancelled)
}

fn search_with_limit(
    snapshot: &InventorySnapshot,
    query: &str,
    type_filter: SearchTypeFilter,
    limit: usize,
    mut cancelled: impl FnMut() -> bool,
) -> Option<SearchResults> {
    let query = query.to_lowercase();
    let library_owner = snapshot.roots().library().map(CategoryKey::owner);
    let mut candidates = BTreeMap::new();
    let mut matched = 0usize;

    for (index, item) in snapshot.items().enumerate() {
        if index.is_multiple_of(SEARCH_CANCEL_INTERVAL) && cancelled() {
            return None;
        }
        if !matches_filter(item, type_filter) {
            continue;
        }
        let name = item.name.to_lowercase();
        if !query.is_empty() && !name.contains(&query) {
            continue;
        }
        matched = matched.saturating_add(1);
        candidates.insert((name, item.key), item);
        if candidates.len() > limit {
            candidates.pop_last();
        }
    }
    if cancelled() {
        return None;
    }

    let items = candidates
        .into_values()
        .map(|item| SearchResult {
            key: item.key,
            name: item.name.clone(),
            asset_type: item.asset_type,
            inventory_type: item.inventory_type,
            folder_path: folder_path(snapshot, item.parent),
            library: library_owner == Some(item.key.owner()),
        })
        .collect();
    Some(SearchResults { items, truncated: matched > limit })
}

fn matches_filter(item: &Item, type_filter: SearchTypeFilter) -> bool {
    match type_filter {
        SearchTypeFilter::All => true,
        SearchTypeFilter::Asset(asset_type) => item.asset_type == asset_type,
        SearchTypeFilter::Inventory(inventory_type) => item.inventory_type == inventory_type,
    }
}

fn folder_path(snapshot: &InventorySnapshot, parent: CategoryKey) -> String {
    let mut names = Vec::new();
    let mut current = Some(parent);
    let mut path_bytes = 0usize;
    let mut truncated = false;

    for _ in 0..MAX_SEARCH_PATH_DEPTH {
        let Some(key) = current else { break };
        let Some(category) = snapshot.category(key) else { break };
        let separator_bytes = usize::from(!names.is_empty()) * 3;
        let next_bytes = path_bytes.saturating_add(separator_bytes).saturating_add(category.name.len());
        if next_bytes > MAX_SEARCH_PATH_BYTES - 8 {
            truncated = true;
            break;
        }
        path_bytes = next_bytes;
        names.push(category.name.as_ref());
        current = category.parent.filter(|parent| *parent != key);
    }
    if current.is_some() {
        truncated = true;
    }

    names.reverse();
    let mut path = String::with_capacity(path_bytes.saturating_add(8));
    if truncated {
        path.push_str("… / ");
    }
    for (index, name) in names.into_iter().enumerate() {
        if index > 0 {
            path.push_str(" / ");
        }
        path.push_str(name);
    }
    path
}

fn ordered_children(snapshot: &InventorySnapshot, parent: CategoryKey) -> Vec<InventoryNodeKey> {
    let mut children = snapshot
        .children(parent)
        .filter_map(|node| {
            let (group, name) = match node.id() {
                InventoryNodeId::Category(id) => {
                    let category = snapshot.category(CategoryKey::new(node.owner(), id))?;
                    let group = match category.preferred_folder_type {
                        ::inventory::FolderType::Trash => 1,
                        folder_type if folder_type.is_protected() => 0,
                        _ => 2,
                    };
                    (group, category.name.to_lowercase())
                }
                InventoryNodeId::Item(id) => {
                    let item = snapshot.item(ItemKey::new(node.owner(), id))?;
                    (3, item.name.to_lowercase())
                }
            };
            Some((group, name, node))
        })
        .collect::<Vec<_>>();
    children.sort_unstable_by(|left, right| (left.0, &left.1, left.2).cmp(&(right.0, &right.1, right.2)));
    children.into_iter().map(|(_, _, node)| node).collect()
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, sync::Arc};

    use ::inventory::{
        AssetType, Category, CategoryKey, FolderType, Inventory, InventoryLimits, InventoryOwner, InventoryType, Item, ItemKey,
        PermissionMask, Permissions, SaleInfo, SaleType, ServerVersion,
    };
    use utils::{AssetId, AvatarId, FolderId, GroupId, ItemId};
    use uuid::Uuid;

    use super::{
        InventoryNodeId, InventoryNodeKey, SearchTypeFilter, build_visible_rows, build_visible_rows_with_limit, search_with_limit,
    };

    #[test]
    fn expanded_rows_hide_collapsed_descendants() {
        let (mut model, personal) = model();
        let folder = category(personal, 3, Some(1), "Folder");
        let key = folder.key;
        model.upsert_category(folder).expect("folder");
        model.upsert_item(item(personal, 4, 3, "Visible child", AssetType::Notecard, InventoryType::Notecard)).expect("child");
        let snapshot = model.snapshot();
        let collapsed = build_visible_rows(&snapshot, snapshot.roots().personal(), &HashSet::new());
        let expanded = build_visible_rows(&snapshot, snapshot.roots().personal(), &HashSet::from([key]));

        assert_eq!(collapsed.rows.len(), 1);
        assert_eq!(collapsed.rows[0].depth, 0);
        assert_eq!(expanded.rows.len(), 2);
        assert_eq!(expanded.rows[1].depth, 1);
    }

    #[test]
    fn folder_groups_match_viewer_order() {
        let (mut model, personal) = model();
        let mut categories = vec![
            category(personal, 3, Some(1), "System Z"),
            category(personal, 4, Some(1), "System A"),
            category(personal, 5, Some(1), "Trash"),
            category(personal, 6, Some(1), "User"),
        ];
        categories[0].preferred_folder_type = FolderType::Texture;
        categories[1].preferred_folder_type = FolderType::Landmark;
        categories[2].preferred_folder_type = FolderType::Trash;
        for category in categories {
            model.upsert_category(category).expect("category");
        }
        let item = item(personal, 7, 1, "Alpha item", AssetType::Notecard, InventoryType::Notecard);
        let item_key = item.key;
        model.upsert_item(item).expect("item");
        let snapshot = model.snapshot();
        let rows = build_visible_rows(&snapshot, snapshot.roots().personal(), &HashSet::new());
        let keys = rows.rows.iter().map(|row| row.node).collect::<Vec<_>>();

        assert_eq!(
            keys,
            [
                node(personal, InventoryNodeId::Category(FolderId::new(uuid(4)))),
                node(personal, InventoryNodeId::Category(FolderId::new(uuid(3)))),
                node(personal, InventoryNodeId::Category(FolderId::new(uuid(5)))),
                node(personal, InventoryNodeId::Category(FolderId::new(uuid(6)))),
                node(personal, InventoryNodeId::Item(item_key.id())),
            ]
        );
    }

    #[test]
    fn tree_rows_report_truncation_at_the_limit() {
        let (mut model, personal) = model();
        for (id, name) in [(3, "Charlie"), (4, "Alpha"), (5, "Bravo")] {
            model.upsert_item(item(personal, id, 1, name, AssetType::Texture, InventoryType::Texture)).expect("item");
        }
        let snapshot = model.snapshot();
        let rows = build_visible_rows_with_limit(&snapshot, snapshot.roots().personal(), &HashSet::new(), 2, || false)
            .expect("tree build completes");

        assert!(rows.truncated);
        assert_eq!(rows.rows.len(), 2);
        assert_eq!(
            rows.rows.iter().map(|row| row.node).collect::<Vec<_>>(),
            [node(personal, InventoryNodeId::Item(ItemId::new(uuid(4)))), node(personal, InventoryNodeId::Item(ItemId::new(uuid(5)))),]
        );
    }

    #[test]
    fn search_filters_case_and_paths() {
        let (mut model, personal) = model();
        let folder = category(personal, 3, Some(1), "Objects");
        let folder_key = folder.key;
        model.upsert_category(folder).expect("folder");
        model.upsert_item(item(personal, 4, 3, "Red Lantern", AssetType::Object, InventoryType::Object)).expect("object");
        model.upsert_item(item(personal, 5, 3, "Red Note", AssetType::Notecard, InventoryType::Notecard)).expect("notecard");
        let library = model.roots().library().expect("library root");
        model.upsert_item(item(library.owner(), 6, 2, "Red Robe", AssetType::Clothing, InventoryType::Wearable)).expect("library item");
        let snapshot = model.snapshot();
        let result =
            search_with_limit(&snapshot, "rEd", SearchTypeFilter::Asset(AssetType::Object), 10, || false).expect("search completes");
        let inventory_type_result = search_with_limit(&snapshot, "rEd", SearchTypeFilter::Inventory(InventoryType::Notecard), 10, || false)
            .expect("inventory type search completes");
        let library_result = search_with_limit(&snapshot, "rEd", SearchTypeFilter::All, 10, || false).expect("search completes");

        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].name.as_ref(), "Red Lantern");
        assert_eq!(result.items[0].folder_path, "Personal / Objects");
        assert_eq!(result.items[0].key, ItemKey::new(personal, ItemId::new(uuid(4))));
        assert_eq!(inventory_type_result.items.len(), 1);
        assert_eq!(inventory_type_result.items[0].name.as_ref(), "Red Note");
        assert_eq!(library_result.items.iter().find(|item| item.library).map(|item| item.folder_path.as_str()), Some("Library"));
        assert_eq!(snapshot.category(folder_key).map(|category| category.name.as_ref()), Some("Objects"));
    }

    #[test]
    fn search_truncates_in_name_order() {
        let (mut model, personal) = model();
        for (id, name) in [(3, "Charlie"), (4, "Alpha"), (5, "Bravo")] {
            model.upsert_item(item(personal, id, 1, name, AssetType::Texture, InventoryType::Texture)).expect("item");
        }
        let result = search_with_limit(&model.snapshot(), "", SearchTypeFilter::All, 2, || false).expect("search completes");

        assert!(result.truncated);
        assert_eq!(result.items.iter().map(|item| item.name.as_ref()).collect::<Vec<_>>(), ["Alpha", "Bravo"]);
    }

    #[test]
    fn search_cancels_during_scan() {
        let (mut model, personal) = model();
        for id in 3..=300 {
            model.upsert_item(item(personal, id, 1, "note", AssetType::Notecard, InventoryType::Notecard)).expect("item");
        }
        let mut checks = 0;
        let result = search_with_limit(&model.snapshot(), "note", SearchTypeFilter::All, 10, || {
            checks += 1;
            checks > 1
        });

        assert!(result.is_none());
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

    fn node(owner: InventoryOwner, id: InventoryNodeId) -> InventoryNodeKey {
        InventoryNodeKey::new(owner, id)
    }

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }
}
