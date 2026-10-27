use std::{
    collections::{HashMap, HashSet, VecDeque},
    error::Error,
    fmt,
    sync::Arc,
};

use ::inventory::{Category, CategoryKey, FolderType, Inventory, InventoryError, InventoryLimits, InventoryOwner, ServerVersion};
use protocol::login::{InventoryBootstrap as LoginInventoryBootstrap, InventorySkeleton, InventorySkeletonFolder, ResponseData};
use tokio::{runtime::Handle, task::JoinHandle};
use utils::FolderId;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InventoryBootstrapReport {
    pub(crate) personal_categories: usize,
    pub(crate) library_categories: usize,
    pub(crate) rejected_personal_entries: usize,
    pub(crate) rejected_library_entries: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InventoryBootstrapError {
    MissingPersonalRoot,
    IncompleteLibrary,
    Model(InventoryError),
}

impl fmt::Display for InventoryBootstrapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPersonalRoot => formatter.write_str("login response did not provide a personal inventory root"),
            Self::IncompleteLibrary => formatter.write_str("login response provided only part of the Library identity"),
            Self::Model(error) => write!(formatter, "{error}"),
        }
    }
}

impl Error for InventoryBootstrapError {}

impl From<InventoryError> for InventoryBootstrapError {
    fn from(error: InventoryError) -> Self {
        Self::Model(error)
    }
}

struct ScopeCategories {
    root: Category,
    categories: Vec<Category>,
    rejected: usize,
}

pub(crate) fn build_inventory(
    agent_id: Uuid,
    bootstrap: LoginInventoryBootstrap,
) -> Result<(Inventory, InventoryBootstrapReport), InventoryBootstrapError> {
    let personal_root_id = bootstrap.personal_root.ok_or(InventoryBootstrapError::MissingPersonalRoot)?;
    if personal_root_id.is_nil() || agent_id.is_nil() {
        return Err(InventoryError::InvalidOwner.into());
    }
    if bootstrap.library_root.is_some() != bootstrap.library_owner.is_some() {
        return Err(InventoryBootstrapError::IncompleteLibrary);
    }

    let personal = build_scope(InventoryOwner::new(agent_id), personal_root_id, bootstrap.personal_skeleton);
    let library_skeleton_rejected = bootstrap.library_skeleton.rejected_entries;
    let library = match (bootstrap.library_root, bootstrap.library_owner) {
        (Some(root_id), Some(owner_id)) => {
            if root_id.is_nil() || owner_id.is_nil() {
                return Err(InventoryError::InvalidOwner.into());
            }
            Some(build_scope(InventoryOwner::new(owner_id), root_id, bootstrap.library_skeleton))
        }
        (None, None) => None,
        _ => return Err(InventoryBootstrapError::IncompleteLibrary),
    };
    let personal_categories = personal.categories.len().saturating_add(1);
    let library_categories = library.as_ref().map_or(0, |scope| scope.categories.len().saturating_add(1));
    let rejected_personal_entries = personal.rejected;
    let rejected_library_entries = library.as_ref().map_or(library_skeleton_rejected, |scope| scope.rejected);
    let ScopeCategories { root: personal_root, mut categories, .. } = personal;
    let library_root = library.map(|scope| {
        categories.extend(scope.categories);
        scope.root
    });
    let inventory = Inventory::from_categories(personal_root, library_root, categories, InventoryLimits::default())?;
    Ok((
        inventory,
        InventoryBootstrapReport { personal_categories, library_categories, rejected_personal_entries, rejected_library_entries },
    ))
}

pub(crate) fn spawn_inventory_build(
    runtime: &Handle,
    agent_id: Uuid,
    login_data: ResponseData,
) -> JoinHandle<Result<(Inventory, InventoryBootstrapReport), String>> {
    runtime.spawn_blocking(move || {
        let bootstrap = login_data.inventory_bootstrap().map_err(|error| error.to_string())?;
        build_inventory(agent_id, bootstrap).map_err(|error| error.to_string())
    })
}

