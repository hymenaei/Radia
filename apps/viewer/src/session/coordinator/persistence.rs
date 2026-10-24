use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use crate::inventory::favorite_store;

pub(crate) type WriteLock = Arc<futures::lock::Mutex<()>>;

pub(crate) fn write_lock() -> WriteLock {
    Arc::new(futures::lock::Mutex::new(()))
}

pub(crate) async fn save_favorite_store(
    store: favorite_store::FavoriteStore,
    lock: WriteLock,
    generation: Arc<AtomicU64>,
    expected_generation: u64,
) -> Option<Result<(), favorite_store::FavoriteStoreError>> {
    save_if_current(store, lock, generation, expected_generation, |store| favorite_store::save(&store)).await
}

async fn save_if_current<T, E>(
    value: T,
    lock: WriteLock,
    generation: Arc<AtomicU64>,
    expected_generation: u64,
    save: impl FnOnce(T) -> Result<(), E>,
) -> Option<Result<(), E>> {
    let _guard = lock.lock().await;
    (generation.load(Ordering::Acquire) == expected_generation).then(|| save(value))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
    };

    use network::Grid;
    use tempfile::tempdir;
    use uuid::Uuid;

    use crate::inventory::{FavoriteLocation, favorite_store::FavoriteStore};

    use super::*;

    #[test]
    fn stale_favorite_snapshot_is_ignored() {
        const AGENT_ID: Uuid = Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        let test_directory = tempdir().expect("create temporary favorite directory");
        let path = test_directory.path().join("favorites.json");

        let mut old_store = FavoriteStore::default();
        old_store
            .replace_favorites(
                Grid::Agni,
                AGENT_ID,
                "Example Resident",
                vec![FavoriteLocation { name: "Old".into(), region_name: "Old Region".into(), x: 1, y: 2, z: 3 }],
            )
            .expect("replace old favorites");
        let mut new_store = FavoriteStore::default();
        new_store
            .replace_favorites(
                Grid::Agni,
                AGENT_ID,
                "Example Resident",
                vec![FavoriteLocation { name: "Newest".into(), region_name: "New Region".into(), x: 4, y: 5, z: 6 }],
            )
            .expect("replace new favorites");

        let (old_result, new_result) = futures::executor::block_on(async {
            let lock = write_lock();
            let guard = lock.lock().await;
            let generation = Arc::new(AtomicU64::new(1));

            let old_path = path.clone();
            let mut old_save = Box::pin(save_if_current(old_store, lock.clone(), Arc::clone(&generation), 1, move |store| {
                let contents = serde_json::to_vec(&store).map_err(|error| error.to_string())?;
                fs::write(old_path, contents).map_err(|error| error.to_string())
            }));
            assert!(futures::poll!(old_save.as_mut()).is_pending());

            let new_path = path.clone();
            let mut new_save = Box::pin(save_if_current(new_store, lock.clone(), Arc::clone(&generation), 2, move |store| {
                let contents = serde_json::to_vec(&store).map_err(|error| error.to_string())?;
                fs::write(new_path, contents).map_err(|error| error.to_string())
            }));
            assert!(futures::poll!(new_save.as_mut()).is_pending());

            generation.store(2, Ordering::Release);
            drop(guard);
            futures::join!(old_save, new_save)
        });

        assert!(old_result.is_none());
        assert_eq!(new_result, Some(Ok(())));
        let saved: FavoriteStore =
            serde_json::from_slice(&fs::read(&path).expect("saved latest favorites")).expect("saved snapshot is valid JSON");
        assert_eq!(saved.favorites_for(Grid::Agni, AGENT_ID)[0].name, "Newest");
    }
}
