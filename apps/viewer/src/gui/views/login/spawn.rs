use gpui::SharedString;
use gpui_component::select::SelectItem;
use network::Grid;

use crate::inventory::{FavoriteLocation, favorite_store::FavoriteStore};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum SpawnChoice {
    Last,
    Home,
    Region,
    Favorite(FavoriteLocation),
}

#[derive(Clone)]
pub(super) struct SpawnOption {
    title: SharedString,
    choice: SpawnChoice,
}

impl SelectItem for SpawnOption {
    type Value = SpawnChoice;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.choice
    }
}

pub(super) fn spawn_options(favorites: &[FavoriteLocation]) -> Vec<SpawnOption> {
    let mut options = vec![
        SpawnOption { title: "Last location".into(), choice: SpawnChoice::Last },
        SpawnOption { title: "Home".into(), choice: SpawnChoice::Home },
    ];
    options.extend(
        favorites
            .iter()
            .cloned()
            .map(|favorite| SpawnOption { title: format!("★ {}", favorite.name).into(), choice: SpawnChoice::Favorite(favorite) }),
    );
    options.push(SpawnOption { title: "Enter region name…".into(), choice: SpawnChoice::Region });
    options
}

pub(super) fn favorites_for_username(favorites: &FavoriteStore, grid: Grid, username: &str) -> Vec<FavoriteLocation> {
    favorites.agent_id_for_username(grid, username).map(|agent_id| favorites.favorites_for(grid, agent_id).to_vec()).unwrap_or_default()
}