fn build_scope(owner: InventoryOwner, root_id: Uuid, skeleton: InventorySkeleton) -> ScopeCategories {
    let root_id = FolderId::new(root_id);
    let mut groups = HashMap::<FolderId, Vec<InventorySkeletonFolder>>::new();
    for folder in skeleton.folders {
        groups.entry(FolderId::new(folder.folder_id)).or_default().push(folder);
    }
    let mut rejected = skeleton.rejected_entries;
    let mut unique = HashMap::with_capacity(groups.len());
    for (id, entries) in groups {
        if let [folder] = entries.as_slice() {
            unique.insert(id, folder.clone());
        } else {
            rejected = rejected.saturating_add(entries.len());
        }
    }

    let root = match unique.remove(&root_id) {
        Some(folder) if folder.parent_id.is_nil() => category(owner, root_id, None, folder),
        Some(_) => {
            rejected = rejected.saturating_add(1);
            empty_root(owner, root_id)
        }
        None => empty_root(owner, root_id),
    };
    let mut candidates = HashMap::with_capacity(unique.len());
    for (id, folder) in unique {
        if folder.parent_id.is_nil() {
            rejected = rejected.saturating_add(1);
            continue;
        }
        let parent = CategoryKey::new(owner, FolderId::new(folder.parent_id));
        candidates.insert(id, category(owner, id, Some(parent), folder));
    }

    let mut children = HashMap::<FolderId, Vec<FolderId>>::new();
    for (id, category) in &candidates {
        if let Some(parent) = category.parent {
            children.entry(parent.id()).or_default().push(*id);
        }
    }
    for child_ids in children.values_mut() {
        child_ids.sort_unstable();
    }
    let mut reached = HashSet::with_capacity(candidates.len());
    let mut order = Vec::with_capacity(candidates.len().saturating_add(1));
    let mut queue = VecDeque::new();
    reached.insert(root_id);
    order.push(root_id);
    queue.push_back(root_id);
    while let Some(parent) = queue.pop_front() {
        if let Some(child_ids) = children.get(&parent) {
            for child in child_ids {
                if reached.insert(*child) {
                    order.push(*child);
                    queue.push_back(*child);
                }
            }
        }
    }
    rejected = rejected.saturating_add(candidates.len().saturating_sub(reached.len().saturating_sub(1)));

    let categories = order.into_iter().filter(|id| *id != root_id).filter_map(|id| candidates.remove(&id)).collect();
    ScopeCategories { root, categories, rejected }
}

fn category(owner: InventoryOwner, id: FolderId, parent: Option<CategoryKey>, folder: InventorySkeletonFolder) -> Category {
    Category {
        key: CategoryKey::new(owner, id),
        parent,
        preferred_folder_type: FolderType::from_raw(folder.type_default.unwrap_or(-1)),
        name: Arc::from(folder.name),
        server_version: ServerVersion::from_raw(folder.version),
        server_descendent_count: None,
    }
}

