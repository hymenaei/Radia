use std::{
    fmt::{self, Display, Formatter},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use gpui::Context;
use network::Grid;
use uuid::Uuid;

use crate::{
    inventory::{
        FavoriteLocation,
        favorite_store::{self, FavoriteStore, FavoriteStoreError},
    },
    session::{FavoritesUpdate, PresentationMailbox, ViewerUpdate, WriteLock, save_favorite_store, write_lock},
};

use super::{LoginScreen, spawn::favorites_for_username};

#[derive(Clone, Debug)]
enum FavoriteWarning {
    Storage(FavoriteStoreError),
    Save(FavoriteStoreError),
    Refresh(FavoriteStoreError),
}

impl Display for FavoriteWarning {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => Display::fmt(error, formatter),
            Self::Save(error) => write!(formatter, "Could not save favorite landmarks: {error}"),
            Self::Refresh(error) => write!(formatter, "Could not refresh favorite landmarks: {error}"),
        }
    }
}

pub(super) struct Favorites {
    locations: Vec<FavoriteLocation>,
    displayed: Vec<FavoriteLocation>,
    store: FavoriteStore,
    storage_warning: Option<FavoriteWarning>,
    refresh_warning: Option<FavoriteWarning>,
    loaded: bool,
    write_lock: WriteLock,
    save_generation: Arc<AtomicU64>,
}

impl Favorites {
    pub(super) fn new() -> Self {
        Self {
            locations: Vec::new(),
            displayed: Vec::new(),
            store: FavoriteStore::default(),
            storage_warning: None,
            refresh_warning: None,
            loaded: false,
            write_lock: write_lock(),
            save_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(super) fn load(&mut self, result: Result<FavoriteStore, FavoriteStoreError>) {
        let (store, warning) = match result {
            Ok(store) => (store, None),
            Err(error) => (FavoriteStore::default(), Some(error)),
        };
        self.store = store;
        self.loaded = warning.is_none();
        self.storage_warning = warning.map(FavoriteWarning::Storage);
    }

    pub(super) fn locations(&self) -> &[FavoriteLocation] {
        &self.locations
    }

    pub(super) fn needs_display_refresh(&self) -> bool {
        self.displayed != self.locations
    }

    pub(super) fn mark_displayed(&mut self) {
        self.displayed.clone_from(&self.locations);
    }

    pub(super) fn for_username(&self, grid: Grid, username: &str) -> Vec<FavoriteLocation> {
        favorites_for_username(&self.store, grid, username)
    }

    pub(super) fn set_locations(&mut self, locations: Vec<FavoriteLocation>) {
        self.locations.clone_from(&locations);
        self.displayed = locations;
    }

    pub(super) fn refresh_for_username(&mut self, grid: Grid, username: &str) -> bool {
        let locations = self.for_username(grid, username);
        if locations == self.locations {
            return false;
        }
        self.locations = locations;
        true
    }

    pub(super) fn prepare_session(
        &mut self,
        grid: Grid,
        agent_id: Uuid,
        username: &str,
        mailbox: Option<PresentationMailbox>,
        cx: &mut Context<LoginScreen>,
    ) {
        let identity_changed = match self.store.remember_agent(grid, agent_id, username) {
            Ok(changed) => changed,
            Err(error) => {
                tracing::warn!(%error, "could not remember favorite account");
                self.storage_warning = Some(FavoriteWarning::Storage(error));
                false
            }
        };
        self.locations = self.store.favorites_for(grid, agent_id).to_vec();
        self.refresh_warning = None;
        if identity_changed && self.loaded {
            self.storage_warning = None;
            self.schedule_save(mailbox, self.store.clone(), cx);
        }
    }

    pub(super) fn apply_update(&mut self, update: FavoritesUpdate, mailbox: Option<PresentationMailbox>, cx: &mut Context<LoginScreen>) {
        if let Err(error) = favorite_store::apply_favorites_refresh(
            &mut self.locations,
            &mut self.store,
            update.grid,
            update.agent_id,
            &update.username,
            update.result,
        ) {
            tracing::warn!(%error, "could not apply favorite inventory refresh");
            self.refresh_warning = Some(FavoriteWarning::Refresh(error));
            return;
        }

        self.refresh_warning = None;
        if self.loaded {
            self.storage_warning = None;
            self.schedule_save(mailbox, self.store.clone(), cx);
        }
    }

    pub(super) fn apply_saved(&mut self, saved: Option<(u64, Result<(), FavoriteStoreError>)>) {
        let Some((generation, result)) = saved else { return };
        if generation != self.save_generation.load(Ordering::Acquire) {
            return;
        }

        match result {
            Ok(()) => self.storage_warning = None,
            Err(error) => {
                tracing::warn!(%error, "could not save favorite landmarks");
                self.storage_warning = Some(FavoriteWarning::Save(error));
            }
        }
    }

    pub(super) fn status_messages(&self) -> Vec<String> {
        self.storage_warning.iter().chain(self.refresh_warning.iter()).map(ToString::to_string).collect()
    }

    fn schedule_save(&self, mailbox: Option<PresentationMailbox>, store: FavoriteStore, cx: &mut Context<LoginScreen>) {
        let Some(mailbox) = mailbox else { return };
        let write_lock = self.write_lock.clone();
        let save_generation = Arc::clone(&self.save_generation);
        let generation = save_generation.fetch_add(1, Ordering::AcqRel) + 1;
        cx.background_executor()
            .spawn(async move {
                if let Some(result) = save_favorite_store(store, write_lock, save_generation, generation).await {
                    mailbox.publish(ViewerUpdate::FavoriteStoreSaved { generation, result });
                }
            })
            .detach();
    }
}