fn empty_root(owner: InventoryOwner, id: FolderId) -> Category {
    Category {
        key: CategoryKey::new(owner, id),
        parent: None,
        preferred_folder_type: FolderType::None,
        name: Arc::from(""),
        server_version: ServerVersion::UNKNOWN,
        server_descendent_count: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::login::{InventorySkeleton, InventorySkeletonFolder, Value};
    use std::{collections::BTreeMap, time::Duration};
    use tokio::time::timeout;

    fn uuid(id: u128) -> Uuid {
        Uuid::from_u128(id)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn max_login_skeleton_does_not_block_intake() {
        let root = uuid(1);
        let folders = (1..=protocol::login::MAX_LOGIN_INVENTORY_FOLDERS)
            .map(|id| {
                let id = uuid(id as u128);
                let parent_id = if id == root { Uuid::nil() } else { root };
                Value::Struct(BTreeMap::from([
                    ("folder_id".into(), Value::String(id.to_string())),
                    ("parent_id".into(), Value::String(parent_id.to_string())),
                    ("name".into(), Value::String("folder".into())),
                    ("version".into(), Value::Integer(-1)),
                ]))
            })
            .collect();
        let login_data = ResponseData::from_fields(BTreeMap::from([
            (
                "inventory-root".into(),
                Value::Array(vec![Value::Struct(BTreeMap::from([("folder_id".into(), Value::String(root.to_string()))]))]),
            ),
            ("inventory-skeleton".into(), Value::Array(folders)),
        ]));
        let started = std::time::Instant::now();
        let build = spawn_inventory_build(&Handle::current(), uuid(10), login_data);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _ = sender.send(());
        });

        timeout(Duration::from_millis(100), receiver)
            .await
            .expect("session intake should run while login folders are built")
            .expect("intake signal should arrive");
        assert!(started.elapsed() < Duration::from_millis(100));
        let (inventory, report) = build.await.expect("inventory worker should finish").expect("valid login inventory should build");
        assert_eq!(report.personal_categories, protocol::login::MAX_LOGIN_INVENTORY_FOLDERS);
        assert_eq!(inventory.snapshot().categories().count(), protocol::login::MAX_LOGIN_INVENTORY_FOLDERS);
    }

    fn folder(id: u128, parent_id: u128, name: &str, type_default: Option<i32>) -> InventorySkeletonFolder {
        InventorySkeletonFolder { folder_id: uuid(id), parent_id: uuid(parent_id), name: name.to_owned(), version: -1, type_default }
    }

    #[test]
    fn builds_personal_and_library_with_distinct_owners() {
        let bootstrap = LoginInventoryBootstrap {
            personal_root: Some(uuid(1)),
            personal_skeleton: InventorySkeleton {
                folders: vec![folder(1, 0, "Personal", Some(8)), folder(2, 1, "Favorites", Some(23))],
                rejected_entries: 0,
            },
            library_root: Some(uuid(3)),
            library_owner: Some(uuid(30)),
            library_skeleton: InventorySkeleton {
                folders: vec![folder(3, 0, "Library", Some(52)), folder(2, 3, "Library child", Some(23))],
                rejected_entries: 0,
            },
        };

        let (inventory, report) = build_inventory(uuid(10), bootstrap).expect("valid login inventory should build");
        let snapshot = inventory.snapshot();
        let personal_root = snapshot.roots().personal();
        let library_root = snapshot.roots().library().expect("Library root should be present");
        let personal_child = CategoryKey::new(personal_root.owner(), FolderId::new(uuid(2)));
        let library_child = CategoryKey::new(library_root.owner(), FolderId::new(uuid(2)));

        assert_ne!(personal_root.owner(), library_root.owner());
        assert_eq!(snapshot.category(personal_child).map(|category| category.name.as_ref()), Some("Favorites"));
        assert_eq!(snapshot.category(library_child).map(|category| category.name.as_ref()), Some("Library child"));
        assert_eq!(
            snapshot.category_with_type(personal_root.owner(), FolderType::Favorite).map(|category| category.key),
            Some(personal_child)
        );
        assert_eq!(report.personal_categories, 2);
        assert_eq!(report.library_categories, 2);
        assert_eq!(report.rejected_personal_entries, 0);
        assert_eq!(report.rejected_library_entries, 0);
    }

    #[test]
    fn counts_duplicate_orphan_and_cyclic_entries() {
        let bootstrap = LoginInventoryBootstrap {
            personal_root: Some(uuid(1)),
            personal_skeleton: InventorySkeleton {
                folders: vec![
                    folder(1, 0, "Personal", None),
                    folder(2, 1, "Duplicate", None),
                    folder(2, 1, "Duplicate", None),
                    folder(3, 99, "Orphan", None),
                    folder(4, 5, "Cycle A", None),
                    folder(5, 4, "Cycle B", None),
                ],
                rejected_entries: 1,
            },
            ..LoginInventoryBootstrap::default()
        };

        let (inventory, report) = build_inventory(uuid(10), bootstrap).expect("the valid root should still build");

        assert_eq!(report.rejected_personal_entries, 6);
        assert_eq!(report.personal_categories, 1);
        assert_eq!(inventory.snapshot().categories().count(), 1);
    }
}
